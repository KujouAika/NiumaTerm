//! Remote sessions in the application: hosting for paired devices, pairing
//! with other computers, and opening their terminals in tabs. Network work
//! runs on the shared runtime; results come back to this global, whose
//! observers (the settings page) refresh.

use std::collections::HashMap;
use std::env;
use std::net::UdpSocket;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use gpui::{App, BorrowAppContext as _, Entity, Global, SharedString, Window};
use gpui_component::Root;
use nmt_platform::runtime;
use nmt_remote::client::{RemoteHost, connect, pair};
use nmt_remote::host::{DEFAULT_PORT, HostConfig, HostService};
use nmt_remote::store::{
    PairedDevice, PairedHost, load_hosts, load_or_create_identity, remote_dir, save_hosts,
};
use nmt_remote_core::identity::{DeviceId, DeviceKey};
use nmt_remote_core::messages::{DeviceInfo, DeviceKind};
use nmt_remote_core::pairing::{PairingCode, PairingLink};
use rust_i18n::t;
use tokio::sync::mpsc;
use tracing::warn;

use crate::ui::{AppSettings, AppWindow};

const APP_VERSION: &str = env!("NIUMATERM_VERSION");

/// Grid a new remote terminal starts with; the tab's layout resizes it.
const INITIAL_GRID: (u16, u16) = (100, 30);

#[derive(Default)]
pub(crate) struct Remote {
    key: Option<Arc<DeviceKey>>,
    host: Option<HostService>,
    hosts: Vec<PairedHost>,
    connections: HashMap<DeviceId, Arc<RemoteHost>>,

    /// Drafts of the "connect to a computer" form.
    pub(crate) address: SharedString,

    pub(crate) code: SharedString,

    /// The outcome of the last action, shown on the settings page.
    status: Option<SharedString>,

    busy: bool,
}

impl Global for Remote {}

/// Install the global and start hosting when the setting is on.
pub(crate) fn initialize(cx: &mut App) {
    cx.set_global(Remote {
        hosts: load_hosts(&remote_dir()),
        ..Remote::default()
    });

    sync_hosting(cx);
}

/// Start or stop hosting to match the setting. Runs on every settings
/// change; a host that failed to start is retried on the next one.
pub(crate) fn sync_hosting(cx: &mut App) {
    let enabled = cx.global::<AppSettings>().config().remote.enabled;

    if enabled != cx.global::<Remote>().host.is_some() {
        set_hosting(enabled, cx);
    }
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

    pub(crate) fn devices(&self) -> Vec<PairedDevice> {
        self.host
            .as_ref()
            .map(HostService::devices)
            .unwrap_or_default()
    }

    pub(crate) fn hosts(&self) -> &[PairedHost] {
        &self.hosts
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
        cx.global_mut::<Remote>().host = None;

        return;
    }

    let result = start_host(cx).map(|host| cx.global_mut::<Remote>().host = Some(host));

    cx.global_mut::<Remote>().report(result);
}

fn start_host(cx: &mut App) -> Result<HostService> {
    let key = device_key(cx)?;
    let config = cx.global::<AppSettings>().config();
    let (shell, args) = cx.global::<AppSettings>().default_profile_command();

    // Pairing changes arrive on a runtime thread; touching the global from
    // the UI thread notifies its observers, which a runtime thread cannot do.
    let (changed, mut changes) = mpsc::unbounded_channel();

    cx.spawn(async move |cx| {
        while changes.recv().await.is_some() {
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

            (link.code, address, Some(link.host_key))
        })
    } else {
        PairingCode::parse(&remote.code).map(|code| {
            let address = (!typed_address.is_empty()).then(|| with_default_port(typed_address));

            (code, address, None)
        })
    };

    let (code, address, expected_host_key) = match parsed {
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
        pair(address.as_deref(), &code, &key, device, expected_host_key).await
    });

    cx.spawn(async move |cx| {
        let result = task
            .await
            .context("pairing stopped")
            .and_then(|paired| paired);

        cx.update_global::<Remote, _>(|remote, _| match result {
            Ok(paired) => {
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
    remote.connections.remove(id);

    let saved = save_hosts(&remote_dir(), &remote.hosts);

    remote.report(saved.map_err(Into::into));
}

/// Open a terminal on a paired host in a new tab of `window`.
pub(crate) fn open_terminal(id: &DeviceId, window: &mut Window, cx: &mut App) {
    let Some(app) = app_window(window, cx) else {
        return;
    };

    let key = match device_key(cx) {
        Ok(key) => key,
        Err(error) => {
            cx.global_mut::<Remote>().report(Err(error));

            return;
        }
    };

    let remote = cx.global_mut::<Remote>();

    let Some(mut host) = remote.hosts.iter().find(|host| &host.id == id).cloned() else {
        return;
    };

    let existing = remote
        .connections
        .get(id)
        .filter(|connection| connection.is_connected())
        .cloned();

    remote.busy = true;

    remote.status = Some(
        t!("remote-connecting-to", name = &host.name)
            .into_owned()
            .into(),
    );

    let task = runtime().spawn(async move {
        let connection = match existing {
            Some(connection) => connection,
            None => connect(&mut host, &key, APP_VERSION).await?,
        };

        let pty = connection
            .open_terminal(INITIAL_GRID.0, INITIAL_GRID.1)
            .await?;

        anyhow::Ok((host, connection, pty))
    });

    window
        .spawn(cx, async move |cx| {
            let result = task
                .await
                .context("connecting stopped")
                .and_then(|opened| opened);

            let _ = cx.update(|window, cx| match result {
                Ok((host, connection, pty)) => {
                    let name = host.name.clone();
                    let remote = cx.global_mut::<Remote>();

                    remote.connections.insert(host.id.clone(), connection);

                    if let Some(saved) = remote.hosts.iter_mut().find(|saved| saved.id == host.id) {
                        *saved = host;
                    }

                    let saved = save_hosts(&remote_dir(), &remote.hosts);

                    remote.report(saved.map_err(Into::into));

                    app.update(cx, |app, cx| {
                        app.open_remote_terminal(pty, name, window, cx)
                    });
                }
                Err(error) => cx.global_mut::<Remote>().report(Err(error)),
            });
        })
        .detach();
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
