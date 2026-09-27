use serde::{Deserialize, Serialize};

/// Hosting sessions for paired devices. Connecting to other computers needs
/// no setting: it is available whenever a host is paired.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
pub struct RemoteConfig {
    /// Accept connections from paired devices on the local network.
    pub enabled: bool,

    pub lan_port: u16,

    /// The name peers see; empty uses the computer name.
    pub device_name: String,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lan_port: 47470,
            device_name: String::new(),
        }
    }
}
