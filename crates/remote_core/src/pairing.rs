//! One-time pairing: a short code shown on the host becomes a strong shared
//! key through SPAKE2, and a `Noise_XXpsk3` handshake under that key swaps
//! the two devices' static keys.
//!
//! The code has only 25 secret bits. That is enough because the PAKE lets an
//! attacker, including a malicious relay, test one guess per live attempt
//! against the host and none offline, and the host allows three attempts per
//! code. Used directly as a Noise PSK, the same code would fall to an offline
//! search of a recorded handshake.
//!
//! Message order, each side's methods consuming the peer's previous message:
//!
//! ```text
//! C -> H  PairHello { v, slot, spake A }     cleartext
//! H -> C  PairReply { spake B }              cleartext
//! C -> H  msg1: e
//! H -> C  msg2: e, ee, s, es                 empty payload
//! C -> H  msg3: s, se, psk                   client DeviceInfo
//! H -> C  transport: PairAccepted
//! ```

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use snow::{Builder, HandshakeState, TransportState};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use url::Url;

use crate::identity::{DeviceId, DeviceKey, key_array};
use crate::messages::{self, DeviceInfo, PairAccepted, PairHello, PairReply, RelayAccess};
use crate::noise::{read, remote_static, write};
use crate::preface::{self, Preface};
use crate::{Error, MAX_NOISE_MESSAGE, base32};

const PARAMS: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
const PROLOGUE_LABEL: &[u8] = b"NiumaTerm pair";
const CLIENT_IDENTITY: &[u8] = b"niumaterm-pair-client";
const HOST_IDENTITY: &[u8] = b"niumaterm-pair-host";
const PAIR_HELLO_VERSION: u32 = 1;

pub const CODE_LEN: usize = 8;
pub const SLOT_LEN: usize = 3;
pub const CODE_LIFETIME_MS: u64 = 5 * 60 * 1000;
pub const MAX_FAILED_ATTEMPTS: u32 = 3;

/// An 8-symbol Crockford base32 code. The first 3 symbols are the rendezvous
/// slot (15 bits), the other 5 the secret (25 bits).
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode(String);

impl PairingCode {
    pub fn generate() -> Result<Self, Error> {
        let mut bytes = [0; 5];

        getrandom::fill(&mut bytes).map_err(Error::Random)?;

        Ok(Self(base32::encode(&bytes)))
    }

    /// Accept a code as a user types it: any case, with separators, and with
    /// `I`/`L` for `1` and `O` for `0`.
    pub fn parse(input: &str) -> Result<Self, Error> {
        let mut code = String::with_capacity(CODE_LEN);

        for c in input.chars().filter(|c| !matches!(c, '-' | ' ')) {
            code.push(base32::canonical(c).ok_or(Error::Malformed("pairing code symbol"))?);
        }

        if code.len() != CODE_LEN {
            return Err(Error::Malformed("pairing code length"));
        }

        Ok(Self(code))
    }

    pub fn slot(&self) -> &str {
        &self.0[..SLOT_LEN]
    }

    /// The 8 symbols without a separator, as carried in a pairing link.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Shown as `K7Q2-M9XD`.
impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", &self.0[..4], &self.0[4..])
    }
}

/// The code stays out of logs.
impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PairingCode({}-****)", self.slot())
    }
}

/// A code the host displays, with the limits that keep online guessing
/// negligible: a five minute lifetime, one successful use, and invalidation
/// after three failed attempts.
#[derive(Debug)]
pub struct IssuedCode {
    code: PairingCode,
    expires_at_ms: u64,
    failed_attempts: u32,
    used: bool,
}

impl IssuedCode {
    pub fn new(code: PairingCode, now_ms: u64) -> Self {
        Self {
            code,
            expires_at_ms: now_ms + CODE_LIFETIME_MS,
            failed_attempts: 0,
            used: false,
        }
    }

    pub fn code(&self) -> &PairingCode {
        &self.code
    }

    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }

    /// Whether a new pairing attempt may start with this code.
    pub fn is_usable(&self, now_ms: u64) -> bool {
        !self.used && self.failed_attempts < MAX_FAILED_ATTEMPTS && now_ms < self.expires_at_ms
    }

    /// Record a failed attempt; returns whether the code is still usable.
    pub fn record_failure(&mut self, now_ms: u64) -> bool {
        self.failed_attempts += 1;

        self.is_usable(now_ms)
    }

    pub fn record_success(&mut self) {
        self.used = true;
    }
}

/// Client side, before the host's SPAKE2 reply.
pub struct ClientPairing {
    spake: Spake2<Ed25519Group>,
    prologue: Vec<u8>,
    info: DeviceInfo,
    expected_host_key: Option<[u8; 32]>,
}

impl ClientPairing {
    /// Start pairing with `code`. `expected_host_key` comes from a pairing
    /// link; the handshake then fails unless the host proves that key.
    pub fn start(
        code: &PairingCode,
        offer: &Preface,
        answer: &Preface,
        info: DeviceInfo,
        expected_host_key: Option<[u8; 32]>,
    ) -> Result<(Self, Vec<u8>), Error> {
        let (spake, outbound) = Spake2::<Ed25519Group>::start_a(
            &Password::new(code.as_str()),
            &Identity::new(CLIENT_IDENTITY),
            &Identity::new(HOST_IDENTITY),
        );

        let hello = messages::encode(&PairHello {
            v: PAIR_HELLO_VERSION,
            slot: code.slot().to_owned(),
            spake: outbound,
        })?;

        let pairing = Self {
            spake,
            prologue: preface::prologue(PROLOGUE_LABEL, offer, answer),
            info,
            expected_host_key,
        };

        Ok((pairing, hello))
    }

    /// Consume the host's `PairReply` and produce Noise msg1.
    pub fn on_reply(
        self,
        key: &DeviceKey,
        reply: &[u8],
    ) -> Result<(ClientPairingHandshake, Vec<u8>), Error> {
        let reply: PairReply = messages::decode(reply)?;
        let psk = shared_key(self.spake.finish(&reply.spake))?;

        let mut noise = Builder::new(PARAMS.parse()?)
            .local_private_key(key.private())?
            .prologue(&self.prologue)?
            .psk(3, &psk)?
            .build_initiator()?;

        let msg1 = write(&mut noise, &[])?;

        let handshake = ClientPairingHandshake {
            noise,
            info: self.info,
            expected_host_key: self.expected_host_key,
        };

        Ok((handshake, msg1))
    }
}

/// Client side, during the Noise handshake.
pub struct ClientPairingHandshake {
    noise: HandshakeState,
    info: DeviceInfo,
    expected_host_key: Option<[u8; 32]>,
}

impl ClientPairingHandshake {
    /// Consume msg2 and produce msg3, which proves the code to the host and
    /// carries this device's description.
    pub fn on_msg2(mut self, msg2: &[u8]) -> Result<(ClientPairingConfirm, Vec<u8>), Error> {
        read(&mut self.noise, msg2)?;

        let host_key = remote_static(&self.noise)?;

        if self
            .expected_host_key
            .is_some_and(|expected| expected != host_key)
        {
            return Err(Error::HostKeyMismatch);
        }

        let msg3 = write(&mut self.noise, &messages::encode(&self.info)?)?;

        let confirm = ClientPairingConfirm {
            transport: self.noise.into_transport_mode()?,
            host_key,
        };

        Ok((confirm, msg3))
    }
}

/// Client side, waiting for the host to accept.
pub struct ClientPairingConfirm {
    transport: TransportState,
    host_key: [u8; 32],
}

impl ClientPairingConfirm {
    /// Consume the host's `PairAccepted`. A host that rejects the code closes
    /// the connection instead of answering.
    pub fn on_accepted(mut self, message: &[u8]) -> Result<PairedHost, Error> {
        let mut buf = vec![0; message.len()];

        let len = self.transport.read_message(message, &mut buf)?;

        Ok(PairedHost {
            public_key: self.host_key,
            accepted: messages::decode(&buf[..len])?,
        })
    }
}

/// What the client stores about a newly paired host.
#[derive(Debug)]
pub struct PairedHost {
    pub public_key: [u8; 32],
    pub accepted: PairAccepted,
}

/// Host side, after the client's `PairHello`.
pub struct HostPairing {
    noise: HandshakeState,
}

impl HostPairing {
    /// Answer a `PairHello` for the code this host is showing. Returns the
    /// `PairReply` to send. A hello for another slot is malformed here; the
    /// caller routes hellos by slot before calling.
    pub fn on_hello(
        code: &PairingCode,
        key: &DeviceKey,
        offer: &Preface,
        answer: &Preface,
        hello: &[u8],
    ) -> Result<(Self, Vec<u8>), Error> {
        let hello: PairHello = messages::decode(hello)?;

        if hello.v != PAIR_HELLO_VERSION || hello.slot != code.slot() {
            return Err(Error::Malformed("pair hello"));
        }

        let (spake, outbound) = Spake2::<Ed25519Group>::start_b(
            &Password::new(code.as_str()),
            &Identity::new(CLIENT_IDENTITY),
            &Identity::new(HOST_IDENTITY),
        );

        let psk = shared_key(spake.finish(&hello.spake))?;

        let noise = Builder::new(PARAMS.parse()?)
            .local_private_key(key.private())?
            .prologue(&preface::prologue(PROLOGUE_LABEL, offer, answer))?
            .psk(3, &psk)?
            .build_responder()?;

        let reply = messages::encode(&PairReply { spake: outbound })?;

        Ok((Self { noise }, reply))
    }

    /// Consume msg1 and produce msg2. msg2 reveals the host's public key but
    /// no host details: those wait until the client has proven the code.
    pub fn on_msg1(&mut self, msg1: &[u8]) -> Result<Vec<u8>, Error> {
        read(&mut self.noise, msg1)?;

        write(&mut self.noise, &[])
    }

    /// Consume msg3. [`Error::WrongCode`] means the client's key derivation
    /// disagreed with the host's, which the caller counts as a failed
    /// attempt.
    pub fn on_msg3(mut self, msg3: &[u8]) -> Result<PairingRequest, Error> {
        let payload = read(&mut self.noise, msg3).map_err(|error| match error {
            Error::Noise(_) => Error::WrongCode,
            other => other,
        })?;

        Ok(PairingRequest {
            client_key: remote_static(&self.noise)?,
            client: messages::decode(&payload)?,
            transport: self.noise.into_transport_mode()?,
        })
    }
}

/// A client that proved the code. The host stores its key, invalidates the
/// code, and answers with [`PairingRequest::accept`].
pub struct PairingRequest {
    pub client_key: [u8; 32],
    pub client: DeviceInfo,
    transport: TransportState,
}

impl PairingRequest {
    pub fn accept(mut self, accepted: &PairAccepted) -> Result<Vec<u8>, Error> {
        let payload = messages::encode(accepted)?;

        let mut out = vec![0; MAX_NOISE_MESSAGE];

        let len = self.transport.write_message(&payload, &mut out)?;

        out.truncate(len);

        Ok(out)
    }
}

/// `niumaterm://pair?...`: everything a phone needs to pair from a QR code.
/// It runs the same PAKE with the same code, and additionally pins the host
/// key and names the routes, so no slot lookup is needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingLink {
    pub code: PairingCode,
    pub host_id: DeviceId,
    pub host_key: [u8; 32],
    pub relay: Option<RelayAccess>,
    pub addresses: Vec<String>,
}

const LINK_VERSION: &str = "1";

impl PairingLink {
    pub fn to_url(&self) -> String {
        let mut url = Url::parse("niumaterm://pair").expect("static URL parses");

        {
            let mut query = url.query_pairs_mut();

            query
                .append_pair("v", LINK_VERSION)
                .append_pair("c", self.code.as_str())
                .append_pair("h", self.host_id.as_str())
                .append_pair("k", &URL_SAFE_NO_PAD.encode(self.host_key));

            if let Some(relay) = &self.relay {
                query
                    .append_pair("r", &relay.url)
                    .append_pair("rk", &relay.access_key);
            }

            if !self.addresses.is_empty() {
                query.append_pair("a", &self.addresses.join(","));
            }
        }

        url.into()
    }

    /// Parse a pasted or scanned link. The host id must be the one the key
    /// derives, so a link cannot pair with one host while naming another.
    pub fn parse(link: &str) -> Result<Self, Error> {
        let url = Url::parse(link.trim()).map_err(|_| Error::Malformed("pairing link"))?;

        if url.scheme() != "niumaterm" || url.host_str() != Some("pair") {
            return Err(Error::Malformed("pairing link"));
        }

        let param = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };

        if param("v").as_deref() != Some(LINK_VERSION) {
            return Err(Error::Malformed("pairing link version"));
        }

        let code = PairingCode::parse(&param("c").ok_or(Error::Malformed("pairing link code"))?)?;

        let host_key = URL_SAFE_NO_PAD
            .decode(param("k").ok_or(Error::Malformed("pairing link key"))?)
            .map_err(|_| Error::Malformed("pairing link key"))
            .and_then(|bytes| key_array(&bytes))?;

        let host_id = DeviceId::from_public_key(&host_key);

        if param("h").as_deref() != Some(host_id.as_str()) {
            return Err(Error::Malformed("pairing link host id"));
        }

        let relay = match (param("r"), param("rk")) {
            (Some(url), Some(access_key)) => Some(RelayAccess { url, access_key }),
            (None, None) => None,
            _ => return Err(Error::Malformed("pairing link relay")),
        };

        let addresses = param("a")
            .map(|list| list.split(',').map(str::to_owned).collect())
            .unwrap_or_default();

        Ok(Self {
            code,
            host_id,
            host_key,
            relay,
            addresses,
        })
    }
}

fn shared_key(finished: Result<Vec<u8>, spake2::Error>) -> Result<[u8; 32], Error> {
    let key = finished.map_err(|_| Error::Malformed("spake2 message"))?;

    key_array(&key)
}
