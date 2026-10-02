//! Push notifications to a paired phone.
//!
//! The phone registers with each host over the encrypted channel: where to
//! send pushes, its APNs device token, and a random key it keeps for that
//! host. The host seals each notification's text under that key, so the
//! relay that forwards it and Apple, which delivers it, see only ciphertext;
//! the phone's notification extension opens it.
//!
//! Sealed form: `base64(nonce || ciphertext || tag)` with ChaCha20-Poly1305,
//! a random 96-bit nonce, and the host id as associated data, so a sealed
//! body cannot be passed off under another host's name. It is exactly what
//! CryptoKit's `ChaChaPoly.SealedBox(combined:)` reads.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};

use crate::Error;

/// Client to host: send this device pushes while it is away from the host.
/// Replaces an earlier registration. Answers `{}`.
pub const PUSH_REGISTER: &str = "push.register";

/// Client to host: stop sending this device pushes. Answers `{}`.
pub const PUSH_UNREGISTER: &str = "push.unregister";

/// The feature a host lists in its hello when it takes registrations.
pub const PUSH_FEATURE: &str = "push";

/// Longest title and body a push carries. Lock-screen banners show a line
/// or two, and the bound keeps a sealed push well inside the forwarder's
/// 3 KB limit, which in turn keeps it inside the 4 KB APNs payload.
pub const MAX_TITLE_CHARS: usize = 120;

pub const MAX_BODY_CHARS: usize = 180;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    TurnFinished,
    TurnFailed,
    Approval,
    Question,
    #[serde(other)]
    Unknown,
}

/// Which APNs environment issued the token: development builds get sandbox
/// tokens, TestFlight and App Store builds production ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushEnvironment {
    Sandbox,
    Production,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushRegistration {
    /// The forwarder's URL, which holds the APNs key for the app build.
    pub endpoint: String,

    /// The APNs device token, hex.
    pub token: String,

    pub environment: PushEnvironment,

    /// The 32-byte key this device opens this host's pushes with, base64.
    pub key: String,

    /// The events the person wants to hear about.
    pub kinds: Vec<PushKind>,
}

/// What a push says, before sealing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushMessage {
    pub v: u32,
    pub host: String,
    pub session: String,
    pub kind: PushKind,
    pub title: String,
    pub body: String,

    /// When it happened, in Unix milliseconds.
    pub at: u64,
}

impl PushMessage {
    /// A message with title and body cut to what a push carries.
    pub fn new(
        host: &str,
        session: &str,
        kind: PushKind,
        title: &str,
        body: &str,
        at: u64,
    ) -> Self {
        Self {
            v: 1,
            host: host.to_owned(),
            session: session.to_owned(),
            kind,
            title: shorten(title, MAX_TITLE_CHARS),
            body: shorten(body, MAX_BODY_CHARS),
            at,
        }
    }
}

/// Seal `message` for the device that registered `key`, under a fresh random
/// nonce.
pub fn seal(key: &str, message: &PushMessage) -> Result<String, Error> {
    let mut nonce = [0u8; 12];

    getrandom::fill(&mut nonce).map_err(Error::Random)?;

    seal_with_nonce(key, message, nonce)
}

/// [`seal`] with a given nonce, for test vectors the phone checks too.
pub(crate) fn seal_with_nonce(
    key: &str,
    message: &PushMessage,
    nonce: [u8; 12],
) -> Result<String, Error> {
    let key = decode_key(key)?;
    let plaintext = serde_json::to_vec(message)?;

    let ciphertext = ChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: message.host.as_bytes(),
            },
        )
        .map_err(|_| Error::Malformed("push message"))?;

    let mut sealed = nonce.to_vec();

    sealed.extend_from_slice(&ciphertext);

    Ok(STANDARD.encode(sealed))
}

/// Open what [`seal`] produced for `host`. The phone does this in Swift; this
/// is for the host's own tests.
pub fn open(key: &str, host: &str, sealed: &str) -> Result<PushMessage, Error> {
    let key = decode_key(key)?;

    let sealed = STANDARD
        .decode(sealed)
        .map_err(|_| Error::Malformed("sealed push"))?;

    if sealed.len() < 12 {
        return Err(Error::Malformed("sealed push"));
    }

    let (nonce, ciphertext) = sealed.split_at(12);

    let plaintext = ChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: host.as_bytes(),
            },
        )
        .map_err(|_| Error::Malformed("sealed push"))?;

    Ok(serde_json::from_slice(&plaintext)?)
}

fn decode_key(key: &str) -> Result<[u8; 32], Error> {
    STANDARD
        .decode(key)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Error::Malformed("push key"))
}

fn shorten(text: &str, limit: usize) -> String {
    let text = text.trim();

    match text.char_indices().nth(limit) {
        Some((cut, _)) => format!("{}…", text[..cut].trim_end()),
        None => text.to_owned(),
    }
}
