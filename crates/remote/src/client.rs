//! The client side of pairing and channel setup. [`crate::connection`]
//! keeps a channel to a paired host alive on top of this.
//!
//! A host is reached on the LAN or through its relay. LAN attempts start at
//! once; the relay joins after a short head start for the LAN, or at once
//! when every LAN attempt failed, and the first handshake to complete wins.
//! Both paths run the same end-to-end channel. A [`PathPolicy`] can pin a
//! link to either path.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;
use std::{error, fmt};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::StreamExt as _;
use futures::stream::FuturesUnordered;
use nmt_remote_core::channel::{Channel, ClientHandshake};
use nmt_remote_core::identity::DeviceKey;
use nmt_remote_core::messages::{ClientHello, DeviceInfo, HostHello, PairAccepted, RelayAccess};
use nmt_remote_core::pairing::{ClientPairing, PairingCode};
use nmt_remote_core::preface::{Preface, PrefaceKind};
use nmt_remote_core::{PROTO_MAJOR, PROTO_MINOR};
use parking_lot::Mutex;
use tokio::select;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::connect_async;
use tracing::warn;

use crate::discovery::{self, Target};
use crate::link::{recv_binary, send_binary};
use crate::relay::{RelaySocket, dial_host, dial_pairing};
use crate::store::{PairedHost, StoredRelay, now_ms};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a lookup browses the LAN before giving up.
const DISCOVERY_WAIT: Duration = Duration::from_secs(3);

/// The LAN's head start before the relay is tried as well.
const RELAY_DELAY: Duration = Duration::from_millis(300);

/// Addresses remembered per host, most recently working first.
const MAX_LAN_HINTS: usize = 4;

/// How long one LAN address may take to open a channel. A host on the same
/// network answers in milliseconds; an address the host no longer has, or a
/// network that keeps devices apart, only times out. All candidates are
/// tried together and the relay starts after [`RELAY_DELAY`] regardless, so
/// a stale address delays nothing unless the relay is out of reach too.
const LAN_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);

const FEATURES: &[&str] = &["terminal"];

/// Which paths a link may take to its host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PathPolicy {
    /// The LAN when it answers, otherwise the relay; a link through the
    /// relay moves to the LAN once the LAN answers.
    #[default]
    Auto,
    /// The relay only, for networks where the LAN reaches the host but is
    /// not trusted, or to exercise the relay path on purpose.
    Relay,
    /// The LAN only, so no traffic leaves the local network.
    Lan,
}

impl PathPolicy {
    /// Whether a link on `path` may stay up under this policy.
    pub fn allows(self, path: &LinkPath) -> bool {
        match (self, path) {
            (Self::Auto, _) | (Self::Relay, LinkPath::Relay) | (Self::Lan, LinkPath::Lan(_)) => {
                true
            }
            (Self::Relay, LinkPath::Lan(_)) | (Self::Lan, LinkPath::Relay) => false,
        }
    }
}

/// How a channel reaches its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkPath {
    /// Directly, at this LAN address.
    Lan(String),
    Relay,
}

/// The host completed the preface but closed instead of answering the
/// handshake: this device is not, or no longer, paired with it.
#[derive(Debug)]
pub(crate) struct Refused;

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the host refused this device; pair again")
    }
}

impl error::Error for Refused {}

/// Hands out `hello_ms` values to concurrent connection attempts: each one
/// exceeds every value before it, so the host never takes a later attempt
/// for a replay of an earlier one.
struct HelloClock(Mutex<u64>);

impl HelloClock {
    fn next(&self) -> u64 {
        let mut last = self.0.lock();

        *last = now_ms().max(*last + 1);

        *last
    }

    fn last(&self) -> u64 {
        *self.0.lock()
    }
}

/// Pair with the host showing `code`. With an `address` (`host:port`) that
/// address is tried; otherwise the LAN is searched for the code's slot.
/// When a relay is known (from a pairing link), the relay is tried in
/// parallel either way. A key from a pairing link makes the exchange fail unless that
/// exact host answers.
pub async fn pair(
    address: Option<&str>,
    code: &PairingCode,
    key: &DeviceKey,
    device: DeviceInfo,
    expected_host_key: Option<[u8; 32]>,
    relay: Option<RelayAccess>,
) -> Result<PairedHost> {
    let lan = async {
        let address = match address {
            Some(address) => address.to_owned(),
            None => discovery::find(Target::PairingSlot(code.slot()), DISCOVERY_WAIT)
                .await
                .context("no computer on this network is showing that code; enter its address")?,
        };

        let ws = connect_lan(&address).await?;

        let (host_key, accepted) =
            pair_over(ws, code, key, device.clone(), expected_host_key).await?;

        anyhow::Ok((host_key, accepted, Some(address)))
    };

    // An address from a pairing link is the host's LAN address, which a
    // client on another network cannot reach, so the relay still races it.
    let paired = async {
        match &relay {
            Some(relay) => {
                race(lan, || async {
                    let ws = dial_pairing(relay, code.slot()).await?;

                    let (host_key, accepted) =
                        pair_over(ws, code, key, device.clone(), expected_host_key).await?;

                    anyhow::Ok((host_key, accepted, None))
                })
                .await
            }
            None => lan.await,
        }
    };

    let (host_key, accepted, address) = timeout(CONNECT_TIMEOUT * 2, paired)
        .await
        .map_err(|_| anyhow!("pairing timed out"))??;

    let mut host = PairedHost::new(host_key, accepted.host.name, address.clone());

    // A device that paired through the relay learns where the host is on
    // its LAN, so it can go direct once it is on that network.
    host.lan_hints = merge_lan_hints(&accepted.lan_hints, address.as_deref(), &host.lan_hints);

    // The host's relay, handed over inside the encrypted exchange, reaches
    // it later from off the LAN.
    if let Some(relay) = accepted.relay.as_ref() {
        match StoredRelay::seal(relay) {
            Ok(stored) => host.relay = Some(stored),
            Err(error) => warn!(%error, "cannot store the host's relay"),
        }
    }

    Ok(host)
}

/// Run the pairing exchange over an open socket.
async fn pair_over(
    mut ws: RelaySocket,
    code: &PairingCode,
    key: &DeviceKey,
    device: DeviceInfo,
    expected_host_key: Option<[u8; 32]>,
) -> Result<([u8; 32], PairAccepted)> {
    timeout(CONNECT_TIMEOUT, async {
        let (offer, answer) = negotiate(&mut ws, PrefaceKind::Pairing).await?;

        let (pairing, hello) =
            ClientPairing::start(code, &offer, &answer, device, expected_host_key)?;

        send_binary(&mut ws, hello).await?;

        let reply = recv_binary(&mut ws).await?;
        let (handshake, msg1) = pairing.on_reply(key, &reply)?;

        send_binary(&mut ws, msg1).await?;

        let msg2 = recv_binary(&mut ws).await?;
        let (confirm, msg3) = handshake.on_msg2(&msg2)?;

        send_binary(&mut ws, msg3).await?;

        // A host that rejects the code closes without answering.
        let accepted = recv_binary(&mut ws)
            .await
            .context("the host did not accept the pairing code")?;

        let paired = confirm.on_accepted(&accepted)?;

        Ok((paired.public_key, paired.accepted))
    })
    .await
    .map_err(|_| anyhow!("pairing timed out"))?
}

/// Open a channel to a paired host on the LAN or through its relay, as
/// `policy` allows. `host` records the handshake time, the host's current
/// name, and the LAN address that worked; the caller stores it afterwards.
/// [`Refused`] means the host no longer trusts this device.
pub(crate) async fn establish(
    host: &mut PairedHost,
    key: &DeviceKey,
    app_version: &str,
    policy: PathPolicy,
) -> Result<(RelaySocket, Channel, LinkPath)> {
    let clock = HelloClock(Mutex::new(host.last_hello_ms));

    let relay = host.relay.as_ref().and_then(|stored| {
        stored
            .open()
            .inspect_err(|error| warn!(%error, "cannot read the host's relay key"))
            .ok()
    });

    let result = match (policy, &relay) {
        (PathPolicy::Lan, _) | (PathPolicy::Auto, None) => {
            lan_path(host, key, app_version, &clock).await
        }
        (PathPolicy::Relay, None) => Err(anyhow!("this computer was paired without a relay")),
        (PathPolicy::Relay, Some(relay)) => relay_path(host, key, app_version, &clock, relay).await,
        (PathPolicy::Auto, Some(relay)) => {
            race(lan_path(host, key, app_version, &clock), || {
                relay_path(host, key, app_version, &clock, relay)
            })
            .await
        }
    };

    host.last_hello_ms = clock.last();

    let (ws, channel, host_hello, address) = result?;

    Ok(finish(host, ws, channel, host_hello, address))
}

/// Open a channel over the LAN only, for a link that runs through the relay
/// and would rather go direct. Updates `host` as [`establish`] does.
pub(crate) async fn establish_lan(
    host: &mut PairedHost,
    key: &DeviceKey,
    app_version: &str,
) -> Result<(RelaySocket, Channel, LinkPath)> {
    let clock = HelloClock(Mutex::new(host.last_hello_ms));

    let result = lan_path(host, key, app_version, &clock).await;

    host.last_hello_ms = clock.last();

    let (ws, channel, host_hello, address) = result?;

    Ok(finish(host, ws, channel, host_hello, address))
}

/// Open a channel through the host's relay.
async fn relay_path(
    host: &PairedHost,
    key: &DeviceKey,
    app_version: &str,
    clock: &HelloClock,
    relay: &RelayAccess,
) -> Result<(RelaySocket, Channel, HostHello, Option<String>)> {
    let ws = dial_host(relay, &host.id).await?;
    let hello = hello(app_version, clock);

    let (ws, channel, host_hello) = timeout(
        CONNECT_TIMEOUT,
        channel_handshake(ws, key, &host.public_key, &hello),
    )
    .await
    .map_err(|_| anyhow!("connecting through the relay timed out"))??;

    Ok((ws, channel, host_hello, None))
}

/// Record what a handshake taught about the host, and say how it went.
fn finish(
    host: &mut PairedHost,
    ws: RelaySocket,
    channel: Channel,
    host_hello: HostHello,
    address: Option<String>,
) -> (RelaySocket, Channel, LinkPath) {
    host.lan_hints = merge_lan_hints(&host_hello.lan_hints, address.as_deref(), &host.lan_hints);

    host.name = host_hello.name;
    host.last_seen = now_ms();

    let path = address.map_or(LinkPath::Relay, LinkPath::Lan);

    (ws, channel, path)
}

/// Try the host's known LAN addresses, then wherever DNS-SD finds it now
/// (DHCP may have moved it).
async fn lan_path(
    host: &PairedHost,
    key: &DeviceKey,
    app_version: &str,
    clock: &HelloClock,
) -> Result<(RelaySocket, Channel, HostHello, Option<String>)> {
    let mut last_error = anyhow!("no known address for this host");
    let mut refused = None;

    for rediscover in [false, true] {
        let candidates = if rediscover {
            match discovery::find(Target::Device(&host.id), DISCOVERY_WAIT).await {
                Some(address) if !host.lan_hints.contains(&address) => vec![address],
                _ => break,
            }
        } else {
            host.lan_hints.clone()
        };

        // Every candidate at once: a stale address costs its timeout only
        // while the others are already trying. Dropping the rest once one
        // succeeds closes their sockets; a handshake that reached the host
        // too only opens and closes a channel there.
        let mut attempts: FuturesUnordered<_> = candidates
            .into_iter()
            .map(|address| {
                // A fresh hello per attempt, each later than the one before,
                // so the host takes none of them for a replay.
                let hello = hello(app_version, clock);

                async move {
                    let attempt = async {
                        let ws = connect_lan(&address).await?;

                        channel_handshake(ws, key, &host.public_key, &hello).await
                    };

                    let result = timeout(LAN_ATTEMPT_TIMEOUT, attempt)
                        .await
                        .unwrap_or_else(|_| Err(anyhow!("connecting to {address} timed out")));

                    (address, result)
                }
            })
            .collect();

        while let Some((address, result)) = attempts.next().await {
            match result {
                Ok((ws, channel, host_hello)) => {
                    return Ok((ws, channel, host_hello, Some(address)));
                }
                // Two addresses of one host (Wi-Fi and Ethernet) reach it
                // with two handshakes, and the host refuses whichever
                // arrives with the older hello as a replay. A refusal only
                // counts once no other address got through.
                Err(error) if error.is::<Refused>() => refused = Some(error),
                Err(error) => last_error = error,
            }
        }

        // The right host answered and said no; rediscovering it would reach
        // the same host.
        if let Some(refused) = refused {
            return Err(refused);
        }
    }

    Err(last_error)
}

/// The LAN addresses to remember for a host: the one that just worked
/// first, then the addresses the host says it has now. The host's list
/// replaces what was learned before, so an address DHCP has since handed to
/// another machine drops out instead of being tried forever. A host too old
/// to send a list leaves the known addresses in place.
pub(crate) fn merge_lan_hints(
    advertised: &[String],
    working: Option<&str>,
    known: &[String],
) -> Vec<String> {
    let advertised: Vec<&str> = advertised
        .iter()
        .map(String::as_str)
        .filter(|address| address.parse::<SocketAddr>().is_ok())
        .collect();

    let rest = if advertised.is_empty() {
        known.iter().map(String::as_str).collect()
    } else {
        advertised
    };

    let mut hints: Vec<String> = Vec::new();

    for address in working.into_iter().chain(rest) {
        if !hints.iter().any(|known| known == address) {
            hints.push(address.to_owned());
        }
    }

    hints.truncate(MAX_LAN_HINTS);

    hints
}

/// Run `lan`, and `relay` too once the LAN has had a head start or has
/// failed, returning the first success. A refusal from either path ends the
/// race: it comes from the host itself, which any path reaches.
async fn race<T, L, R, F>(lan: L, relay: R) -> Result<T>
where
    L: Future<Output = Result<T>>,
    R: FnOnce() -> F,
    F: Future<Output = Result<T>>,
{
    tokio::pin!(lan);

    let head_start = select! {
        result = &mut lan => Some(result),
        () = sleep(RELAY_DELAY) => None,
    };

    let lan_error = match head_start {
        Some(Ok(value)) => return Ok(value),
        Some(Err(error)) if error.is::<Refused>() => return Err(error),
        Some(Err(error)) => Some(error),
        None => None,
    };

    let relay = relay();

    tokio::pin!(relay);

    if let Some(lan_error) = lan_error {
        return relay
            .await
            .map_err(|relay_error| pick_error(lan_error, relay_error));
    }

    select! {
        result = &mut lan => match result {
            Ok(value) => Ok(value),
            Err(error) if error.is::<Refused>() => Err(error),
            Err(lan_error) => relay.await.map_err(|relay_error| pick_error(lan_error, relay_error)),
        },
        result = &mut relay => match result {
            Ok(value) => Ok(value),
            Err(error) if error.is::<Refused>() => Err(error),
            Err(relay_error) => lan.await.map_err(|lan_error| pick_error(lan_error, relay_error)),
        },
    }
}

/// A refusal says the most; otherwise the relay's error, which is the path
/// that works from anywhere.
fn pick_error(lan: anyhow::Error, relay: anyhow::Error) -> anyhow::Error {
    if lan.is::<Refused>() { lan } else { relay }
}

fn hello(app_version: &str, clock: &HelloClock) -> ClientHello {
    ClientHello {
        proto_minor: PROTO_MINOR,
        app_version: app_version.to_owned(),
        features: FEATURES.iter().map(|&feature| feature.into()).collect(),
        hello_ms: clock.next(),
    }
}

/// Run the channel handshake over an open socket.
async fn channel_handshake(
    mut ws: RelaySocket,
    key: &DeviceKey,
    host_key: &[u8; 32],
    hello: &ClientHello,
) -> Result<(RelaySocket, Channel, HostHello)> {
    let (offer, answer) = negotiate(&mut ws, PrefaceKind::Channel).await?;
    let (handshake, msg1) = ClientHandshake::start(key, host_key, &offer, &answer, hello)?;

    send_binary(&mut ws, msg1).await?;

    // An unpaired or revoked device gets no reply, only a closed socket.
    let msg2 = recv_binary(&mut ws).await.map_err(|_| Refused)?;

    let (channel, host_hello) = handshake.finish(&msg2)?;

    Ok((ws, channel, host_hello))
}

async fn connect_lan(address: &str) -> Result<RelaySocket> {
    let (ws, _) = connect_async(format!("ws://{address}/v1")).await?;

    Ok(ws)
}

/// Agree on a protocol major with the host.
async fn negotiate(ws: &mut RelaySocket, kind: PrefaceKind) -> Result<(Preface, Preface)> {
    let offer = Preface::offer(kind);

    send_binary(ws, offer.encode().to_vec()).await?;

    let answer = Preface::decode(&recv_binary(ws).await?)?;

    if answer.kind == PrefaceKind::Rejected {
        if answer.min_major > PROTO_MAJOR {
            bail!("this computer runs an older remote protocol; update NiumaTerm here");
        }

        bail!("the other computer runs an older remote protocol; update NiumaTerm there");
    }

    Ok((offer, answer))
}
