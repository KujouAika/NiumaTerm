use std::{fmt, str};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use snow::Builder;

use crate::{Error, base32};

/// Noise parameters used only to generate X25519 key pairs; every handshake
/// pattern in this crate uses the same curve.
const KEYGEN_PARAMS: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

/// One installation's static X25519 key pair. Paired peers store the public
/// half; the private half never leaves the device's secret storage.
pub struct DeviceKey {
    private: [u8; 32],
    public: [u8; 32],
}

impl DeviceKey {
    pub fn generate() -> Result<Self, Error> {
        let params = KEYGEN_PARAMS.parse()?;
        let pair = Builder::new(params).generate_keypair()?;

        Ok(Self {
            private: key_array(&pair.private)?,
            public: key_array(&pair.public)?,
        })
    }

    /// Rebuild a key pair loaded from secret storage. Both halves are stored
    /// because the Noise library does not derive a public key from a private
    /// one.
    pub fn from_parts(private: [u8; 32], public: [u8; 32]) -> Self {
        Self { private, public }
    }

    pub fn private(&self) -> &[u8; 32] {
        &self.private
    }

    pub fn public(&self) -> &[u8; 32] {
        &self.public
    }

    pub fn id(&self) -> DeviceId {
        DeviceId::from_public_key(&self.public)
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceKey")
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}

pub(crate) fn key_array(bytes: &[u8]) -> Result<[u8; 32], Error> {
    bytes.try_into().map_err(|_| Error::Malformed("key length"))
}

/// Short name of a device: the first 10 bytes of SHA-256 of its public key
/// in lowercase Crockford base32. It labels devices in the UI and logs and is
/// a host's relay routing key. 80 bits keep accidental collisions out of
/// reach; the id authenticates nothing, the key does.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn from_public_key(public: &[u8; 32]) -> Self {
        let digest = Sha256::digest(public);

        Self(base32::encode(&digest[..10]).to_ascii_lowercase())
    }

    /// The 16-character form used in URLs, records, and relay routes.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Grouped for reading aloud: `abcd-efgh-jkmn-pqrs`.
impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, group) in self.0.as_bytes().chunks(4).enumerate() {
            if index > 0 {
                f.write_str("-")?;
            }

            f.write_str(str::from_utf8(group).map_err(|_| fmt::Error)?)?;
        }

        Ok(())
    }
}
