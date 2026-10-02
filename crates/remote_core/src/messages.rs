//! JSON payloads carried inside the pairing and channel handshakes. Unknown
//! fields are ignored and unknown enum values decode to `Unknown`, so a newer
//! peer can add fields and values without breaking an older one.

use serde::{Deserialize, Serialize};

use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Desktop,
    Mobile,
    #[serde(other)]
    Unknown,
}

/// How a device describes itself to a peer during pairing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub kind: DeviceKind,
    pub platform: String,
    pub app_version: String,
}

/// The longest device name, in characters, so a name fits one sidebar row.
/// The DNS-SD record cuts a name further to fit its 63-byte label.
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

/// A device name as it may be stored and shown: trimmed, with no control
/// characters, neither empty nor longer than [`MAX_DEVICE_NAME_CHARS`].
/// `None` rejects the input.
pub fn device_name(input: &str) -> Option<String> {
    let name = input.trim();

    let valid = !name.is_empty()
        && name.chars().count() <= MAX_DEVICE_NAME_CHARS
        && !name.chars().any(char::is_control);

    valid.then(|| name.to_owned())
}

/// Payload of the client's first channel handshake message. That message is
/// replayable and not forward secret, so it carries no application data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub proto_minor: u32,
    pub app_version: String,
    pub features: Vec<String>,

    /// Strictly increasing per client device. The host stores the last value
    /// it accepted from each device and drops a message that does not exceed
    /// it, which rejects a replayed first handshake message.
    pub hello_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostHello {
    pub proto_minor: u32,
    pub app_version: String,
    pub features: Vec<String>,
    pub name: String,

    /// LAN addresses, so a client that arrived through the relay can try the
    /// local network next time.
    pub lan_hints: Vec<String>,
}

/// Cleartext pairing opener. The slot routes the attempt to the host holding
/// the code; it is random data independent of the secret symbols.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PairHello {
    pub v: u32,
    pub slot: String,
    #[serde(with = "base64url")]
    pub spake: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PairReply {
    #[serde(with = "base64url")]
    pub spake: Vec<u8>,
}

/// The user's relay and the key that admits sockets to it. The key guards the
/// relay owner's quota only; channel confidentiality never depends on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayAccess {
    pub url: String,
    pub access_key: String,
}

/// Sent by the host, encrypted, once the client proved it holds the code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairAccepted {
    pub host: DeviceInfo,
    pub relay: Option<RelayAccess>,
    pub lan_hints: Vec<String>,
}

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    Ok(serde_json::to_vec(value)?)
}

pub(crate) fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, Error> {
    Ok(serde_json::from_slice(bytes)?)
}

mod base64url {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;

        URL_SAFE_NO_PAD.decode(text).map_err(D::Error::custom)
    }
}
