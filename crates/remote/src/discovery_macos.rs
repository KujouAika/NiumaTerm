//! DNS-SD through the system mDNSResponder (Bonjour).
//!
//! A browse reports each instance once per interface it answers on, and
//! reports it leaving per interface too, so an instance counts as gone only
//! once every interface has dropped it. Each present instance keeps a
//! resolve running, which reports its record again whenever the TXT record
//! changes, such as a pairing slot coming or going.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::Result;
use nmt_platform::macos::dns_sd::{self, BrowseReply, Query};
#[cfg(feature = "host")]
use nmt_platform::macos::dns_sd::{Registration, ServiceRecord};
use nmt_platform::runtime;
#[cfg(feature = "host")]
use parking_lot::Mutex;
use tokio::select;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::AbortHandle;
use tokio::time::timeout;
use tracing::debug;

#[cfg(feature = "host")]
use crate::discovery::Publication;
use crate::discovery::{BrowseEvent, Resolved, SERVICE_TYPE};

/// How long a resolved host's address lookup may take before the record is
/// reported without addresses.
const ADDRESS_WAIT: Duration = Duration::from_secs(2);

/// Publishes one record and keeps its properties current; dropping it
/// withdraws the record.
#[cfg(feature = "host")]
pub(crate) struct Publisher {
    current: Mutex<Option<Registration>>,
}

/// A running browse. Dropping it stops the browse and every resolve it
/// started, and closes its events.
pub(crate) struct Browse {
    task: AbortHandle,
}

/// An instance the browse has heard and the resolve that follows it.
struct Present {
    interfaces: HashSet<u32>,
    _follow: Follow,
}

/// A task forwarding one instance's resolved records; dropping it stops the
/// resolve.
struct Follow(AbortHandle);

#[cfg(feature = "host")]
impl Publisher {
    pub(crate) fn start() -> Result<Self> {
        Ok(Self {
            current: Mutex::new(None),
        })
    }

    /// Publish `publication`. An advertiser only ever changes the TXT
    /// properties of its record, so once published the record is updated in
    /// place: browsers see the change at once, and the instance keeps its
    /// name instead of leaving and registering again.
    pub(crate) fn publish(&self, publication: &Publication) -> Result<()> {
        let mut current = self.current.lock();

        if let Some(registration) = current.as_ref() {
            return registration.set_properties(publication.properties);
        }

        *current = Some(Registration::new(&ServiceRecord {
            name: publication.instance,
            regtype: SERVICE_TYPE,
            port: publication.port,
            properties: publication.properties,
        })?);

        Ok(())
    }
}

impl Browse {
    pub(crate) fn start() -> Result<(Self, UnboundedReceiver<BrowseEvent>)> {
        let (browse, replies) = dns_sd::browse(SERVICE_TYPE)?;
        let (sender, receiver) = mpsc::unbounded_channel();

        let task = runtime().spawn(track(browse, replies, sender));

        Ok((
            Self {
                task: task.abort_handle(),
            },
            receiver,
        ))
    }
}

impl Drop for Browse {
    fn drop(&mut self) {
        // Aborting drops the browse and the instances' follow tasks.
        self.task.abort();
    }
}

impl Drop for Follow {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Turn browse replies into events: start following an instance when it
/// first appears, and report it lost when its last interface drops it.
async fn track(
    _browse: Query,
    mut replies: UnboundedReceiver<BrowseReply>,
    events: UnboundedSender<BrowseEvent>,
) {
    let mut present: HashMap<String, Present> = HashMap::new();

    // Follow tasks report here instead of to `events`, so a record that a
    // stopped follow task sent late is dropped once its instance is gone.
    let (found_sender, mut found) = mpsc::unbounded_channel::<Resolved>();

    loop {
        select! {
            reply = replies.recv() => {
                let Some(reply) = reply else {
                    return;
                };

                let fullname = format!("{}.{}{}", reply.name, reply.regtype, reply.domain);

                if reply.added {
                    match present.get_mut(&fullname) {
                        Some(instance) => {
                            instance.interfaces.insert(reply.interface);
                        }
                        None => {
                            let Some(follow) = follow(&reply, fullname.clone(), found_sender.clone())
                            else {
                                continue;
                            };

                            present.insert(fullname, Present {
                                interfaces: HashSet::from([reply.interface]),
                                _follow: follow,
                            });
                        }
                    }
                } else if let Some(instance) = present.get_mut(&fullname) {
                    instance.interfaces.remove(&reply.interface);

                    if instance.interfaces.is_empty() {
                        present.remove(&fullname);

                        if events.send(BrowseEvent::Lost(fullname)).is_err() {
                            return;
                        }
                    }
                }
            }

            Some(resolved) = found.recv() => {
                if present.contains_key(&resolved.fullname)
                    && events.send(BrowseEvent::Found(resolved)).is_err()
                {
                    return;
                }
            }
        }
    }
}

/// Resolve the instance `reply` names and forward each record it resolves
/// to, with the host's IPv4 addresses, until the returned task is dropped.
fn follow(
    reply: &BrowseReply,
    fullname: String,
    found: UnboundedSender<Resolved>,
) -> Option<Follow> {
    let (resolve, mut records) =
        dns_sd::resolve(&reply.name, &reply.regtype, &reply.domain, reply.interface)
            .inspect_err(|error| debug!(%error, "cannot resolve a DNS-SD instance"))
            .ok()?;

    let instance = reply.name.clone();

    let task = runtime().spawn(async move {
        let _resolve = resolve;

        while let Some(record) = records.recv().await {
            let ipv4 = timeout(ADDRESS_WAIT, dns_sd::ipv4_addresses(&record.host))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();

            let resolved = Resolved {
                fullname: fullname.clone(),
                instance: instance.clone(),
                properties: record.properties,
                ipv4,
                port: record.port,
            };

            if found.send(resolved).is_err() {
                return;
            }
        }
    });

    Some(Follow(task.abort_handle()))
}
