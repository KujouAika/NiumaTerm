//! DNS-SD through the in-process `mdns-sd` responder, for platforms without
//! a system DNS-SD service this crate binds.

use anyhow::Result;
#[cfg(feature = "host")]
use mdns_sd::ServiceInfo;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use nmt_platform::runtime;
#[cfg(feature = "host")]
use parking_lot::Mutex;
use tokio::sync::mpsc::{self, UnboundedReceiver};

#[cfg(feature = "host")]
use crate::discovery::Publication;
use crate::discovery::{BrowseEvent, Resolved, SERVICE_TYPE};

/// Publishes one record at a time and withdraws it on drop.
#[cfg(feature = "host")]
pub(crate) struct Publisher {
    daemon: ServiceDaemon,
    fullname: Mutex<Option<String>>,
}

/// A running browse. Dropping it stops the browse and closes its events.
pub(crate) struct Browse {
    daemon: ServiceDaemon,
}

#[cfg(feature = "host")]
impl Publisher {
    pub(crate) fn start() -> Result<Self> {
        Ok(Self {
            daemon: ServiceDaemon::new()?,
            fullname: Mutex::new(None),
        })
    }

    /// Publish `publication` in place of the previous record.
    pub(crate) fn publish(&self, publication: &Publication) -> Result<()> {
        let info = ServiceInfo::new(
            &format!("{SERVICE_TYPE}.local."),
            publication.instance,
            &format!("{}.local.", publication.id.as_str()),
            "",
            publication.port,
            publication.properties,
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

#[cfg(feature = "host")]
impl Drop for Publisher {
    fn drop(&mut self) {
        if let Some(fullname) = self.fullname.lock().take() {
            let _ = self.daemon.unregister(&fullname);
        }

        let _ = self.daemon.shutdown();
    }
}

impl Browse {
    pub(crate) fn start() -> Result<(Self, UnboundedReceiver<BrowseEvent>)> {
        let daemon = ServiceDaemon::new()?;
        let events = daemon.browse(&format!("{SERVICE_TYPE}.local."))?;
        let (sender, receiver) = mpsc::unbounded_channel();

        runtime().spawn(async move {
            // The event channel closes when the daemon shuts down on drop.
            while let Ok(event) = events.recv_async().await {
                let event = match event {
                    ServiceEvent::ServiceResolved(service) => {
                        BrowseEvent::Found(resolved(&service))
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => BrowseEvent::Lost(fullname),
                    _ => continue,
                };

                if sender.send(event).is_err() {
                    break;
                }
            }
        });

        Ok((Self { daemon }, receiver))
    }
}

impl Drop for Browse {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(&format!("{SERVICE_TYPE}.local."));
        let _ = self.daemon.shutdown();
    }
}

fn resolved(service: &ResolvedService) -> Resolved {
    let instance = service
        .fullname
        .strip_suffix(&service.ty_domain)
        .and_then(|name| name.strip_suffix('.'))
        .unwrap_or(&service.fullname)
        .to_owned();

    Resolved {
        fullname: service.fullname.clone(),
        instance,
        properties: service
            .txt_properties
            .iter()
            .map(|property| (property.key().to_owned(), property.val_str().to_owned()))
            .collect(),
        ipv4: service.get_addresses_v4().into_iter().collect(),
        port: service.port,
    }
}
