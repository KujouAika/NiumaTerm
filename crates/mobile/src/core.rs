use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use nmt_platform::runtime;
use nmt_remote::client;
use nmt_remote::connection::{RemoteHost, Status};
use nmt_remote::store::{PairedHost, load_hosts, load_or_create_identity, save_hosts};
use nmt_remote_core::identity::DeviceKey;
use nmt_remote_core::messages::{DeviceInfo, DeviceKind, RelayAccess};
use nmt_remote_core::pairing::{PairingCode, PairingLink};
use parking_lot::Mutex;
use tokio::select;
use tokio::task::AbortHandle;
use tracing::{debug, warn};

use crate::agent::{AgentHandle, AgentObserver};
use crate::error::CoreError;
use crate::records::{HostOffer, HostRecord, SessionRecord};
use crate::terminal::{TerminalHandle, TerminalObserver};

/// Told about hosts and their sessions. Called on the core's threads; the
/// app moves to its main thread itself.
#[uniffi::export(with_foreign)]
pub trait CoreObserver: Send + Sync {
    fn host_changed(&self, host: HostRecord);

    /// The host's whole session list, after it changed.
    fn sessions_changed(&self, host: String, sessions: Vec<SessionRecord>);
}

/// The paired hosts of this device and a lasting connection to each.
#[derive(uniffi::Object)]
pub struct MobileCore {
    dir: PathBuf,
    key: Arc<DeviceKey>,
    device: DeviceInfo,

    /// The stored records, which each host's connection updates after it
    /// connects (address, name, handshake time).
    records: Arc<Mutex<Vec<PairedHost>>>,

    hosts: Mutex<Vec<Paired>>,
    observer: Mutex<Option<Arc<dyn CoreObserver>>>,

    /// While the app is in the foreground every host stays connected, so
    /// session lists stay live.
    foreground: AtomicBool,
}

struct Paired {
    remote: Arc<RemoteHost>,
    watcher: Option<AbortHandle>,
}

#[uniffi::export]
impl MobileCore {
    /// Open the core on `state_dir`, where pairing records live. The device
    /// key is created on first use and sealed with a Keychain-held key.
    #[uniffi::constructor]
    pub fn new(
        state_dir: String,
        device_name: String,
        app_version: String,
    ) -> Result<Arc<Self>, CoreError> {
        let dir = PathBuf::from(state_dir);
        let key = Arc::new(load_or_create_identity(&dir)?);
        let records = load_hosts(&dir);

        let core = Arc::new(Self {
            dir,
            key,
            device: DeviceInfo {
                name: device_name,
                kind: DeviceKind::Mobile,
                platform: "ios".into(),
                app_version,
            },
            records: Arc::new(Mutex::new(records.clone())),
            hosts: Mutex::new(Vec::new()),
            observer: Mutex::new(None),
            foreground: AtomicBool::new(false),
        });

        for record in records {
            core.add_host(record);
        }

        Ok(core)
    }

    pub fn hosts(&self) -> Vec<HostRecord> {
        self.hosts
            .lock()
            .iter()
            .map(|paired| host_record(&paired.remote, *paired.remote.status().borrow()))
            .collect()
    }

    /// Report host and session changes to `observer` from now on, starting
    /// with the current state of every host.
    pub fn observe(&self, observer: Arc<dyn CoreObserver>) {
        *self.observer.lock() = Some(Arc::clone(&observer));

        for paired in self.hosts.lock().iter_mut() {
            if let Some(watcher) = paired.watcher.take() {
                watcher.abort();
            }

            paired.watcher = Some(spawn_watcher(&paired.remote, &observer));
        }
    }

    /// In the foreground every paired host stays connected so its session
    /// list stays live; in the background only open views hold a link, and
    /// the app closes those itself before it is suspended.
    pub fn set_foreground(&self, foreground: bool) {
        self.foreground.store(foreground, Ordering::Relaxed);

        for paired in self.hosts.lock().iter() {
            if foreground {
                paired.remote.keep_connected();
            } else {
                paired.remote.stop_listing();
            }
        }
    }

    /// Pair with a host from a scanned or pasted `niumaterm://pair` link, or
    /// from a typed code plus the host's relay.
    pub async fn pair(
        self: Arc<Self>,
        input: String,
        relay_url: Option<String>,
        access_key: Option<String>,
    ) -> Result<HostRecord, CoreError> {
        let core = Arc::clone(&self);

        let record = runtime()
            .spawn(async move { core.pair_on_runtime(input, relay_url, access_key).await })
            .await??;

        Ok(record)
    }

    /// Forget a host on this device. The host keeps its record of this
    /// device until the user removes it there.
    pub fn forget(&self, host: String) {
        let removed = {
            let mut hosts = self.hosts.lock();

            hosts
                .iter()
                .position(|paired| paired.remote.id().as_str() == host)
                .map(|index| hosts.remove(index))
        };

        if let Some(paired) = removed {
            if let Some(watcher) = paired.watcher {
                watcher.abort();
            }

            paired.remote.shutdown();
        }

        let mut records = self.records.lock();

        records.retain(|record| record.id.as_str() != host);

        if let Err(error) = save_hosts(&self.dir, &records) {
            warn!(%error, "cannot store the paired hosts");
        }
    }

    pub async fn sessions(&self, host: String) -> Result<Vec<SessionRecord>, CoreError> {
        let remote = self.remote(&host)?;

        let sessions = runtime()
            .spawn(async move { remote.list_sessions().await })
            .await??;

        Ok(sessions.into_iter().map(SessionRecord::from).collect())
    }

    pub async fn host_info(&self, host: String) -> Result<HostOffer, CoreError> {
        let remote = self.remote(&host)?;

        let info = runtime()
            .spawn(async move { remote.host_info().await })
            .await??;

        Ok(info.into())
    }

    /// Start an agent on the host from a profile and a workspace path that
    /// [`Self::host_info`] listed, and return the new session's id.
    pub async fn open_agent(
        &self,
        host: String,
        profile: String,
        workspace: String,
    ) -> Result<String, CoreError> {
        let remote = self.remote(&host)?;

        let session = runtime()
            .spawn(async move { remote.open_agent(profile, workspace).await })
            .await??;

        Ok(session)
    }

    /// Open a view of an agent session, which takes control of it from the
    /// person at the host. Dropping the handle detaches.
    pub fn attach_agent(
        &self,
        host: String,
        session: String,
        observer: Arc<dyn AgentObserver>,
    ) -> Result<Arc<AgentHandle>, CoreError> {
        let remote = self.remote(&host)?;

        Ok(AgentHandle::attach(remote, session, observer))
    }

    /// Start a shell on the host at this view's grid size and open a view
    /// of it. The shell runs headless there until someone ends it.
    pub async fn open_terminal(
        &self,
        host: String,
        cols: u16,
        rows: u16,
        observer: Arc<dyn TerminalObserver>,
    ) -> Result<Arc<TerminalHandle>, CoreError> {
        let remote = self.remote(&host)?;
        let opener = Arc::clone(&remote);

        let pty = runtime()
            .spawn(async move { opener.open_terminal(cols, rows).await })
            .await??;

        TerminalHandle::attach(remote, pty, cols, rows, observer)
    }

    /// Open a view of a terminal session, which takes control of it from
    /// the person at the host. Dropping the handle detaches.
    pub fn attach_terminal(
        &self,
        host: String,
        session: String,
        cols: u16,
        rows: u16,
        observer: Arc<dyn TerminalObserver>,
    ) -> Result<Arc<TerminalHandle>, CoreError> {
        let remote = self.remote(&host)?;
        let pty = remote.view(session);

        TerminalHandle::attach(remote, pty, cols, rows, observer)
    }
}

impl MobileCore {
    fn remote(&self, host: &str) -> Result<Arc<RemoteHost>> {
        self.hosts
            .lock()
            .iter()
            .find(|paired| paired.remote.id().as_str() == host)
            .map(|paired| Arc::clone(&paired.remote))
            .ok_or_else(|| anyhow!("this device is not paired with that computer"))
    }

    fn add_host(&self, record: PairedHost) {
        let records = Arc::clone(&self.records);
        let dir = self.dir.clone();

        let remote = RemoteHost::new(
            record,
            Arc::clone(&self.key),
            self.device.app_version.clone(),
            move |updated| {
                let mut records = records.lock();

                if let Some(slot) = records.iter_mut().find(|known| known.id == updated.id) {
                    *slot = updated;
                }

                if let Err(error) = save_hosts(&dir, &records) {
                    warn!(%error, "cannot store the paired hosts");
                }
            },
        );

        if self.foreground.load(Ordering::Relaxed) {
            remote.keep_connected();
        }

        let watcher = self
            .observer
            .lock()
            .as_ref()
            .map(|observer| spawn_watcher(&remote, observer));

        self.hosts.lock().push(Paired { remote, watcher });
    }

    async fn pair_on_runtime(
        &self,
        input: String,
        relay_url: Option<String>,
        access_key: Option<String>,
    ) -> Result<HostRecord> {
        let (code, address, host_key, relay) = if input.trim().starts_with("niumaterm:") {
            let link =
                PairingLink::parse(&input).context("that is not a NiumaTerm pairing link")?;

            // The relay reaches the host from any network. A LAN address from
            // the link is the route only when the host has no relay.
            let address = match link.relay {
                Some(_) => None,
                None => link.addresses.first().cloned(),
            };

            (link.code, address, Some(link.host_key), link.relay)
        } else {
            let code = PairingCode::parse(&input).context("that is not a pairing code")?;

            let relay = match (relay_url, access_key) {
                (Some(url), Some(key)) if !url.trim().is_empty() && !key.trim().is_empty() => {
                    RelayAccess {
                        url: url.trim().to_owned(),
                        access_key: key.trim().to_owned(),
                    }
                }
                _ => bail!("enter the relay URL and access key shown on the computer"),
            };

            (code, None, None, Some(relay))
        };

        let record = client::pair(
            address.as_deref(),
            &code,
            &self.key,
            self.device.clone(),
            host_key,
            relay,
        )
        .await?;

        // Pairing again with a known host replaces its record and
        // connection, since the host may have moved or changed its relay.
        let stale = {
            let mut hosts = self.hosts.lock();

            hosts
                .iter()
                .position(|paired| paired.remote.id() == &record.id)
                .map(|index| hosts.remove(index))
        };

        if let Some(paired) = stale {
            if let Some(watcher) = paired.watcher {
                watcher.abort();
            }

            paired.remote.shutdown();
        }

        {
            let mut records = self.records.lock();

            records.retain(|known| known.id != record.id);
            records.push(record.clone());

            save_hosts(&self.dir, &records).context("storing the paired host")?;
        }

        let id = record.id.as_str().to_owned();

        self.add_host(record);

        let remote = self.remote(&id)?;

        Ok(host_record(&remote, *remote.status().borrow()))
    }
}

fn host_record(remote: &RemoteHost, status: Status) -> HostRecord {
    HostRecord {
        id: remote.id().as_str().to_owned(),
        name: remote.name(),
        status: status.into(),
    }
}

fn spawn_watcher(remote: &Arc<RemoteHost>, observer: &Arc<dyn CoreObserver>) -> AbortHandle {
    runtime()
        .spawn(watch(Arc::clone(remote), Arc::clone(observer)))
        .abort_handle()
}

/// Report a host's status, and its session list whenever it is connected
/// and the list may have changed.
async fn watch(remote: Arc<RemoteHost>, observer: Arc<dyn CoreObserver>) {
    let mut status = remote.status();
    let mut sessions = remote.session_changes();

    loop {
        let current = *status.borrow_and_update();

        sessions.borrow_and_update();

        observer.host_changed(host_record(&remote, current));

        if current == Status::Connected {
            match remote.list_sessions().await {
                Ok(list) => observer.sessions_changed(
                    remote.id().as_str().to_owned(),
                    list.into_iter().map(SessionRecord::from).collect(),
                ),
                Err(error) => debug!(%error, "listing the host's sessions failed"),
            }
        }

        select! {
            changed = status.changed() => if changed.is_err() {
                return;
            },
            changed = sessions.changed() => if changed.is_err() {
                return;
            },
        }
    }
}
