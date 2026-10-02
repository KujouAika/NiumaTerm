//! LAN discovery through DNS-SD. iOS and Android browse DNS-SD natively,
//! whereas custom UDP broadcast needs a restricted multicast entitlement on
//! iOS, so the same records serve desktop and mobile clients.
//!
//! The record names the device and its id to the local network. Users on
//! untrusted networks can keep LAN hosting off.

#[cfg(feature = "lan")]
use std::collections::BTreeMap;
use std::time::Duration;

#[cfg(feature = "lan")]
use anyhow::Result;
#[cfg(feature = "host")]
use mdns_sd::ServiceInfo;
#[cfg(feature = "lan")]
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
#[cfg(feature = "lan")]
use nmt_platform::runtime;
use nmt_remote_core::identity::DeviceId;
#[cfg(feature = "host")]
use parking_lot::Mutex;
#[cfg(feature = "lan")]
use tokio::sync::mpsc::{self, UnboundedReceiver};
#[cfg(feature = "lan")]
use tokio::time::{Instant, timeout_at};
#[cfg(feature = "host")]
use tracing::warn;

#[cfg(feature = "lan")]
const SERVICE_TYPE: &str = "_niumaterm._tcp.local.";

#[cfg(feature = "lan")]
const RECORD_VERSION: &str = "1";

/// The longest DNS label, which an instance name has to fit.
#[cfg(feature = "host")]
const MAX_LABEL_BYTES: usize = 63;

/// A host's DNS-SD record, kept current while it lives.
#[cfg(feature = "host")]
pub(crate) struct Advertiser {
    daemon: ServiceDaemon,
    name: String,
    id: DeviceId,
    port: u16,
    fullname: Mutex<Option<String>>,
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
    daemon: ServiceDaemon,
}

/// What a client looks for on the LAN.
pub enum Target<'a> {
    /// A paired host, by device id.
    Device(&'a DeviceId),
    /// A host showing a pairing code with this slot.
    PairingSlot(&'a str),
}

#[cfg(feature = "host")]
impl Advertiser {
    pub(crate) fn start(name: &str, id: DeviceId, port: u16) -> Result<Self> {
        let suffix = format!(" {}", &id.as_str()[..4]);

        let advertiser = Self {
            daemon: ServiceDaemon::new()?,
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
            fullname: Mutex::new(None),
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

        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &self.name,
            &format!("{}.local.", self.id.as_str()),
            "",
            self.port,
            &properties[..],
        )?
        .enable_addr_auto();

        let mut fullname = self.fullname.lock();

        // Re-registering the same name with a new TXT set announces the
        // change; the previous record is withdrawn first so browsers never
        // hold a stale slot.
        if let Some(previous) = fullname.take() {
            let _ = self.daemon.unregister(&previous);
        }

        *fullname = Some(info.get_fullname().to_owned());

        self.daemon.register(info)?;

        Ok(())
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

#[cfg(feature = "host")]
impl Drop for Advertiser {
    fn drop(&mut self) {
        if let Some(fullname) = self.fullname.lock().take() {
            let _ = self.daemon.unregister(&fullname);
        }

        let _ = self.daemon.shutdown();
    }
}

#[cfg(feature = "lan")]
impl Browser {
    /// Browse until dropped. The receiver gets the full host list, in name
    /// order, each time a host appears, changes its record, or leaves.
    pub fn start() -> Result<(Self, UnboundedReceiver<Vec<NearbyHost>>)> {
        let daemon = ServiceDaemon::new()?;
        let events = daemon.browse(SERVICE_TYPE)?;
        let (sender, receiver) = mpsc::unbounded_channel();

        runtime().spawn(async move {
            let mut hosts = BTreeMap::new();

            // The event channel closes when the daemon shuts down on drop.
            while let Ok(event) = events.recv_async().await {
                let changed = match event {
                    ServiceEvent::ServiceResolved(service) => match nearby_host(&service) {
                        Some(host) => {
                            hosts.insert(service.fullname.clone(), host.clone()) != Some(host)
                        }
                        None => hosts.remove(&service.fullname).is_some(),
                    },
                    ServiceEvent::ServiceRemoved(_, fullname) => hosts.remove(&fullname).is_some(),
                    _ => false,
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

        Ok((Self { daemon }, receiver))
    }
}

#[cfg(feature = "lan")]
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        let _ = self.daemon.shutdown();
    }
}

/// A resolved record as a listed host, or `None` for a record this version
/// cannot read or a host without an IPv4 address the listener accepts.
#[cfg(feature = "lan")]
fn nearby_host(service: &ResolvedService) -> Option<NearbyHost> {
    if service.get_property_val_str("v") != Some(RECORD_VERSION) {
        return None;
    }

    let id = service.get_property_val_str("id")?.to_owned();

    // Answers heard on the loopback interface carry 127.0.0.1, which names
    // this computer rather than the host.
    let ip = service
        .get_addresses_v4()
        .into_iter()
        .filter(|ip| !ip.is_loopback() && !ip.is_link_local())
        .min()?;

    let instance = service
        .fullname
        .strip_suffix(&service.ty_domain)
        .and_then(|name| name.strip_suffix('.'))
        .unwrap_or(&service.fullname);

    // The advertiser appends an id prefix to keep instance names of two
    // installs on one computer apart, and DNS-SD conflict resolution can add
    // a " (2)" after that; the list shows the address instead of either.
    let name = id
        .get(..4)
        .and_then(|prefix| instance.rsplit_once(&format!(" {prefix}")))
        .map_or(instance, |(name, _)| name)
        .to_owned();

    Some(NearbyHost {
        name,
        id,
        address: format!("{ip}:{}", service.get_port()),
        pairing: service.get_property_val_str("pair").is_some(),
    })
}

/// Browse the LAN for up to `wait` and return the first matching host
/// address as `ip:port`.
#[cfg(feature = "lan")]
pub async fn find(target: Target<'_>, wait: Duration) -> Option<String> {
    let daemon = ServiceDaemon::new().ok()?;
    let events = daemon.browse(SERVICE_TYPE).ok()?;
    let deadline = Instant::now() + wait;

    let found = loop {
        let Ok(Ok(event)) = timeout_at(deadline, events.recv_async()).await else {
            break None;
        };

        let ServiceEvent::ServiceResolved(service) = event else {
            continue;
        };

        let matches = match &target {
            Target::Device(id) => service.get_property_val_str("id") == Some(id.as_str()),
            Target::PairingSlot(slot) => service.get_property_val_str("pair") == Some(*slot),
        };

        if !matches || service.get_property_val_str("v") != Some(RECORD_VERSION) {
            continue;
        }

        // IPv4 matches the listener; the first address is as good as any.
        if let Some(ip) = service.get_addresses_v4().into_iter().next() {
            break Some(format!("{ip}:{}", service.get_port()));
        }
    };

    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();

    found
}

/// Without DNS-SD nothing on the LAN can be found, so a client falls back at
/// once to the addresses it was given and to the relay.
#[cfg(not(feature = "lan"))]
pub async fn find(_target: Target<'_>, _wait: Duration) -> Option<String> {
    None
}
