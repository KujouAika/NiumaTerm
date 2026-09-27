//! Pairing records live as JSON in the application state directory rather
//! than in `config.toml`: pairing creates them, nobody edits them by hand.
//! Public keys are not secret, and anyone able to rewrite these files already
//! runs as the user. The private key is the one secret and goes through
//! [`crate::secret`].

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use nmt_config::config_dir_path;
use nmt_platform::durable_file;
use nmt_remote_core::identity::{DeviceId, DeviceKey};
use nmt_remote_core::messages::DeviceKind;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::secret;

const SCHEMA: u32 = 1;
const IDENTITY_FILE: &str = "identity.key";
const DEVICES_FILE: &str = "devices.json";
const HOSTS_FILE: &str = "hosts.json";

/// The remote-session state directory. Testing instances resolve their own
/// configuration directory, so they get their own identity and records.
pub fn remote_dir() -> PathBuf {
    config_dir_path().join("remote")
}

/// Host side: one device allowed to connect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDevice {
    pub schema: u32,
    pub id: DeviceId,
    pub name: String,
    pub kind: DeviceKind,
    pub platform: String,
    #[serde(with = "key_base64")]
    pub public_key: [u8; 32],

    /// Every paired device is a full operator today. The field exists so a
    /// read-only role needs no migration.
    pub role: String,

    pub paired_at: u64,
    pub last_seen: u64,

    /// The last `hello_ms` accepted from this device; a first handshake
    /// message that does not exceed it is a replay.
    pub last_hello_ms: u64,
}

/// Client side: one host this device may open sessions on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedHost {
    pub schema: u32,
    pub id: DeviceId,
    pub name: String,
    #[serde(with = "key_base64")]
    pub public_key: [u8; 32],
    pub lan_hints: Vec<String>,
    pub paired_at: u64,
    pub last_seen: u64,

    /// The last `hello_ms` sent to this host. The next one exceeds it even
    /// when the clock stepped backwards, so the host never mistakes it for a
    /// replay.
    pub last_hello_ms: u64,
}

impl PairedDevice {
    pub(crate) fn new(
        public_key: [u8; 32],
        name: String,
        kind: DeviceKind,
        platform: String,
    ) -> Self {
        let now = now_ms();

        Self {
            schema: SCHEMA,
            id: DeviceId::from_public_key(&public_key),
            name,
            kind,
            platform,
            public_key,
            role: "operator".into(),
            paired_at: now,
            last_seen: now,
            last_hello_ms: 0,
        }
    }
}

impl PairedHost {
    pub(crate) fn new(public_key: [u8; 32], name: String, address: String) -> Self {
        let now = now_ms();

        Self {
            schema: SCHEMA,
            id: DeviceId::from_public_key(&public_key),
            name,
            public_key,
            lan_hints: vec![address],
            paired_at: now,
            last_seen: now,
            last_hello_ms: 0,
        }
    }

    /// The `hello_ms` for the next connection, recorded as sent.
    pub(crate) fn next_hello_ms(&mut self) -> u64 {
        self.last_hello_ms = now_ms().max(self.last_hello_ms + 1);

        self.last_hello_ms
    }
}

/// Load this installation's key pair, creating and storing one on first use.
pub fn load_or_create_identity(dir: &Path) -> Result<DeviceKey> {
    let path = dir.join(IDENTITY_FILE);

    match fs::read(&path) {
        Ok(sealed) => {
            let bytes = secret::unprotect(&sealed).context("unsealing the device key")?;

            let (private, public) = bytes
                .split_first_chunk::<32>()
                .and_then(|(private, rest)| Some((*private, *rest.first_chunk::<32>()?)))
                .context("device key file is truncated")?;

            Ok(DeviceKey::from_parts(private, public))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let key = DeviceKey::generate()?;
            let sealed = secret::protect(&[*key.private(), *key.public()].concat())?;

            fs::create_dir_all(dir)?;
            durable_file::write(&path, &sealed)?;

            Ok(key)
        }
        Err(error) => Err(error).context("reading the device key"),
    }
}

pub fn load_devices(dir: &Path) -> Vec<PairedDevice> {
    load_list(&dir.join(DEVICES_FILE))
}

pub fn save_devices(dir: &Path, devices: &[PairedDevice]) -> io::Result<()> {
    save_list(dir, DEVICES_FILE, devices)
}

pub fn load_hosts(dir: &Path) -> Vec<PairedHost> {
    load_list(&dir.join(HOSTS_FILE))
}

pub fn save_hosts(dir: &Path, hosts: &[PairedHost]) -> io::Result<()> {
    save_list(dir, HOSTS_FILE, hosts)
}

/// A missing or unreadable list is empty: losing it means pairing again,
/// never running with a partially trusted set.
fn load_list<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };

    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        warn!(path = %path.display(), %error, "ignoring unreadable pairing records");

        Vec::new()
    })
}

fn save_list<T: Serialize>(dir: &Path, name: &str, list: &[T]) -> io::Result<()> {
    fs::create_dir_all(dir)?;

    let json = serde_json::to_vec_pretty(list).map_err(io::Error::other)?;

    durable_file::write(&dir.join(name), &json)
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

mod key_base64 {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(key: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(key))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = STANDARD.decode(text).map_err(D::Error::custom)?;

        bytes
            .try_into()
            .map_err(|_| D::Error::custom("public key must be 32 bytes"))
    }
}
