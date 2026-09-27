//! Remote session protocol core: device identity, pairing, the encrypted
//! channel, and stream framing.
//!
//! The crate performs no I/O and starts no runtime. Callers move bytes between
//! these state machines and a transport, which keeps the cryptography and
//! codecs identical on desktop hosts, desktop clients, and mobile apps that
//! link this crate.

pub mod channel;
pub mod frame;
pub mod identity;
pub mod messages;
pub mod pairing;
pub mod preface;

mod base32;
mod noise;

#[cfg(test)]
mod channel_tests;
#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod pairing_tests;
#[cfg(test)]
mod preface_tests;

/// Highest channel protocol major this build speaks. A major changes only for
/// changes an older peer cannot skip: frame layout, handshake, cryptography.
pub const PROTO_MAJOR: u8 = 1;

/// Lowest channel protocol major this build still accepts, kept one release
/// behind a new major so hosts and clients can update in either order.
pub const PROTO_MIN_MAJOR: u8 = 1;

/// Additive protocol revision. Both sides use the lower of the two values.
pub const PROTO_MINOR: u32 = 0;

/// Pairing exchange major, versioned apart from the channel.
pub const PAIR_MAJOR: u8 = 1;

/// Largest Noise message, and therefore the largest transport message.
pub const MAX_NOISE_MESSAGE: usize = 65535;

/// ChaCha20-Poly1305 authentication tag appended to every sealed message.
pub const TAG_LEN: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A handshake or transport message failed to decrypt or authenticate:
    /// tampering, replay, reordering, a wrong peer key, or a wrong pairing
    /// code on the side that cannot tell them apart.
    #[error("noise: {0}")]
    Noise(#[from] snow::Error),
    #[error("malformed {0}")]
    Malformed(&'static str),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// The pairing code did not match the host's. Counts as one attempt.
    #[error("wrong pairing code")]
    WrongCode,
    /// The host presented a key other than the one a pairing link names.
    #[error("host key does not match the pairing link")]
    HostKeyMismatch,
    #[error("message exceeds {limit} bytes")]
    TooLarge { limit: usize },
    #[error("random source failed: {0}")]
    Random(getrandom::Error),
}
