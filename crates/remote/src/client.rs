//! The client side of pairing and channel setup. [`crate::connection`]
//! keeps a channel to a paired host alive on top of this.

use std::time::Duration;
use std::{error, fmt};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::{Sink, Stream};
use nmt_remote_core::channel::{Channel, ClientHandshake};
use nmt_remote_core::identity::DeviceKey;
use nmt_remote_core::messages::{ClientHello, DeviceInfo, HostHello};
use nmt_remote_core::pairing::{ClientPairing, PairingCode};
use nmt_remote_core::preface::{Preface, PrefaceKind};
use nmt_remote_core::{PROTO_MAJOR, PROTO_MINOR};
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::discovery::{self, Target};
use crate::link::{recv_binary, send_binary};
use crate::store::{PairedHost, now_ms};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a lookup browses the LAN before giving up.
const DISCOVERY_WAIT: Duration = Duration::from_secs(3);

/// Addresses remembered per host, most recently working first.
const MAX_LAN_HINTS: usize = 4;

const FEATURES: &[&str] = &["terminal"];

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

/// Pair with the host showing `code`, at `address` (`host:port`) or, when
/// none is given, wherever the LAN advertises that code's slot. A key from a
/// pairing link makes the exchange fail unless that exact host answers.
pub async fn pair(
    address: Option<&str>,
    code: &PairingCode,
    key: &DeviceKey,
    device: DeviceInfo,
    expected_host_key: Option<[u8; 32]>,
) -> Result<PairedHost> {
    let address = match address {
        Some(address) => address.to_owned(),
        None => discovery::find(Target::PairingSlot(code.slot()), DISCOVERY_WAIT)
            .await
            .context("no computer on this network is showing that code; enter its address")?,
    };

    timeout(CONNECT_TIMEOUT, async {
        let (mut ws, offer, answer) = open(&address, PrefaceKind::Pairing).await?;

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

        Ok(PairedHost::new(
            paired.public_key,
            paired.accepted.host.name,
            address.clone(),
        ))
    })
    .await
    .map_err(|_| anyhow!("pairing timed out"))?
}

/// Open a channel to a paired host: its known LAN addresses first, then
/// wherever DNS-SD finds it now (DHCP may have moved it). `host` records the
/// handshake time, the host's current name, and the address that worked;
/// the caller stores it afterwards. [`Refused`] means the host no longer
/// trusts this device.
pub(crate) async fn establish(
    host: &mut PairedHost,
    key: &DeviceKey,
    app_version: &str,
) -> Result<(
    impl Stream<Item = Result<Message, WsError>>
    + Sink<Message, Error = WsError>
    + Unpin
    + Send
    + 'static,
    Channel,
)> {
    let mut last_error = anyhow!("no known address for this host");

    for rediscover in [false, true] {
        let candidates = if rediscover {
            match discovery::find(Target::Device(&host.id), DISCOVERY_WAIT).await {
                Some(address) if !host.lan_hints.contains(&address) => vec![address],
                _ => break,
            }
        } else {
            host.lan_hints.clone()
        };

        for address in candidates {
            // A fresh hello per attempt: the host may have recorded the
            // previous one before the attempt failed.
            let hello = ClientHello {
                proto_minor: PROTO_MINOR,
                app_version: app_version.to_owned(),
                features: FEATURES.iter().map(|&feature| feature.into()).collect(),
                hello_ms: host.next_hello_ms(),
            };

            match timeout(
                CONNECT_TIMEOUT,
                handshake(&address, key, &host.public_key, &hello),
            )
            .await
            {
                Ok(Ok((ws, channel, host_hello))) => {
                    host.lan_hints.retain(|known| known != &address);
                    host.lan_hints.insert(0, address);
                    host.lan_hints.truncate(MAX_LAN_HINTS);

                    host.name = host_hello.name;
                    host.last_seen = now_ms();

                    return Ok((ws, channel));
                }
                // The right host answered and said no; another address
                // would reach the same host.
                Ok(Err(error)) if error.is::<Refused>() => return Err(error),
                Ok(Err(error)) => last_error = error,
                Err(_) => last_error = anyhow!("connecting to {address} timed out"),
            }
        }
    }

    Err(last_error)
}

/// Run the channel handshake with the host at `address`.
async fn handshake(
    address: &str,
    key: &DeviceKey,
    host_key: &[u8; 32],
    hello: &ClientHello,
) -> Result<(
    impl Stream<Item = Result<Message, WsError>>
    + Sink<Message, Error = WsError>
    + Unpin
    + Send
    + 'static,
    Channel,
    HostHello,
)> {
    let (mut ws, offer, answer) = open(address, PrefaceKind::Channel).await?;

    let (handshake, msg1) = ClientHandshake::start(key, host_key, &offer, &answer, hello)?;

    send_binary(&mut ws, msg1).await?;

    // An unpaired or revoked device gets no reply, only a closed socket.
    let msg2 = recv_binary(&mut ws).await.map_err(|_| Refused)?;

    let (channel, host_hello) = handshake.finish(&msg2)?;

    Ok((ws, channel, host_hello))
}

/// Connect a WebSocket to `address` and agree on a protocol major.
async fn open(
    address: &str,
    kind: PrefaceKind,
) -> Result<(
    impl Stream<Item = Result<Message, WsError>>
    + Sink<Message, Error = WsError>
    + Unpin
    + Send
    + 'static,
    Preface,
    Preface,
)> {
    let (mut ws, _) = connect_async(format!("ws://{address}/v1")).await?;

    let offer = Preface::offer(kind);

    send_binary(&mut ws, offer.encode().to_vec()).await?;

    let answer = Preface::decode(&recv_binary(&mut ws).await?)?;

    if answer.kind == PrefaceKind::Rejected {
        if answer.min_major > PROTO_MAJOR {
            bail!("this computer runs an older remote protocol; update NiumaTerm here");
        }

        bail!("the other computer runs an older remote protocol; update NiumaTerm there");
    }

    Ok((ws, offer, answer))
}
