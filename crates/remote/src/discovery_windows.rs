//! DNS-SD through the Windows DNS Client service.
//!
//! The service reports instances as they answer but never reports one
//! leaving, so a browse runs in rounds: each round browses afresh and
//! resolves every instance it hears, and an instance absent from a whole
//! round counts as gone. Resolving again each round also picks up a changed
//! TXT record, such as a pairing slot coming or going.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;
use futures::StreamExt as _;
use futures::stream::FuturesUnordered;
use nmt_platform::runtime;
use nmt_platform::windows::dns_sd;
#[cfg(feature = "host")]
use nmt_platform::windows::dns_sd::{Registration, ServiceRecord, local_host_name};
#[cfg(feature = "host")]
use parking_lot::Mutex;
use tokio::select;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep, sleep_until};
use tracing::debug;

#[cfg(feature = "host")]
use crate::discovery::Publication;
use crate::discovery::{BrowseEvent, Resolved, SERVICE_TYPE};

/// How long a round listens for answers and waits for its resolves.
const ROUND_LENGTH: Duration = Duration::from_secs(3);

/// The pause between rounds. A host that left drops off the list within
/// one round and one pause.
const ROUND_PAUSE: Duration = Duration::from_secs(7);

/// Publishes one record at a time and withdraws it on drop.
#[cfg(feature = "host")]
pub(crate) struct Publisher {
    /// This computer's mDNS name. The DNS Client service answers address
    /// queries for it on every interface, which a single address passed
    /// with the record could not cover.
    host: String,

    current: Mutex<Option<Registration>>,
}

/// A running browse. Dropping it stops the rounds and closes its events.
pub(crate) struct Browse {
    task: AbortHandle,
}

/// One round's browse and the instance names it hears.
type RoundBrowse = (dns_sd::Browse, UnboundedReceiver<String>);

#[cfg(feature = "host")]
impl Publisher {
    pub(crate) fn start() -> Result<Self> {
        Ok(Self {
            host: local_host_name()?,
            current: Mutex::new(None),
        })
    }

    /// Publish `publication` in place of the previous record.
    pub(crate) fn publish(&self, publication: &Publication) -> Result<()> {
        let mut current = self.current.lock();

        // The previous record is withdrawn first so browsers never hold a
        // stale pairing slot.
        current.take();

        *current = Some(Registration::new(&ServiceRecord {
            name: &format!("{}.{}", publication.instance, local_type()),
            host: &self.host,
            port: publication.port,
            properties: publication.properties,
        })?);

        Ok(())
    }
}

impl Browse {
    pub(crate) fn start() -> Result<(Self, UnboundedReceiver<BrowseEvent>)> {
        // The first browse starts here so a machine without the service
        // reports that to the caller instead of failing every round.
        let first = dns_sd::Browse::start(&local_type())?;
        let (sender, receiver) = mpsc::unbounded_channel();

        let task = runtime().spawn(rounds(first, sender));

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
        // Aborting drops the round's browse and resolves, which cancels them.
        self.task.abort();
    }
}

/// Browse in rounds, starting with `first`, until `events` closes. Each
/// round reports what it finds and what the previous round had that this
/// one lacks.
async fn rounds(first: RoundBrowse, events: UnboundedSender<BrowseEvent>) {
    let mut present = HashSet::new();
    let mut next = Some(first);

    while !events.is_closed() {
        let browse = match next.take() {
            Some(browse) => browse,
            None => match dns_sd::Browse::start(&local_type()) {
                Ok(browse) => browse,
                Err(error) => {
                    // A round that could not start heard no instance, so
                    // treating its silence as departures would empty the
                    // list; the previous round's hosts stay present.
                    debug!(%error, "DNS-SD browse round failed");

                    sleep(ROUND_PAUSE).await;

                    continue;
                }
            },
        };

        let seen = round(browse, &events).await;

        for fullname in present.difference(&seen) {
            if events.send(BrowseEvent::Lost(fullname.clone())).is_err() {
                return;
            }
        }

        present = seen;

        sleep(ROUND_PAUSE).await;
    }
}

/// One browse round: every instance heard is resolved and reported as
/// found. Returns the full names heard.
async fn round(
    (_browse, mut names): RoundBrowse,
    events: &UnboundedSender<BrowseEvent>,
) -> HashSet<String> {
    let deadline = Instant::now() + ROUND_LENGTH;

    let mut seen = HashSet::new();
    let mut resolving = FuturesUnordered::new();

    loop {
        select! {
            Some(fullname) = names.recv() => {
                if seen.insert(fullname.clone()) {
                    resolving.push(resolve(fullname));
                }
            }

            Some(found) = resolving.next(), if !resolving.is_empty() => {
                if let Some(found) = found
                    && events.send(BrowseEvent::Found(found)).is_err()
                {
                    break;
                }
            }
            () = sleep_until(deadline) => break,
        }
    }

    seen
}

async fn resolve(fullname: String) -> Option<Resolved> {
    let instance = dns_sd::resolve(&fullname).await?;

    Some(Resolved {
        instance: instance_label(&fullname).to_owned(),
        fullname,
        properties: instance.properties,
        ipv4: instance.ipv4.into_iter().collect(),
        port: instance.port,
    })
}

/// The instance label of a full service name: the part before the service
/// type, which the service may return with a trailing root label and in
/// another case.
fn instance_label(fullname: &str) -> &str {
    let name = fullname.strip_suffix('.').unwrap_or(fullname);
    let suffix = format!(".{}", local_type());

    name.len()
        .checked_sub(suffix.len())
        .filter(|&start| {
            name.is_char_boundary(start) && name[start..].eq_ignore_ascii_case(&suffix)
        })
        .map_or(name, |start| &name[..start])
}

/// The service type in the local domain, the form the Windows service takes
/// and returns names in.
fn local_type() -> String {
    format!("{SERVICE_TYPE}.local")
}
