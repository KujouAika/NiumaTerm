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
use app::remote_control::HostControl;
use app::terminal_tab::view::{HostShare, TerminalPane};
use gpui::{
    App, BorrowAppContext as _, Entity, EntityId, Global, SharedString, Subscription, Task, Window,
};
use gpui_component::Root;
use nmt_platform::runtime;
use nmt_remote::client::pair;
use nmt_remote::connection::{RemoteHost, Status};
use nmt_remote::host::{DEFAULT_PORT, HostConfig, HostService};
use nmt_remote::sessions::{
    AgentControl, AgentRequest, HostRequest, SessionRegistry, TerminalControl,
};
use nmt_remote::store::{
    PairedDevice, PairedHost, load_hosts, load_or_create_identity, load_relay_access_key,
    remote_dir, save_hosts, save_relay_access_key,
};
use nmt_remote::{NetworkPty, local_view};
use nmt_remote_core::identity::{DeviceId, DeviceKey};
use nmt_remote_core::messages::{DeviceInfo, DeviceKind, RelayAccess};
use nmt_remote_core::pairing::{PairingCode, PairingLink};
use nmt_remote_core::rpc::{
    AgentOpen, AgentProfileInfo, HostInfo, SessionInfo, SessionKind, SessionRef, WorkspaceInfo,
};
use rust_i18n::t;
use serde_json::Value;
use tokio::select;
use tokio::sync::mpsc::{self, UnboundedSender, WeakUnboundedSender};
use tracing::warn;
use uuid::Uuid;

use crate::last_active_window;
use crate::ui::settings::AgentProfile;
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

    /// The remote-created terminal each host view of one shows, by pane
    /// entity, so the view's tab can mark the devices watching it too.
    viewed_sessions: HashMap<EntityId, String>,

    /// What serves each shared agent tab, by pane entity.
    shared_agents: HashMap<EntityId, SharedAgent>,

    /// Agent tabs offered while still waiting to be restored, by the id
    /// devices know them by: the task that restores one on a device's first
    /// request.
    restoring_agents: HashMap<String, Task<()>>,

    hosts: Vec<PairedHost>,
    connections: HashMap<DeviceId, Arc<RemoteHost>>,

    /// The session lists of paired hosts, as last fetched.
    host_sessions: HashMap<DeviceId, Vec<SessionInfo>>,

    /// What each paired host lets this computer start, as last fetched.
    host_offers: HashMap<DeviceId, HostInfo>,

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

    /// Set once the startup windows restored their tabs. Hosting waits for
    /// it: a device reconnecting after this app restarted asks for tabs by
    /// the ids they are restored under, and would find none before then.
    tabs_restored: bool,
}

impl Global for Remote {}

/// A connected paired host as the workspace sidebar lists it: its sessions,
/// and what it lets this computer start.
pub(crate) struct RemoteWorkspace {
    pub(crate) id: DeviceId,
    pub(crate) name: String,
    pub(crate) sessions: Vec<SessionInfo>,
    pub(crate) offers: Option<HostInfo>,
}

/// One shared agent tab: the id paired devices know it by, the task
/// answering them, and the subscription keeping its listed title current.
struct SharedAgent {
    id: String,
    _serve: Task<()>,
    _titles: Subscription,
}

/// Install the global and start hosting when the setting is on.
pub(crate) fn initialize(cx: &mut App) {
    let (record_updates, mut records) = mpsc::unbounded_channel::<PairedHost>();
    let (host_requests, mut host_requests_rx) = mpsc::unbounded_channel();

    let registry = SessionRegistry::new();

    registry.serve_host_requests(host_requests);

    cx.spawn(async move |cx| {
        while let Some(request) = host_requests_rx.recv().await {
            cx.update(|cx| answer_host(request, cx));
        }
    })
    .detach();

    cx.set_global(Remote {
        key: None,
        host: None,
        registry,
        shared_tabs: HashMap::new(),
        viewed_sessions: HashMap::new(),
        shared_agents: HashMap::new(),
        restoring_agents: HashMap::new(),
        hosts: load_hosts(&remote_dir()),
        connections: HashMap::new(),
        host_sessions: HashMap::new(),
        host_offers: HashMap::new(),
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
        tabs_restored: false,
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
}

/// Start hosting, if the setting is on, now that the startup windows have
/// restored their tabs.
pub(crate) fn tabs_restored(cx: &mut App) {
    cx.global_mut::<Remote>().tabs_restored = true;

    sync_hosting(cx);

    let hosts: Vec<DeviceId> = cx
        .global::<Remote>()
        .hosts
        .iter()
        .map(|host| host.id.clone())
        .collect();

    for id in hosts {
        list_host(&id, cx);
    }
}

/// Keep a paired host connected so the sidebar lists its sessions as they
/// change, reconnecting whenever it comes back into reach.
fn list_host(id: &DeviceId, cx: &mut App) {
    match connection(id, cx) {
        Ok(connection) => connection.keep_connected(),
        Err(error) => warn!(%error, "cannot list a paired host's sessions"),
    }
}

/// Start or stop hosting to match the setting, restarting it when the
/// relay changed. Runs on every settings change; a host that failed to
/// start is retried on the next one.
pub(crate) fn sync_hosting(cx: &mut App) {
    let enabled = cx.global::<AppSettings>().config().remote.enabled;
    let remote = cx.global::<Remote>();

    if !remote.tabs_restored {
        return;
    }

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

    /// Names of the paired devices connected to this host now.
    pub(crate) fn connected_devices(&self) -> Vec<String> {
        self.host
            .as_ref()
            .map(HostService::connected_devices)
            .unwrap_or_default()
    }

    /// Names of the paired devices viewing any of these host panes.
    pub(crate) fn viewers_of(&self, panes: impl IntoIterator<Item = EntityId>) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();

        for pane in panes {
            let session = self
                .shared_tabs
                .get(&pane)
                .or_else(|| self.viewed_sessions.get(&pane))
                .or_else(|| self.shared_agents.get(&pane).map(|shared| &shared.id));

            for name in session
                .map(|session| self.registry.viewers(session))
                .unwrap_or_default()
            {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }

        names
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

    pub(crate) fn host_offers(&self, id: &DeviceId) -> Option<&HostInfo> {
        self.host_offers.get(id)
    }

    /// The paired hosts connected now, each with its sessions, in pairing
    /// order.
    pub(crate) fn remote_workspaces(&self) -> Vec<RemoteWorkspace> {
        self.hosts
            .iter()
            .filter(|host| self.host_status(&host.id) == Status::Connected)
            .map(|host| RemoteWorkspace {
                id: host.id.clone(),
                name: host.name.clone(),
                sessions: self.host_sessions(&host.id).unwrap_or_default().to_vec(),
                offers: self.host_offers(&host.id).cloned(),
            })
            .collect()
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
    let mut viewers = registry.subscribe_viewers();

    cx.spawn(async move |cx| {
        loop {
            select! {
                change = changes.recv() => if change.is_none() { break },
                change = sessions.changed() => if change.is_err() { break },
                change = viewers.changed() => if change.is_err() { break },
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

/// Open a tab on a terminal a paired device started here, alongside that
/// device's own view.
pub(crate) fn open_remote_created(session: &SessionInfo, window: &mut Window, cx: &mut App) {
    let Some(app) = app_window(window, cx) else {
        return;
    };

    let registry = Arc::clone(&cx.global::<Remote>().registry);

    // The terminal may have ended since the page listed it; the page then
    // refreshes without it.
    let Some(view) = local_view::open(&registry, &session.session) else {
        return;
    };

    let title = session.title.clone();

    app.update(cx, |app, cx| app.open_local_view(view, title, window, cx));
}

/// Offer a host tab's terminal to paired devices for as long as the pane
/// lives. Remote panes are not offered again.
pub(crate) fn share_tab(pane: &Entity<TerminalPane>, cx: &mut App) {
    let pane_id = pane.entity_id();

    let (title, (cols, rows), messenger, remote, viewed) = {
        let pane = pane.read(cx);

        (
            pane.profile_name().to_owned(),
            pane.grid_size(),
            pane.session_messenger(),
            pane.is_remote(),
            pane.remote_created_session().map(str::to_owned),
        )
    };

    // Devices already see the terminal this pane views; the tab only follows
    // who is watching it.
    if let Some(session) = viewed {
        let control = host_control(&cx.global::<Remote>().registry, &session);

        pane.update(cx, |pane, cx| pane.control_from_host(control, cx));

        cx.global_mut::<Remote>()
            .viewed_sessions
            .insert(pane_id, session);

        cx.observe_release(pane, move |_, cx| {
            cx.global_mut::<Remote>().viewed_sessions.remove(&pane_id);
        })
        .detach();

        return;
    }

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

    let control = host_control(&registry, &session);

    pane.update(cx, |pane, cx| {
        pane.share_with_host(HostShare {
            claimed_remotely,
            on_size,
        });

        pane.control_from_host(control, cx);
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

/// Offer a host agent tab to paired devices for as long as the pane lives,
/// under `id` when the tab was offered before (a restored tab) and a new id
/// otherwise. A pane following another computer's session is not offered
/// again.
pub(crate) fn share_agent_tab(pane: &Entity<AgentPane>, id: Option<String>, cx: &mut App) {
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

    let id = id.unwrap_or_else(|| format!("a-{}", Uuid::new_v4().simple()));

    // The task that held this tab's place until now forwards what it already
    // received and ends once the registry lets go of its channel.
    if let Some(placeholder) = cx.global_mut::<Remote>().restoring_agents.remove(&id) {
        placeholder.detach();
    }

    let registry = Arc::clone(&cx.global::<Remote>().registry);
    let (requests, requests_rx) = mpsc::unbounded_channel();

    registry.register_agent(
        id.clone(),
        title.clone(),
        harness,
        AgentControl { requests },
    );

    let serve = agent_remote::serve(session.downgrade(), requests_rx, cx);

    let control = host_control(&registry, &id);

    pane.update(cx, |pane, cx| pane.control_from_host(control, cx));

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

    cx.global_mut::<Remote>().shared_agents.insert(
        pane_id,
        SharedAgent {
            id: id.clone(),
            _serve: serve,
            _titles: titles,
        },
    );

    cx.observe_release(pane, move |_, cx| {
        registry.unregister(&id);

        cx.global_mut::<Remote>().shared_agents.remove(&pane_id);
    })
    .detach();
}

/// What a shared host tab needs to show who controls `session` from another
/// computer and to take it back.
fn host_control(registry: &Arc<SessionRegistry>, session: &str) -> HostControl {
    let viewers = Arc::clone(registry);
    let taker = Arc::clone(registry);
    let listed = session.to_owned();
    let taken = session.to_owned();

    HostControl {
        controllers: Arc::new(move || viewers.viewers(&listed)),
        changes: registry.subscribe_viewers(),
        take_back: Arc::new(move || taker.take_back(&taken)),
    }
}

/// The id paired devices know a shared agent tab by, saved with the tab.
pub(crate) fn shared_agent_id(pane: &Entity<AgentPane>, cx: &App) -> Option<String> {
    cx.global::<Remote>()
        .shared_agents
        .get(&pane.entity_id())
        .map(|shared| shared.id.clone())
}

/// Offer an agent tab that is still waiting to be restored under the id
/// devices knew it by before this app restarted, so a device that followed
/// it reattaches instead of losing its view. Restored tabs start only when
/// activated, so the first request from a device calls `restore`, which
/// starts the tab and reports whether it could; every request then goes on
/// to the live tab.
pub(crate) fn offer_restoring_agent(
    id: String,
    title: String,
    harness: String,
    restore: impl FnOnce(&mut App) -> bool + 'static,
    cx: &mut App,
) {
    let registry = Arc::clone(&cx.global::<Remote>().registry);

    let (requests, mut requests_rx) = mpsc::unbounded_channel();

    // Weak, so the channel closes once the live tab replaces this offer.
    let own = requests.downgrade();

    registry.register_agent(id.clone(), title, harness, AgentControl { requests });

    let task = cx.spawn({
        let id = id.clone();

        async move |cx| {
            let mut restore = Some(restore);

            while let Some(request) = requests_rx.recv().await {
                let forwarded = cx.update(|cx| {
                    let restoring = cx
                        .global_mut::<Remote>()
                        .restoring_agents
                        .remove(&id)
                        .map(Task::detach)
                        .is_some();

                    if restoring
                        && let Some(restore) = restore.take()
                        && !restore(cx)
                    {
                        return false;
                    }

                    // The offer still standing means the tab did not take
                    // over its id, so there is nothing to forward to.
                    match registry.agent(&id) {
                        Some(control)
                            if !own
                                .upgrade()
                                .is_some_and(|own| own.same_channel(&control.requests)) =>
                        {
                            control.requests.send(request).is_ok()
                        }
                        _ => false,
                    }
                });

                if !forwarded {
                    withdraw_offer(&registry, &id, &own);

                    break;
                }
            }
        }
    });

    cx.global_mut::<Remote>().restoring_agents.insert(id, task);
}

/// Withdraw the offer of an agent tab closed before it was ever restored.
pub(crate) fn withdraw_restoring_agent(id: &str, cx: &mut App) {
    let remote = cx.global_mut::<Remote>();

    if remote.restoring_agents.remove(id).is_some() {
        remote.registry.unregister(id);
    }
}

/// Unregister `id` if it is still the offer holding `own`, not a live tab
/// that took its place.
fn withdraw_offer(registry: &SessionRegistry, id: &str, own: &WeakUnboundedSender<AgentRequest>) {
    if let (Some(own), Some(control)) = (own.upgrade(), registry.agent(id))
        && own.same_channel(&control.requests)
    {
        registry.unregister(id);
    }
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

        cx.update(|cx| {
            let paired = cx.update_global::<Remote, _>(|remote, _| match result {
                Ok(paired) => {
                    // A new pairing replaces a connection that the host may
                    // have refused under the old record.
                    if let Some(old) = remote.connections.remove(&paired.id) {
                        old.shutdown();
                    }

                    let id = paired.id.clone();

                    remote.hosts.retain(|host| host.id != paired.id);
                    remote.hosts.push(paired);

                    remote.code = SharedString::default();

                    let saved = save_hosts(&remote_dir(), &remote.hosts);

                    remote.report(saved.map_err(Into::into));

                    Some(id)
                }
                Err(error) => {
                    remote.report(Err(error));

                    None
                }
            });

            if let Some(id) = paired {
                list_host(&id, cx);
            }
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

    // The settings page shows each host's link state and the sidebar its
    // sessions, which are listed afresh whenever the link comes up.
    let mut status = connection.status();
    let mut sessions = connection.session_changes();

    let watched = id.clone();

    cx.spawn(async move |cx| {
        loop {
            let listed = select! {
                change = status.changed() => {
                    if change.is_err() { break }

                    *status.borrow() == Status::Connected
                }

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

    let task = runtime().spawn(async move {
        let sessions = connection.list_sessions().await?;

        // A host from before agents could be started remotely answers
        // `unsupported`; it still lists its sessions.
        let offers = connection.host_info().await.ok();

        anyhow::Ok((sessions, offers))
    });

    let id = id.clone();

    cx.spawn(async move |cx| {
        let result = task.await.context("listing stopped").and_then(|list| list);

        cx.update_global::<Remote, _>(|remote, _| match result {
            Ok((sessions, offers)) => {
                remote.host_sessions.insert(id.clone(), sessions);

                match offers {
                    Some(offers) => remote.host_offers.insert(id, offers),
                    None => remote.host_offers.remove(&id),
                };
            }
            Err(error) => remote.report(Err(error)),
        });
    })
    .detach();
}

/// Start an agent on a paired host from one of the profiles and workspaces
/// it offers, and open a tab following it in `window`. The agent runs in a
/// host tab, so closing this view leaves it running there.
pub(crate) fn open_agent(
    id: &DeviceId,
    profile: &AgentProfileInfo,
    workspace: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(app) = app_window(window, cx) else {
        return;
    };

    let Some(kind) = AgentKind::from_id(&profile.harness) else {
        cx.global_mut::<Remote>()
            .report(Err(anyhow!(t!("remote-agent-unknown").into_owned())));

        return;
    };

    let connection = match connection(id, cx) {
        Ok(connection) => connection,
        Err(error) => {
            cx.global_mut::<Remote>().report(Err(error));

            return;
        }
    };

    cx.global_mut::<Remote>().busy = true;

    let opened = Arc::clone(&connection);
    let name = profile.name.clone();
    let title = profile.name.clone();

    let task = runtime().spawn(async move { opened.open_agent(name, workspace).await });

    window
        .spawn(cx, async move |cx| {
            let result = task
                .await
                .context("starting stopped")
                .and_then(|opened| opened);

            let _ = cx.update(|window, cx| match result {
                Ok(session) => {
                    cx.global_mut::<Remote>().report(Ok(()));

                    let (owner, pane) = agent_remote::open(connection, session, kind, window, cx);

                    app.update(cx, |app, cx| {
                        app.open_remote_agent_tab(owner, pane, title, window, cx)
                    });
                }
                Err(error) => cx.global_mut::<Remote>().report(Err(error)),
            });
        })
        .detach();
}

/// The name a profile goes by for paired devices: its own, or its agent's
/// for a profile left unnamed.
fn offered_profile_name(profile: &AgentProfile) -> String {
    if profile.name.trim().is_empty() {
        profile.kind.display().to_owned()
    } else {
        profile.name.clone()
    }
}

/// Answer a paired device's request of the application. The most recently
/// active window serves it, since that is where the person at the host
/// works.
fn answer_host(request: HostRequest, cx: &mut App) {
    match request {
        HostRequest::Info { reply } => {
            let _ = reply.send(host_offers(cx));
        }
        HostRequest::OpenAgent { params, reply } => {
            let _ = reply.send(open_agent_for_device(params, cx));
        }
    }
}

fn host_offers(cx: &mut App) -> Result<Value, String> {
    let agents = cx
        .global::<AppSettings>()
        .config()
        .agent_profiles
        .list
        .iter()
        .map(|profile| {
            let harness: &str = profile.kind.into();

            AgentProfileInfo {
                name: offered_profile_name(profile),
                harness: harness.to_owned(),
            }
        })
        .collect();

    let workspaces = last_active_window(cx)
        .and_then(|(_, view)| view.upgrade())
        .map(|view| view.read(cx).device_workspaces())
        .unwrap_or_default()
        .into_iter()
        .map(|(name, path)| WorkspaceInfo { name, path })
        .collect();

    serde_json::to_value(HostInfo { agents, workspaces }).map_err(|error| error.to_string())
}

fn open_agent_for_device(params: Value, cx: &mut App) -> Result<Value, String> {
    let AgentOpen { profile, workspace } =
        serde_json::from_value(params).map_err(|error| error.to_string())?;

    let profile = cx
        .global::<AppSettings>()
        .config()
        .agent_profiles
        .list
        .iter()
        .find(|candidate| offered_profile_name(candidate) == profile)
        .cloned()
        .ok_or_else(|| format!("no agent profile named {profile}"))?;

    let (handle, view) = last_active_window(cx).ok_or("no window is open")?;

    let session = handle
        .update(cx, |_, window, cx| {
            view.update(cx, |app, cx| {
                app.open_agent_tab_for_device(&profile, &workspace, window, cx)
            })
        })
        .ok()
        .and_then(Result::ok)
        .flatten()
        .ok_or_else(|| format!("{workspace} is not a workspace on this computer"))?;

    serde_json::to_value(SessionRef { session }).map_err(|error| error.to_string())
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
