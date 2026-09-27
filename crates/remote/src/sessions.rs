//! The terminals a host offers to paired devices: the tabs open on the host
//! and the headless terminals remote devices started. Sessions outlive the
//! channels and views attached to them.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use anyhow::{Result, anyhow};
use nmt_config::{CursorShape, active_colors};
use nmt_platform::{PtyOptions, WinsizeBuilder, create_pty_with_env, default_shell};
use nmt_remote_core::rpc::{Origin, SessionInfo};
use nmt_terminal::event::{EventListener, Msg, MsgSender, TerminalEvent};
use nmt_terminal::session::TerminalSessionConfig;
use nmt_terminal::termio::{SessionOptions, SessionWorker, start_session};
use parking_lot::Mutex;
use tokio::sync::watch;
use tracing::info;

/// What the host service needs to drive one session from a runtime thread,
/// without passing through the UI thread that owns a tab.
#[derive(Clone)]
pub struct TerminalControl {
    pub messenger: MsgSender,

    /// Set when a remote view resized the PTY. The host tab showing the
    /// session takes the size back the next time its user types, so the
    /// view in active use always sees the PTY at its own size.
    pub claimed_remotely: Arc<AtomicBool>,
}

pub struct SessionRegistry {
    inner: Mutex<Inner>,
    changed: watch::Sender<u64>,
}

#[derive(Default)]
struct Inner {
    sessions: BTreeMap<String, Entry>,
    next: u64,
}

struct Entry {
    info: SessionInfo,
    control: TerminalControl,

    /// The PTY size, watched by every attached remote view.
    size: watch::Sender<(u16, u16)>,

    /// Held for headless sessions only; dropping it ends the shell. Tab
    /// sessions belong to the tab.
    _worker: Option<SessionWorker>,
}

/// Events of a headless terminal. Nobody on the host is looking at it, so
/// bell, clipboard, and notification requests are dropped; the shell ending
/// removes the session.
struct HeadlessEvents {
    registry: Weak<SessionRegistry>,
    session: String,
}

impl SessionRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
            changed: watch::channel(0).0,
        })
    }

    /// Offer a host tab's terminal. Returns its session id.
    pub fn register_tab(
        &self,
        title: String,
        cols: u16,
        rows: u16,
        control: TerminalControl,
    ) -> String {
        self.insert(Origin::Tab, title, cols, rows, control, None)
    }

    pub fn unregister(&self, session: &str) {
        let removed = self.inner.lock().sessions.remove(session);

        drop(removed);

        self.notify();
    }

    pub fn set_title(&self, session: &str, title: String) {
        let mut inner = self.inner.lock();

        let Some(entry) = inner.sessions.get_mut(session) else {
            return;
        };

        if entry.info.title == title {
            return;
        }

        entry.info.title = title;

        drop(inner);

        self.notify();
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        self.inner
            .lock()
            .sessions
            .values()
            .map(|entry| entry.info.clone())
            .collect()
    }

    /// Headless sessions, which the host user can end.
    pub fn remote_sessions(&self) -> Vec<SessionInfo> {
        self.list()
            .into_iter()
            .filter(|info| info.origin == Origin::Remote)
            .collect()
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// What a remote view needs to attach: the session's control handle and
    /// its PTY size, current and future.
    pub(crate) fn attach_info(
        &self,
        session: &str,
    ) -> Option<(TerminalControl, watch::Receiver<(u16, u16)>)> {
        self.inner
            .lock()
            .sessions
            .get(session)
            .map(|entry| (entry.control.clone(), entry.size.subscribe()))
    }

    /// A host tab resized its session's PTY, or took the size back from a
    /// remote view. Remote views hear about it.
    pub fn note_local_resize(&self, session: &str, cols: u16, rows: u16) {
        let mut inner = self.inner.lock();

        let Some(entry) = inner.sessions.get_mut(session) else {
            return;
        };

        entry.info.cols = cols;
        entry.info.rows = rows;

        entry
            .control
            .claimed_remotely
            .store(false, Ordering::Relaxed);

        entry.size.send_if_modified(|size| {
            let changed = *size != (cols, rows);

            *size = (cols, rows);

            changed
        });
    }

    pub fn origin(&self, session: &str) -> Option<Origin> {
        self.inner
            .lock()
            .sessions
            .get(session)
            .map(|entry| entry.info.origin)
    }

    /// Resize a session's PTY for a remote view.
    pub(crate) fn resize(&self, session: &str, cols: u16, rows: u16) -> bool {
        let mut inner = self.inner.lock();

        let Some(entry) = inner.sessions.get_mut(session) else {
            return false;
        };

        entry.info.cols = cols;
        entry.info.rows = rows;

        entry
            .control
            .claimed_remotely
            .store(true, Ordering::Relaxed);

        entry.size.send_replace((cols, rows));

        // The host engine derives its cell size from these pixels for image
        // placement; a remote view's real cell size is unknown here, so a
        // typical one stands in.
        entry
            .control
            .messenger
            .send(Msg::Resize(WinsizeBuilder {
                cols,
                rows,
                width: cols.saturating_mul(8),
                height: rows.saturating_mul(16),
            }))
            .is_ok()
    }

    /// End a headless session. Host tabs are refused: they belong to the
    /// person at the host.
    pub fn close_remote(&self, session: &str) -> bool {
        let mut inner = self.inner.lock();

        if !inner
            .sessions
            .get(session)
            .is_some_and(|entry| entry.info.origin == Origin::Remote)
        {
            return false;
        }

        let removed = inner.sessions.remove(session);

        drop(inner);

        info!(%session, "closing a remote terminal");

        drop(removed);

        self.notify();

        true
    }

    /// End every headless session, as the host stops.
    pub(crate) fn close_all_remote(&self) {
        let removed: Vec<_> = {
            let mut inner = self.inner.lock();

            let ids: Vec<_> = inner
                .sessions
                .iter()
                .filter(|(_, entry)| entry.info.origin == Origin::Remote)
                .map(|(id, _)| id.clone())
                .collect();

            ids.iter()
                .filter_map(|id| inner.sessions.remove(id))
                .collect()
        };

        drop(removed);

        self.notify();
    }

    /// Start a headless terminal for a remote device with the host's shell.
    pub(crate) fn open_headless(
        self: &Arc<Self>,
        shell: Option<String>,
        args: Vec<String>,
        cols: u16,
        rows: u16,
    ) -> Result<String> {
        let config = TerminalSessionConfig {
            shell,
            args,
            ..TerminalSessionConfig::default()
        }
        .with_shell_integration();

        let program = config.shell.clone().unwrap_or_else(default_shell);
        let home = dirs::home_dir().map(|home| home.to_string_lossy().into_owned());

        let pty = create_pty_with_env(PtyOptions {
            shell: &program,
            args: &config.args,
            working_directory: home.as_deref(),
            columns: cols,
            rows,
            environment_overrides: &config.environment_overrides,
            starting_title: None,
            bootstrap: config.bootstrap.as_deref(),
        })
        .map_err(|error| anyhow!("starting {program}: {error}"))?;

        let session = {
            let mut inner = self.inner.lock();

            inner.next += 1;

            format!("r{}", inner.next)
        };

        // The host engine answers terminal queries next to the PTY, so the
        // program gets exactly one reply without a network round trip.
        let handles = start_session(
            pty,
            HeadlessEvents {
                registry: Arc::downgrade(self),
                session: session.clone(),
            },
            SessionOptions {
                cols,
                rows,
                route_id: 0,
                colors: active_colors(),
                cursor_shape: CursorShape::Block,
                scrollback_lines: config.scrollback_lines,
                engine_blocks: config.engine_blocks,
                terminal_responses: true,
            },
        )
        .map_err(|error| anyhow!("starting the terminal engine: {error}"))?;

        let title = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&program)
            .to_owned();

        let control = TerminalControl {
            messenger: handles.messenger,
            claimed_remotely: Arc::new(AtomicBool::new(false)),
        };

        self.insert_with_id(
            session.clone(),
            Origin::Remote,
            title,
            cols,
            rows,
            control,
            Some(handles.worker),
        );

        info!(%session, cols, rows, "opened a remote terminal");

        Ok(session)
    }

    fn insert(
        &self,
        origin: Origin,
        title: String,
        cols: u16,
        rows: u16,
        control: TerminalControl,
        worker: Option<SessionWorker>,
    ) -> String {
        let session = {
            let mut inner = self.inner.lock();

            inner.next += 1;

            format!("t{}", inner.next)
        };

        self.insert_with_id(session.clone(), origin, title, cols, rows, control, worker);

        session
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_with_id(
        &self,
        session: String,
        origin: Origin,
        title: String,
        cols: u16,
        rows: u16,
        control: TerminalControl,
        worker: Option<SessionWorker>,
    ) {
        let entry = Entry {
            info: SessionInfo {
                session: session.clone(),
                title,
                origin,
                cols,
                rows,
            },
            control,
            size: watch::channel((cols, rows)).0,
            _worker: worker,
        };

        self.inner.lock().sessions.insert(session, entry);

        self.notify();
    }

    fn notify(&self) {
        self.changed.send_modify(|version| *version += 1);
    }
}

impl EventListener for HeadlessEvents {
    fn send_event(&self, event: TerminalEvent) {
        if let TerminalEvent::CloseTerminal(_) = event
            && let Some(registry) = self.registry.upgrade()
        {
            registry.unregister(&self.session);
        }
    }
}
