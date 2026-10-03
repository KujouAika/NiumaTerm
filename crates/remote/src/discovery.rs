//! LAN discovery through DNS-SD. iOS and Android browse DNS-SD natively,
//! whereas custom UDP broadcast needs a restricted multicast entitlement on
//! iOS, so the same records serve desktop and mobile clients.
//!
//! The record names the device and its id to the local network. Users on
//! untrusted networks can keep LAN hosting off.
//!
//! Windows and macOS publish and browse through the system DNS-SD service,
//! so this process opens no multicast socket there. A socket of our own on
//! 5353 raises the Windows firewall prompt the first time discovery runs,
//! even for a client that only looks for a paired host, and on macOS it
//! competes with mDNSResponder for the port. Other platforms run the
//! `mdns-sd` responder in process.

#[cfg(feature = "lan")]
use std::collections::BTreeMap;
#[cfg(feature = "lan")]
use std::net::Ipv4Addr;
use std::time::Duration;

#[cfg(feature = "lan")]
use anyhow::Result;
#[cfg(feature = "lan")]
use nmt_platform::runtime;
use nmt_remote_core::identity::DeviceId;
#[cfg(feature = "lan")]
use tokio::sync::mpsc::{self, UnboundedReceiver};
#[cfg(feature = "lan")]
use tokio::time::{Instant, timeout_at};
#[cfg(feature = "host")]
use tracing::warn;

#[cfg(all(feature = "lan", target_os = "macos"))]
use crate::discovery_macos::Browse;
#[cfg(all(feature = "host", target_os = "macos"))]
use crate::discovery_macos::Publisher;
#[cfg(all(feature = "lan", not(any(windows, target_os = "macos"))))]
use crate::discovery_mdns::Browse;
#[cfg(all(feature = "host", not(any(windows, target_os = "macos"))))]
use crate::discovery_mdns::Publisher;
#[cfg(all(feature = "lan", windows))]
use crate::discovery_windows::Browse;
#[cfg(all(feature = "host", windows))]
use crate::discovery_windows::Publisher;

/// The DNS-SD service type, without a domain.
#[cfg(feature = "lan")]
pub(crate) const SERVICE_TYPE: &str = "_niumaterm._tcp";

#[cfg(feature = "lan")]
const RECORD_VERSION: &str = "1";

/// The longest DNS label, which an instance name has to fit.
#[cfg(feature = "host")]
const MAX_LABEL_BYTES: usize = 63;

/// A host's DNS-SD record, kept current while it lives.
#[cfg(feature = "host")]
pub(crate) struct Advertiser {
    publisher: Publisher,
    name: String,
    id: DeviceId,
    port: u16,
}

/// A host announcing itself on the LAN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NearbyHost {
    /// The host's computer name.
    pub name: String,

    /// Its device id as the record states it. The record is unauthenticated,
    /// so the id only matches the host against pairing records for display.
    pub id: String,

    /// Where it listens, as `ip:port`.
    pub address: String,

    /// Whether it is showing a pairing code now.
    pub pairing: bool,
}

/// A running LAN browse for hosts. Dropping it stops the browse.
#[cfg(feature = "lan")]
pub struct Browser {
    _browse: Browse,
}

/// What a client looks for on the LAN.
pub enum Target<'a> {
    /// A paired host, by device id.
    Device(&'a DeviceId),
    /// A host showing a pairing code with this slot.
    PairingSlot(&'a str),
}

/// A service record as a backend resolved it.
#[cfg(feature = "lan")]
pub(crate) struct Resolved {
    /// The full instance name, which keys the record across updates.
    pub(crate) fullname: String,

    /// The instance label alone, without the service type and domain.
    pub(crate) instance: String,

    /// The TXT record as key and value pairs.
    pub(crate) properties: Vec<(String, String)>,

    pub(crate) ipv4: Vec<Ipv4Addr>,
    pub(crate) port: u16,
}

/// A change a browse reports.
#[cfg(feature = "lan")]
pub(crate) enum BrowseEvent {
    /// An instance appeared or its record changed.
    Found(Resolved),
    /// The instance with this full name left.
    Lost(String),
}

/// A record to publish for this host.
#[cfg(feature = "host")]
pub(crate) struct Publication<'a> {
    pub(crate) instance: &'a str,

    /// The device id, which names the record's host where the backend
    /// publishes its own address records instead of the system's.
    #[cfg(not(any(windows, target_os = "macos")))]
    pub(crate) id: &'a DeviceId,

    pub(crate) port: u16,
    pub(crate) properties: &'a [(&'a str, &'a str)],
}

#[cfg(feature = "host")]
impl Advertiser {
    pub(crate) fn start(name: &str, id: DeviceId, port: u16) -> Result<Self> {
        let suffix = format!(" {}", &id.as_str()[..4]);

        let advertiser = Self {
            publisher: Publisher::start()?,
            // Two installs can share a computer name; the id prefix keeps
            // their instance names apart. A DNS label holds 63 bytes, so a
            // long or non-ASCII name is cut on a character boundary to leave
            // the suffix room.
            name: format!(
                "{}{suffix}",
                truncate_bytes(name, MAX_LABEL_BYTES - suffix.len())
            ),
            id,
            port,
        };

        advertiser.publish(None)?;

        Ok(advertiser)
    }

    /// Add or drop the pairing slot from the record.
    pub(crate) fn set_pairing_slot(&self, slot: Option<&str>) {
        if let Err(error) = self.publish(slot) {
            warn!(%error, "failed to update the DNS-SD record");
        }
    }

    fn publish(&self, slot: Option<&str>) -> Result<()> {
        let mut properties = vec![("v", RECORD_VERSION), ("id", self.id.as_str())];

        if let Some(slot) = slot {
            properties.push(("pair", slot));
        }

        self.publisher.publish(&Publication {
            instance: &self.name,
            #[cfg(not(any(windows, target_os = "macos")))]
            id: &self.id,
            port: self.port,
            properties: &properties,
        })
    }
}

/// The longest prefix of `text` that fits `max` bytes without splitting a
/// character.
#[cfg(feature = "host")]
fn truncate_bytes(text: &str, max: usize) -> &str {
    let end = text
        .char_indices()
        .map(|(start, c)| start + c.len_utf8())
        .take_while(|&end| end <= max)
        .last()
        .unwrap_or(0);

    &text[..end]
}

#[cfg(feature = "lan")]
impl Resolved {
    pub(crate) fn property(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

#[cfg(feature = "lan")]
impl Browser {
    /// Browse until dropped. The receiver gets the full host list, in name
    /// order, each time a host appears, changes its record, or leaves.
    pub fn start() -> Result<(Self, UnboundedReceiver<Vec<NearbyHost>>)> {
        let (browse, mut events) = Browse::start()?;

        let (sender, receiver) = mpsc::unbounded_channel();

        runtime().spawn(async move {
            let mut hosts = BTreeMap::new();

            // The event channel closes when the browse stops on drop.
            while let Some(event) = events.recv().await {
                let changed = match event {
                    BrowseEvent::Found(service) => match nearby_host(&service) {
                        Some(host) => hosts.insert(service.fullname, host.clone()) != Some(host),
                        None => hosts.remove(&service.fullname).is_some(),
                    },
                    BrowseEvent::Lost(fullname) => hosts.remove(&fullname).is_some(),
                };

                if !changed {
                    continue;
                }

                let mut list: Vec<NearbyHost> = hosts.values().cloned().collect();

                // A restarted host can linger under an older, conflict-renamed
                // instance until its record expires; one row per device is
                // enough.
                list.sort_by(|a, b| a.id.cmp(&b.id));
                list.dedup_by(|a, b| a.id == b.id);
                list.sort_by(|a, b| a.name.cmp(&b.name));

                if sender.send(list).is_err() {
                    break;
                }
            }
        });

        Ok((Self { _browse: browse }, receiver))
    }
}

/// A resolved record as a listed host, or `None` for a record this version
/// cannot read or a host without an IPv4 address the listener accepts.
#[cfg(feature = "lan")]
fn nearby_host(service: &Resolved) -> Option<NearbyHost> {
    if service.property("v") != Some(RECORD_VERSION) {
        return None;
    }

    let id = service.property("id")?.to_owned();

    // Answers heard on the loopback interface carry 127.0.0.1, which names
    // this computer, not the host.
    let ip = service
        .ipv4
        .iter()
        .filter(|ip| !ip.is_loopback() && !ip.is_link_local())
        .min()?;

    // The advertiser appends an id prefix to keep instance names of two
    // installs on one computer apart, and DNS-SD conflict resolution can add
    // a " (2)" after that; the list shows the address instead of either.
    let name = id
        .get(..4)
        .and_then(|prefix| service.instance.rsplit_once(&format!(" {prefix}")))
        .map_or(service.instance.as_str(), |(name, _)| name)
        .to_owned();

    Some(NearbyHost {
        name,
        id,
        address: format!("{ip}:{}", service.port),
        pairing: service.property("pair").is_some(),
    })
}

/// Browse the LAN for up to `wait` and return the first matching host
/// address as `ip:port`.
#[cfg(feature = "lan")]
pub async fn find(target: Target<'_>, wait: Duration) -> Option<String> {
    let (_browse, mut events) = Browse::start().ok()?;

    let deadline = Instant::now() + wait;

    loop {
        let Ok(Some(event)) = timeout_at(deadline, events.recv()).await else {
            return None;
        };

        let BrowseEvent::Found(service) = event else {
            continue;
        };

        let matches = match &target {
            Target::Device(id) => service.property("id") == Some(id.as_str()),
            Target::PairingSlot(slot) => service.property("pair") == Some(*slot),
        };

        if !matches || service.property("v") != Some(RECORD_VERSION) {
            continue;
        }

        // IPv4 matches the listener; the first address is as good as any.
        if let Some(ip) = service.ipv4.first() {
            return Some(format!("{ip}:{}", service.port));
        }
    }
}

/// Without DNS-SD nothing on the LAN can be found, so a client falls back at
/// once to the addresses it was given and to the relay.
#[cfg(not(feature = "lan"))]
pub async fn find(_target: Target<'_>, _wait: Duration) -> Option<String> {
    None
}
