//! Remote sessions in the application: hosting for paired devices, pairing
//! with other computers, and opening their terminals in tabs. Network work
//! runs on the shared runtime; results come back to this global, whose
//! observers (the settings page) refresh.

use std::collections::HashMap;
use std::env;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, Result, anyhow};
use app::agent_tab::execution::SessionOwner;
use app::agent_tab::{AgentKind, AgentPane, AgentPaneEvent, remote as agent_remote};
use app::terminal_tab::view::{HostShare, TerminalPane};
use gpui::{
    App, BorrowAppContext as _, Entity, EntityId, Global, SharedString, Subscription, Task, Window,
};
use gpui_component::Root;
use nmt_platform::runtime;
use nmt_remote::NetworkPty;
use nmt_remote::client::pair;
use nmt_remote::connection::{RemoteHost, Status};
use nmt_remote::host::{DEFAULT_PORT, HostConfig, HostService};
use nmt_remote::sessions::{AgentControl, SessionRegistry, TerminalControl};
use nmt_remote::store::{
    PairedDevice, PairedHost, load_hosts, load_or_create_identity, load_relay_access_key,
    remote_dir, save_hosts, save_relay_access_key,
};
use nmt_remote_core::identity::{DeviceId, DeviceKey};
use nmt_remote_core::messages::{DeviceInfo, DeviceKind, RelayAccess};
use nmt_remote_core::pairing::{PairingCode, PairingLink};
use nmt_remote_core::rpc::{SessionInfo, SessionKind};
use rust_i18n::t;
use tokio::select;
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::warn;

use crate::ui::{AppSettings, AppWindow};

const APP_VERSION: &str = env!("NIUMATERM_VERSION");

/// Grid a new remote terminal starts with; the tab's layout resizes it.
const INITIAL_GRID: (u16, u16) = (100, 30);

pub(crate) struct Remote {
    key: Option<Arc<DeviceKey>>,
    host: Option<HostService>,

    /// Host tabs and headless terminals offered to paired devices. Tabs
    /// register whether or not hosting is on, so turning it on offers the
    /// tabs already open.
    registry: Arc<SessionRegistry>,

    /// Host session ids of shared tabs, by pane entity.
    shared_tabs: HashMap<EntityId, String>,

    /// What serves each shared agent tab, by pane entity: the task answering
    /// paired devices and the subscription keeping its listed title current.
    shared_agents: HashMap<EntityId, (Task<()>, Subscription)>,

    hosts: Vec<PairedHost>,
    connections: HashMap<DeviceId, Arc<RemoteHost>>,

    /// The session lists of paired hosts, as last fetched.
    host_sessions: HashMap<DeviceId, Vec<SessionInfo>>,

    /// Host records updated by a connection, saved on the UI thread.
    record_updates: UnboundedSender<PairedHost>,

    /// Drafts of the "connect to a computer" form.
    pub(crate) address: SharedString,

    pub(crate) code: SharedString,

    /// Drafts of the relay form, applied together: every edit to the live
    /// settings would restart hosting.
    pub(crate) relay_url: SharedString,

    pub(crate) relay_key: SharedString,

    /// The relay the running host registered with.
    hosted_relay: Option<RelayAccess>,

    /// The outcome of the last action, shown on the settings page.
    status: Option<SharedString>,

    busy: bool,
}

impl Global for Remote {}

/// Install the global and start hosting when the setting is on.
pub(crate) fn initialize(cx: &mut App) {
    let (record_updates, mut records) = mpsc::unbounded_channel::<PairedHost>();

    cx.set_global(Remote {
        key: None,
        host: None,
        registry: SessionRegistry::new(),
        shared_tabs: HashMap::new(),
        shared_agents: HashMap::new(),
        hosts: load_hosts(&remote_dir()),
        connections: HashMap::new(),
        host_sessions: HashMap::new(),
        record_updates,
        address: SharedString::default(),
        code: SharedString::default(),
        relay_url: cx
            .global::<AppSettings>()
            .config()
            .remote
            .relay_url
            .clone()
            .into(),
        relay_key: SharedString::default(),
        hosted_relay: None,
        status: None,
        busy: false,
    });

    cx.spawn(async move |cx| {
        while let Some(record) = records.recv().await {
            cx.update_global::<Remote, _>(|remote, _| {
                if let Some(saved) = remote.hosts.iter_mut().find(|host| host.id == record.id) {
                    *saved = record;

                    if let Err(error) = save_hosts(&remote_dir(), &remote.hosts) {
                        warn!(%error, "failed to save a paired host");
                    }
                }
            });
        }
    })
    .detach();

    sync_hosting(cx);
}

/// Start or stop hosting to match the setting, restarting it when the
/// relay changed. Runs on every settings change; a host that failed to
/// start is retried on the next one.
pub(crate) fn sync_hosting(cx: &mut App) {
    let enabled = cx.global::<AppSettings>().config().remote.enabled;
    let remote = cx.global::<Remote>();

    let relay_changed = remote.host.is_some() && remote.hosted_relay != configured_relay(cx);

    if relay_changed {
        set_hosting(false, cx);
    }

    if enabled != cx.global::<Remote>().host.is_some() {
        set_hosting(enabled, cx);
    }
}

/// The relay in the settings, when both its URL and access key are set.
fn configured_relay(cx: &App) -> Option<RelayAccess> {
    let url = cx.global::<AppSettings>().config().remote.relay_url.trim();

    if url.is_empty() {
        return None;
    }

    Some(RelayAccess {
        url: url.to_owned(),
        access_key: load_relay_access_key(&remote_dir())?,
    })
}

/// Seal the access key typed into the relay form, reporting whether it is
/// saved. An empty field keeps the key saved before.
pub(crate) fn save_relay_key(cx: &mut App) -> bool {
    let remote = cx.global_mut::<Remote>();
    let key = remote.relay_key.trim().to_owned();

    if key.is_empty() {
        return true;
    }

    let result = save_relay_access_key(&remote_dir(), &key);
    let saved = result.is_ok();

    if saved {
        remote.relay_key = SharedString::default();
    }

    remote.report(result.map_err(Into::into));

    saved
}

impl Remote {
    pub(crate) fn hosting_address(&self) -> Option<String> {
        self.host
            .as_ref()
            .map(|host| format!("{}:{}", local_ip(), host.local_addr().port()))
    }

    pub(crate) fn device_id(&self) -> Option<DeviceId> {
        self.key.as_ref().map(|key| key.id())
    }

    pub(crate) fn pairing(&self) -> Option<(PairingCode, u64)> {
        self.host.as_ref()?.pairing()
    }

    /// The showing code as a link that also carries this host's key, so
    /// the device pairs only with this host, and its relay, so it pairs
    /// from off the LAN too.
    pub(crate) fn pairing_link(&self) -> Option<String> {
        let key = self.key.as_ref()?;
        let (code, _) = self.pairing()?;

        let link = PairingLink {
            code,
            host_id: key.id(),
            host_key: *key.public(),
            relay: self.hosted_relay.clone(),
            addresses: self.hosting_address().into_iter().collect(),
        };

        Some(link.to_url())
    }

    pub(crate) fn relay_configured(&self) -> bool {
        self.hosted_relay.is_some()
    }

    pub(crate) fn devices(&self) -> Vec<PairedDevice> {
        self.host
            .as_ref()
            .map(HostService::devices)
            .unwrap_or_default()
    }

    /// Terminals paired devices started here, which the host user can end.
    pub(crate) fn remote_created_sessions(&self) -> Vec<SessionInfo> {
        self.registry.remote_sessions()
    }

    pub(crate) fn hosts(&self) -> &[PairedHost] {
        &self.hosts
    }

    pub(crate) fn host_status(&self, id: &DeviceId) -> Status {
        self.connections
            .get(id)
            .map_or(Status::Idle, |host| *host.status().borrow())
    }

    pub(crate) fn host_sessions(&self, id: &DeviceId) -> Option<&[SessionInfo]> {
        self.host_sessions.get(id).map(Vec::as_slice)
    }

    pub(crate) fn status(&self) -> Option<&SharedString> {
        self.status.as_ref()
    }

    pub(crate) fn busy(&self) -> bool {
        self.busy
    }

    fn report(&mut self, result: Result<()>) {
        self.busy = false;
        self.status = result.err().map(|error| format!("{error:#}").into());
    }
}

/// Start or stop accepting paired devices. Stopping ends the terminals they
/// opened here.
fn set_hosting(enabled: bool, cx: &mut App) {
    if !enabled {
        let remote = cx.global_mut::<Remote>();

        remote.host = None;
        remote.hosted_relay = None;

        return;
    }

    let relay = configured_relay(cx);

    let result = start_host(relay.clone(), cx).map(|host| {
        let remote = cx.global_mut::<Remote>();

        remote.host = Some(host);
        remote.hosted_relay = relay;
    });

    cx.global_mut::<Remote>().report(result);
}

fn start_host(relay: Option<RelayAccess>, cx: &mut App) -> Result<HostService> {
    let key = device_key(cx)?;
    let config = cx.global::<AppSettings>().config();
    let (shell, args) = cx.global::<AppSettings>().default_profile_command();
    let registry = Arc::clone(&cx.global::<Remote>().registry);

    // Pairing and session changes arrive on runtime threads; touching the
    // global from the UI thread notifies its observers, which a runtime
    // thread cannot do.
    let (changed, mut changes) = mpsc::unbounded_channel();
    let mut sessions = registry.subscribe();

    cx.spawn(async move |cx| {
        loop {
            select! {
                change = changes.recv() => if change.is_none() { break },
                change = sessions.changed() => if change.is_err() { break },
            }

            cx.update_global::<Remote, _>(|_, _| {});
        }
    })
    .detach();

    HostService::start(
        remote_dir(),
        DeviceKey::from_parts(*key.private(), *key.public()),
        HostConfig {
            port: config.remote.lan_port,
            device: device_info(&config.remote.device_name),
            shell,
            args,
            registry,
            relay,
            on_change: Arc::new(move || {
                let _ = changed.send(());
            }),
        },
    )
}

pub(crate) fn start_pairing(cx: &mut App) {
    let remote = cx.global_mut::<Remote>();

    let result = match &remote.host {
        Some(host) => host.start_pairing().map(|_| ()),
        None => Err(anyhow!(t!("remote-hosting-off").into_owned())),
    };

    remote.report(result);
}

pub(crate) fn cancel_pairing(cx: &mut App) {
    if let Some(host) = &cx.global::<Remote>().host {
        host.cancel_pairing();
    }

    cx.update_global::<Remote, _>(|_, _| {});
}

pub(crate) fn remove_device(id: &DeviceId, cx: &mut App) {
    let remote = cx.global_mut::<Remote>();

    let result = match &remote.host {
        Some(host) => host.remove_device(id),
        None => Ok(()),
    };

    remote.report(result);
}

/// End a terminal a paired device started here.
pub(crate) fn close_remote_created(session: &str, cx: &mut App) {
    cx.global_mut::<Remote>().registry.close_remote(session);
}

/// Offer a host tab's terminal to paired devices for as long as the pane
/// lives. Remote panes are not offered again.
pub(crate) fn share_tab(pane: &Entity<TerminalPane>, cx: &mut App) {
    let pane_id = pane.entity_id();

    let (title, (cols, rows), messenger, remote) = {
        let pane = pane.read(cx);

        (
            pane.profile_name().to_owned(),
            pane.grid_size(),
            pane.session_messenger(),
            pane.is_remote(),
        )
    };

    if remote {
        return;
    }

    let registry = Arc::clone(&cx.global::<Remote>().registry);
    let claimed_remotely = Arc::new(AtomicBool::new(false));

    let session = registry.register_tab(
        title,
        cols,
        rows,
        TerminalControl {
            messenger,
            claimed_remotely: Arc::clone(&claimed_remotely),
        },
    );

    let on_size = {
        let registry = Arc::clone(&registry);
        let session = session.clone();

        Box::new(move |cols, rows| registry.note_local_resize(&session, cols, rows))
    };

    pane.update(cx, |pane, _| {
        pane.share_with_host(HostShare {
            claimed_remotely,
            on_size,
        })
    });

    cx.global_mut::<Remote>()
        .shared_tabs
        .insert(pane_id, session.clone());

    cx.observe_release(pane, move |_, cx| {
        registry.unregister(&session);

        cx.global_mut::<Remote>().shared_tabs.remove(&pane_id);
    })
    .detach();
}

/// Offer a host agent tab to paired devices for as long as the pane lives.
/// A pane following another computer's session is not offered again.
pub(crate) fn share_agent_tab(pane: &Entity<AgentPane>, cx: &mut App) {
    let pane_id = pane.entity_id();

    let (session, remote) = {
        let pane = pane.read(cx);

        (pane.agent_session(), pane.remote_address().is_some())
    };

    let Some(session) = session else {
        return;
    };

    if remote {
        return;
    }

    let (title, harness) = {
        let profile = session.read(cx).profile();

        let title = if profile.name.trim().is_empty() {
            profile.kind.display().to_owned()
        } else {
            profile.name.clone()
        };

        let harness: &str = profile.kind.into();

        (title, harness.to_owned())
    };

    let registry = Arc::clone(&cx.global::<Remote>().registry);
    let (requests, requests_rx) = mpsc::unbounded_channel();

    let id = registry.register_agent(title.clone(), harness, AgentControl { requests });
    let task = agent_remote::serve(session.downgrade(), requests_rx, cx);

    let titles = {
        let registry = Arc::clone(&registry);
        let id = id.clone();

        // An empty suggestion clears a conversation's title; the tab then
        // goes by its profile again.
        cx.subscribe(&session, move |_, event: &AgentPaneEvent, _| {
            if let AgentPaneEvent::TitleSuggested(suggested) = event {
                let listed = if suggested.is_empty() {
                    title.clone()
                } else {
                    suggested.clone()
                };

                registry.set_title(&id, listed);
            }
        })
    };

    cx.global_mut::<Remote>()
        .shared_agents
        .insert(pane_id, (task, titles));

    cx.observe_release(pane, move |_, cx| {
        registry.unregister(&id);

        cx.global_mut::<Remote>().shared_agents.remove(&pane_id);
    })
    .detach();
}

/// Keep the name paired devices see for a host tab current.
pub(crate) fn tab_title_changed(pane: &Entity<TerminalPane>, title: &str, cx: &mut App) {
    let remote = cx.global::<Remote>();

    if let Some(session) = remote.shared_tabs.get(&pane.entity_id()) {
        remote.registry.set_title(session, title.to_owned());
    }
}

/// Pair with the computer named in the connect form. The code field also
/// takes a pasted pairing link, which carries the code, the host's
/// addresses, and the host key to insist on. An empty address searches the
/// LAN for the host showing the code.
pub(crate) fn pair_with_host(cx: &mut App) {
    let remote = cx.global::<Remote>();
    let typed_address = remote.address.trim();

    let parsed = if remote.code.trim().starts_with("niumaterm:") {
        PairingLink::parse(&remote.code).map(|link| {
            let address = if typed_address.is_empty() {
                link.addresses.first().cloned()
            } else {
                Some(with_default_port(typed_address))
            };

            (link.code, address, Some(link.host_key), link.relay)
        })
    } else {
        PairingCode::parse(&remote.code).map(|code| {
            let address = (!typed_address.is_empty()).then(|| with_default_port(typed_address));

            (code, address, None, None)
        })
    };

    let (code, address, expected_host_key, relay) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            cx.global_mut::<Remote>().report(Err(error.into()));

            return;
        }
    };

    let key = match device_key(cx) {
        Ok(key) => key,
        Err(error) => {
            cx.global_mut::<Remote>().report(Err(error));

            return;
        }
    };

    let device = device_info(&cx.global::<AppSettings>().config().remote.device_name);
    let remote = cx.global_mut::<Remote>();

    remote.busy = true;

    remote.status = Some(match &address {
        Some(address) => t!("remote-pairing-with", address = address)
            .into_owned()
            .into(),
        None => t!("remote-searching").into_owned().into(),
    });

    let task = runtime().spawn(async move {
        pair(
            address.as_deref(),
            &code,
            &key,
            device,
            expected_host_key,
            relay,
        )
        .await
    });

    cx.spawn(async move |cx| {
        let result = task
            .await
            .context("pairing stopped")
            .and_then(|paired| paired);

        cx.update_global::<Remote, _>(|remote, _| match result {
            Ok(paired) => {
                // A new pairing replaces a connection that the host may
                // have refused under the old record.
                if let Some(old) = remote.connections.remove(&paired.id) {
                    old.shutdown();
                }

                remote.hosts.retain(|host| host.id != paired.id);
                remote.hosts.push(paired);

                remote.code = SharedString::default();

                let saved = save_hosts(&remote_dir(), &remote.hosts);

                remote.report(saved.map_err(Into::into));
            }
            Err(error) => remote.report(Err(error)),
        });
    })
    .detach();
}

pub(crate) fn forget_host(id: &DeviceId, cx: &mut App) {
    let remote = cx.global_mut::<Remote>();

    remote.hosts.retain(|host| &host.id != id);
    remote.host_sessions.remove(id);

    if let Some(connection) = remote.connections.remove(id) {
        connection.shutdown();
    }

    let saved = save_hosts(&remote_dir(), &remote.hosts);

    remote.report(saved.map_err(Into::into));
}

/// The lasting connection to a paired host, started on first use.
fn connection(id: &DeviceId, cx: &mut App) -> Result<Arc<RemoteHost>> {
    if let Some(connection) = cx.global::<Remote>().connections.get(id) {
        return Ok(Arc::clone(connection));
    }

    let key = device_key(cx)?;
    let remote = cx.global::<Remote>();

    let record = remote
        .hosts
        .iter()
        .find(|host| &host.id == id)
        .cloned()
        .context("this computer is no longer paired")?;

    let updates = remote.record_updates.clone();

    let connection = RemoteHost::new(record, key, APP_VERSION.into(), move |record| {
        let _ = updates.send(record);
    });

    // The settings page shows each host's link state and session list.
    let mut status = connection.status();
    let mut sessions = connection.session_changes();

    let watched = id.clone();

    cx.spawn(async move |cx| {
        loop {
            let listed = select! {
                change = status.changed() => { if change.is_err() { break } false }

                change = sessions.changed() => { if change.is_err() { break } true }
            };

            cx.update(|cx| {
                if listed {
                    refresh_sessions(&watched, cx);
                } else {
                    cx.update_global::<Remote, _>(|_, _| {});
                }
            });
        }
    })
    .detach();

    cx.global_mut::<Remote>()
        .connections
        .insert(id.clone(), Arc::clone(&connection));

    Ok(connection)
}

/// Fetch a paired host's session list for the settings page.
pub(crate) fn refresh_sessions(id: &DeviceId, cx: &mut App) {
    let connection = match connection(id, cx) {
        Ok(connection) => connection,
        Err(error) => {
            cx.global_mut::<Remote>().report(Err(error));

            return;
        }
    };

    let task = runtime().spawn(async move { connection.list_sessions().await });
    let id = id.clone();

    cx.spawn(async move |cx| {
        let result = task.await.context("listing stopped").and_then(|list| list);

        cx.update_global::<Remote, _>(|remote, _| match result {
            Ok(sessions) => {
                remote.host_sessions.insert(id, sessions);
            }
            Err(error) => remote.report(Err(error)),
        });
    })
    .detach();
}

/// Open a new terminal on a paired host in a tab of `window`. The tab owns
/// the session: closing it ends the terminal on the host.
pub(crate) fn open_terminal(id: &DeviceId, window: &mut Window, cx: &mut App) {
    let Some(app) = app_window(window, cx) else {
        return;
    };

    let connection = match connection(id, cx) {
        Ok(connection) => connection,
        Err(error) => {
            cx.global_mut::<Remote>().report(Err(error));

            return;
        }
    };

    let remote = cx.global_mut::<Remote>();

    remote.busy = true;

    remote.status = Some(
        t!("remote-connecting-to", name = connection.name())
            .into_owned()
            .into(),
    );

    let task = runtime().spawn(async move {
        connection
            .open_terminal(INITIAL_GRID.0, INITIAL_GRID.1)
            .await
    });

    window
        .spawn(cx, async move |cx| {
            let result = task
                .await
                .context("connecting stopped")
                .and_then(|opened| opened);

            let _ = cx.update(|window, cx| match result {
                Ok(pty) => {
                    cx.global_mut::<Remote>().report(Ok(()));

                    app.update(cx, |app, cx| {
                        app.open_remote_terminal(pty, true, window, cx)
                    });
                }
                Err(error) => cx.global_mut::<Remote>().report(Err(error)),
            });
        })
        .detach();
}

/// Open a view of an existing session on a paired host. Closing the tab
/// leaves the session running there.
pub(crate) fn open_session(
    id: &DeviceId,
    session: &SessionInfo,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(app) = app_window(window, cx) else {
        return;
    };

    let connection = match connection(id, cx) {
        Ok(connection) => connection,
        Err(error) => {
            cx.global_mut::<Remote>().report(Err(error));

            return;
        }
    };

    match session.kind {
        SessionKind::Agent => {
            let Some(kind) = session.harness.as_deref().and_then(AgentKind::from_id) else {
                cx.global_mut::<Remote>()
                    .report(Err(anyhow!(t!("remote-agent-unknown").into_owned())));

                return;
            };

            let (owner, pane) =
                agent_remote::open(connection, session.session.clone(), kind, window, cx);

            let title = session.title.clone();

            app.update(cx, |app, cx| {
                app.open_remote_agent_tab(owner, pane, title, window, cx)
            });
        }
        SessionKind::Terminal | SessionKind::Unknown => {
            let pty = connection.view(session.session.clone());

            app.update(cx, |app, cx| {
                app.open_remote_terminal(pty, false, window, cx)
            });
        }
    }
}

/// The name of the paired host with this device id.
pub(crate) fn paired_host_name(id: &str, cx: &App) -> Option<String> {
    cx.global::<Remote>()
        .hosts
        .iter()
        .find(|host| host.id.as_str() == id)
        .map(|host| host.name.clone())
}

/// A view of a saved remote tab's session, reattached on restore. `None`
/// when the host is no longer paired.
pub(crate) fn restore_view(host: &str, session: &str, cx: &mut App) -> Option<NetworkPty> {
    let id = cx
        .global::<Remote>()
        .hosts
        .iter()
        .find(|paired| paired.id.as_str() == host)
        .map(|paired| paired.id.clone())?;

    connection(&id, cx)
        .inspect_err(|error| warn!(%error, "cannot restore a remote tab"))
        .ok()
        .map(|connection| connection.view(session.to_owned()))
}

/// A view of a saved remote agent tab's session, reattached on restore.
/// `None` when the host is no longer paired.
pub(crate) fn restore_agent(
    host: &str,
    session: &str,
    kind: AgentKind,
    window: &mut Window,
    cx: &mut App,
) -> Option<(SessionOwner, Entity<AgentPane>)> {
    let id = cx
        .global::<Remote>()
        .hosts
        .iter()
        .find(|paired| paired.id.as_str() == host)
        .map(|paired| paired.id.clone())?;

    let connection = connection(&id, cx)
        .inspect_err(|error| warn!(%error, "cannot restore a remote agent tab"))
        .ok()?;

    Some(agent_remote::open(
        connection,
        session.to_owned(),
        kind,
        window,
        cx,
    ))
}

/// The device key, loaded from secret storage on first use.
fn device_key(cx: &mut App) -> Result<Arc<DeviceKey>> {
    if let Some(key) = &cx.global::<Remote>().key {
        return Ok(Arc::clone(key));
    }

    let key = Arc::new(load_or_create_identity(&remote_dir())?);

    cx.global_mut::<Remote>().key = Some(Arc::clone(&key));

    Ok(key)
}

fn device_info(configured_name: &str) -> DeviceInfo {
    let name = if configured_name.is_empty() {
        env::var("COMPUTERNAME")
            .or_else(|_| env::var("HOSTNAME"))
            .unwrap_or_else(|_| "NiumaTerm".into())
    } else {
        configured_name.to_owned()
    };

    DeviceInfo {
        name,
        kind: DeviceKind::Desktop,
        platform: env::consts::OS.into(),
        app_version: APP_VERSION.into(),
    }
}

/// Accept `host` or `host:port`; the port defaults to the standard one.
fn with_default_port(address: &str) -> String {
    if address
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>().is_ok())
    {
        address.to_owned()
    } else {
        format!("{address}:{DEFAULT_PORT}")
    }
}

/// The address other machines on the LAN most likely reach this one at: the
/// source address of the default route. Connecting a UDP socket sends
/// nothing; it only selects the route.
fn local_ip() -> String {
    UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|socket| {
            socket.connect(("192.0.2.1", 9))?;

            socket.local_addr()
        })
        .map(|address| address.ip().to_string())
        .unwrap_or_else(|error| {
            warn!(%error, "no LAN address found");

            "127.0.0.1".into()
        })
}

fn app_window(window: &mut Window, cx: &mut App) -> Option<Entity<AppWindow>> {
    let root = window.root::<Root>().flatten()?;

    root.read(cx).view().clone().downcast::<AppWindow>().ok()
}
