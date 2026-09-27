//! LAN discovery through DNS-SD. iOS and Android browse DNS-SD natively,
//! whereas custom UDP broadcast needs a restricted multicast entitlement on
//! iOS, so the same records serve desktop and mobile clients.
//!
//! The record names the device and its id to the local network. Users on
//! untrusted networks can keep LAN hosting off.

use std::time::Duration;

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use nmt_remote_core::identity::DeviceId;
use parking_lot::Mutex;
use tokio::time::{Instant, timeout_at};
use tracing::warn;

const SERVICE_TYPE: &str = "_niumaterm._tcp.local.";
const RECORD_VERSION: &str = "1";

/// A host's DNS-SD record, kept current while it lives.
pub(crate) struct Advertiser {
    daemon: ServiceDaemon,
    name: String,
    id: DeviceId,
    port: u16,
    fullname: Mutex<Option<String>>,
}

/// What a client looks for on the LAN.
pub enum Target<'a> {
    /// A paired host, by device id.
    Device(&'a DeviceId),
    /// A host showing a pairing code with this slot.
    PairingSlot(&'a str),
}

impl Advertiser {
    pub(crate) fn start(name: &str, id: DeviceId, port: u16) -> Result<Self> {
        let advertiser = Self {
            daemon: ServiceDaemon::new()?,
            // Two installs can share a computer name; the id prefix keeps
            // their instance names apart.
            name: format!("{name} {}", &id.as_str()[..4]),
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

impl Drop for Advertiser {
    fn drop(&mut self) {
        if let Some(fullname) = self.fullname.lock().take() {
            let _ = self.daemon.unregister(&fullname);
        }

        let _ = self.daemon.shutdown();
    }
}

/// Browse the LAN for up to `wait` and return the first matching host
/// address as `ip:port`.
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
