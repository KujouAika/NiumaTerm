//! A client's lasting connection to one paired host.
//!
//! Sessions belong to the host and survive network loss, so the client
//! keeps its views and reconnects under them: after a drop it backs off
//! from 0.5 s to 30 s with jitter, and on success reattaches every open view
//! by session id. A reattached view starts over from a checkpoint, which is
//! why views are addressed by session rather than by stream: stream ids
//! last only as long as one channel.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use getrandom::fill;
use nmt_platform::runtime;
use nmt_remote_core::frame::{CONTROL_STREAM, Message as FrameMessage, kind};
use nmt_remote_core::identity::{DeviceId, DeviceKey};
use nmt_remote_core::rpc::{
    self, Control, ErrorCode, RpcError, SessionInfo, SessionList, SessionRef, StreamRef,
    TerminalOpen, TerminalResize,
};
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::select;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::{Notify, oneshot, watch};
use tokio::task::AbortHandle;
use tokio::time::{Instant, sleep, timeout};
use tracing::{debug, info};

use crate::client::{Refused, establish};
use crate::link::{Outbound, SendQueue, pump};
use crate::network_pty::NetworkPty;
use crate::store::PairedHost;

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A link with no views closes after this long, so a host that is no longer
/// used stops holding a socket open.
const IDLE_CLOSE: Duration = Duration::from_secs(60);

/// How long a request waits for a link before failing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// No view needs the host; nothing is connected.
    Idle,
    Connecting,
    Connected,
    /// The link dropped; views wait while it comes back.
    Reconnecting,
    /// The host no longer trusts this device. Pairing again is the fix.
    Refused,
}

/// What a terminal stream delivers to its [`NetworkPty`].
pub(crate) enum StreamEvent {
    Output(Vec<u8>),
    /// A full replay of the terminal that replaces everything before it.
    Checkpoint(Vec<u8>),
    /// The host PTY now has this size, set by another view.
    Size(u16, u16),
    Exit,
}

pub struct RemoteHost {
    id: DeviceId,
    key: Arc<DeviceKey>,
    app_version: String,
    record: Mutex<PairedHost>,

    /// Called with the host record after each connection, which updates its
    /// address, name, and handshake time, so the caller can store it.
    on_record: Box<dyn Fn(PairedHost) + Send + Sync>,

    link: Mutex<Option<Link>>,
    views: Mutex<HashMap<String, View>>,
    status: watch::Sender<Status>,

    /// Bumped when the host reports its session list changed.
    sessions: watch::Sender<u64>,

    /// Wakes the supervisor: a view or request needs a link now.
    wake: Notify,

    next_id: AtomicU64,
    supervisor: Mutex<Option<AbortHandle>>,
}

#[derive(Clone)]
struct Link {
    queue: SendQueue,
    calls: Arc<Mutex<HashMap<u64, PendingCall>>>,
    streams: Arc<Mutex<HashMap<u32, String>>>,
}

struct PendingCall {
    reply: oneshot::Sender<Result<Value, RpcError>>,

    /// For an attach: the session to route the new stream to. Routing it
    /// while handling the response, before the next frame, means the
    /// checkpoint that follows never arrives for an unknown stream.
    attach_for: Option<String>,
}

struct View {
    events: UnboundedSender<StreamEvent>,
    stream: Option<u32>,

    /// Input typed before the view's first attach completes, sent once it
    /// does, so typing right after opening a tab loses nothing. After that,
    /// input during a reconnect is dropped rather than replayed later into a
    /// terminal whose state the user no longer sees.
    early_input: Option<Vec<u8>>,
}

/// Early input kept per view; a paste larger than this before the first
/// attach is cut.
const MAX_EARLY_INPUT: usize = 64 * 1024;

#[derive(Deserialize)]
struct AttachReply {
    stream: u32,
}

impl RemoteHost {
    pub fn new(
        record: PairedHost,
        key: Arc<DeviceKey>,
        app_version: String,
        on_record: impl Fn(PairedHost) + Send + Sync + 'static,
    ) -> Arc<Self> {
        let host = Arc::new(Self {
            id: record.id.clone(),
            key,
            app_version,
            record: Mutex::new(record),
            on_record: Box::new(on_record),
            link: Mutex::new(None),
            views: Mutex::new(HashMap::new()),
            status: watch::channel(Status::Idle).0,
            sessions: watch::channel(0).0,
            wake: Notify::new(),
            next_id: AtomicU64::new(1),
            supervisor: Mutex::new(None),
        });

        let task = runtime().spawn(Arc::clone(&host).supervise());

        *host.supervisor.lock() = Some(task.abort_handle());

        host
    }

    pub fn id(&self) -> &DeviceId {
        &self.id
    }

    pub fn name(&self) -> String {
        self.record.lock().name.clone()
    }

    pub fn status(&self) -> watch::Receiver<Status> {
        self.status.subscribe()
    }

    /// Bumped whenever the host's session list changes.
    pub fn session_changes(&self) -> watch::Receiver<u64> {
        self.sessions.subscribe()
    }

    /// Stop the connection for good, as when the host is forgotten.
    pub fn shutdown(&self) {
        if let Some(task) = self.supervisor.lock().take() {
            task.abort();
        }

        *self.link.lock() = None;

        for (_, view) in self.views.lock().drain() {
            let _ = view.events.send(StreamEvent::Exit);
        }
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let list: SessionList = self.call(rpc::SESSIONS_LIST, &Value::Null).await?;

        Ok(list.sessions)
    }

    /// Start a terminal on the host and open a view of it.
    pub async fn open_terminal(self: &Arc<Self>, cols: u16, rows: u16) -> Result<NetworkPty> {
        let SessionRef { session } = self
            .call(rpc::TERMINAL_OPEN, &TerminalOpen { cols, rows })
            .await?;

        Ok(self.view(session))
    }

    /// A view of an existing session. It attaches now if connected and
    /// otherwise as soon as a link is up, and again after every reconnect.
    pub fn view(self: &Arc<Self>, session: String) -> NetworkPty {
        let (events, events_rx) = mpsc::unbounded_channel();

        self.views.lock().insert(
            session.clone(),
            View {
                events,
                stream: None,
                early_input: Some(Vec::new()),
            },
        );

        if let Some(link) = self.link.lock().clone() {
            self.attach(&link, session.clone());
        } else {
            self.wake.notify_one();
        }

        NetworkPty::new(Arc::clone(self), session, events_rx)
    }

    /// End a session on the host. Host tabs refuse: they belong to the
    /// person at the host.
    pub fn terminate(&self, session: &str) {
        self.notify(
            rpc::TERMINAL_CLOSE,
            &SessionRef {
                session: session.to_owned(),
            },
        );
    }

    pub(crate) fn send_input(&self, session: &str, bytes: Vec<u8>) {
        let stream = {
            let mut views = self.views.lock();

            let Some(view) = views.get_mut(session) else {
                return;
            };

            match (view.stream, &mut view.early_input) {
                (Some(stream), _) => stream,
                (None, Some(early)) => {
                    let room = MAX_EARLY_INPUT.saturating_sub(early.len());

                    early.extend_from_slice(&bytes[..bytes.len().min(room)]);

                    return;
                }
                // Input typed while the link is down has no terminal to go
                // to; the tab shows that it is reconnecting.
                (None, None) => return,
            }
        };

        if let Some(link) = self.link.lock().as_ref() {
            link.queue.send(Outbound::new(stream, kind::INPUT, bytes));
        }
    }

    pub(crate) fn resize(&self, session: &str, cols: u16, rows: u16) {
        self.notify(
            rpc::TERMINAL_RESIZE,
            &TerminalResize {
                session: session.to_owned(),
                cols,
                rows,
            },
        );
    }

    /// The view closed; the session keeps running on the host.
    pub(crate) fn detach(&self, session: &str) {
        let Some(view) = self.views.lock().remove(session) else {
            return;
        };

        if let Some(stream) = view.stream {
            if let Some(link) = self.link.lock().as_ref() {
                link.streams.lock().remove(&stream);
            }

            self.notify(rpc::STREAM_CLOSE, &StreamRef { stream });
        }
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, params: &impl Serialize) -> Result<T> {
        let value = self
            .call_raw(method, params, None)
            .await?
            .map_err(|error| anyhow!("{method}: {}", error.message))?;

        Ok(serde_json::from_value(value)?)
    }

    async fn call_raw(
        &self,
        method: &str,
        params: &impl Serialize,
        attach_for: Option<String>,
    ) -> Result<Result<Value, RpcError>> {
        let link = self.wait_for_link().await?;
        let (reply, response) = oneshot::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        link.calls
            .lock()
            .insert(id, PendingCall { reply, attach_for });

        send_request(&link, id, method, params)?;

        response.await.map_err(|_| anyhow!("the connection closed"))
    }

    /// Send a request whose answer nobody waits for; the dispatcher drops
    /// answers to unknown ids. Nothing is sent without a link.
    fn notify(&self, method: &str, params: &impl Serialize) {
        if let Some(link) = self.link.lock().as_ref() {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);

            let _ = send_request(link, id, method, params);
        }
    }

    async fn wait_for_link(&self) -> Result<Link> {
        let mut status = self.status.subscribe();

        let deadline = Instant::now() + REQUEST_TIMEOUT;

        loop {
            if let Some(link) = self.link.lock().clone() {
                return Ok(link);
            }

            if *status.borrow() == Status::Refused {
                return Err(Refused.into());
            }

            self.wake.notify_one();

            if timeout(deadline - Instant::now(), status.changed())
                .await
                .is_err()
            {
                return Err(anyhow!("{} is not reachable", self.name()));
            }
        }
    }

    fn attach(self: &Arc<Self>, link: &Link, session: String) {
        let host = Arc::clone(self);
        let link = link.clone();

        // Views open from the UI thread, which has no runtime context.
        runtime().spawn(async move {
            let (reply, response) = oneshot::channel();
            let id = host.next_id.fetch_add(1, Ordering::Relaxed);

            link.calls.lock().insert(
                id,
                PendingCall {
                    reply,
                    attach_for: Some(session.clone()),
                },
            );

            let request = SessionRef {
                session: session.clone(),
            };

            if send_request(&link, id, rpc::TERMINAL_ATTACH, &request).is_err() {
                return;
            }

            // A session that ended while this view was away is over.
            if let Ok(Err(error)) = response.await
                && error.code == ErrorCode::NotFound
                && let Some(view) = host.views.lock().remove(&session)
            {
                let _ = view.events.send(StreamEvent::Exit);
            }
        });
    }

    async fn supervise(self: Arc<Self>) {
        let mut backoff = MIN_BACKOFF;
        let mut ever_connected = false;

        loop {
            if self.views.lock().is_empty() {
                self.status.send_replace(Status::Idle);

                self.wake.notified().await;
            }

            self.status.send_replace(if ever_connected {
                Status::Reconnecting
            } else {
                Status::Connecting
            });

            let mut record = self.record.lock().clone();

            match establish(&mut record, &self.key, &self.app_version).await {
                Ok((ws, channel)) => {
                    *self.record.lock() = record.clone();

                    (self.on_record)(record);

                    backoff = MIN_BACKOFF;
                    ever_connected = true;

                    let (queue, queue_rx) = SendQueue::new();
                    let (inbound, inbound_rx) = mpsc::unbounded_channel();

                    let link = Link {
                        queue,
                        calls: Arc::default(),
                        streams: Arc::default(),
                    };

                    let pump = tokio::spawn(pump(ws, channel, queue_rx, inbound));

                    *self.link.lock() = Some(link.clone());

                    self.status.send_replace(Status::Connected);

                    info!(host = %self.id, "connected to the remote host");

                    let sessions: Vec<_> = self.views.lock().keys().cloned().collect();

                    for session in sessions {
                        self.attach(&link, session);
                    }

                    self.dispatch(&link, inbound_rx).await;

                    pump.abort();

                    *self.link.lock() = None;

                    link.calls.lock().clear();

                    for view in self.views.lock().values_mut() {
                        view.stream = None;
                    }

                    info!(host = %self.id, "the remote host link closed");
                }
                Err(error) if error.is::<Refused>() => {
                    self.status.send_replace(Status::Refused);

                    for (_, view) in self.views.lock().drain() {
                        let _ = view.events.send(StreamEvent::Exit);
                    }

                    return;
                }
                Err(error) => {
                    debug!(host = %self.id, %error, "connecting to the remote host failed");

                    // A new view or request retries at once; otherwise the
                    // backoff spreads retries of many clients apart.
                    select! {
                        () = sleep(jitter(backoff)) => {}

                        () = self.wake.notified() => {}
                    }

                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }

    /// Route one link's inbound messages until it closes or goes idle.
    async fn dispatch(&self, link: &Link, mut inbound: UnboundedReceiver<FrameMessage>) {
        let mut idle_since: Option<Instant> = None;

        loop {
            let message = select! {
                message = inbound.recv() => message,
                () = sleep(Duration::from_secs(5)) => {
                    let idle = self.views.lock().is_empty() && link.calls.lock().is_empty();

                    match (idle, idle_since) {
                        (false, _) => idle_since = None,
                        (true, None) => idle_since = Some(Instant::now()),
                        (true, Some(since)) if since.elapsed() >= IDLE_CLOSE => return,
                        (true, Some(_)) => {}
                    }

                    continue;
                }
            };

            let Some(message) = message else {
                return;
            };

            self.route(link, message);
        }
    }

    fn route(&self, link: &Link, message: FrameMessage) {
        match (message.stream, message.kind) {
            (CONTROL_STREAM, kind::CONTROL_JSON) => match Control::decode(&message.payload) {
                Ok(Control::Response { id, outcome }) => {
                    let Some(call) = link.calls.lock().remove(&id) else {
                        return;
                    };

                    if let (Ok(result), Some(session)) = (&outcome, &call.attach_for)
                        && let Ok(AttachReply { stream }) =
                            serde_json::from_value::<AttachReply>(result.clone())
                        && let Some(view) = self.views.lock().get_mut(session)
                    {
                        view.stream = Some(stream);

                        link.streams.lock().insert(stream, session.clone());

                        if let Some(early) = view.early_input.take()
                            && !early.is_empty()
                        {
                            link.queue.send(Outbound::new(stream, kind::INPUT, early));
                        }
                    }

                    let _ = call.reply.send(outcome);
                }
                Ok(Control::Notification { method, .. }) if method == rpc::SESSIONS_CHANGED => {
                    self.sessions.send_modify(|version| *version += 1);
                }
                _ => {}
            },
            (stream, kind) => {
                let Some(session) = link.streams.lock().get(&stream).cloned() else {
                    return;
                };

                let event = match kind {
                    kind::OUTPUT => StreamEvent::Output(message.payload),
                    kind::CHECKPOINT => StreamEvent::Checkpoint(message.payload),
                    kind::SIZE => match serde_json::from_slice::<Size>(&message.payload) {
                        Ok(size) => StreamEvent::Size(size.cols, size.rows),
                        Err(_) => return,
                    },
                    kind::EXIT => {
                        link.streams.lock().remove(&stream);

                        if let Some(view) = self.views.lock().remove(&session) {
                            let _ = view.events.send(StreamEvent::Exit);
                        }

                        return;
                    }
                    // Unknown frame kinds come from a newer peer.
                    _ => return,
                };

                if let Some(view) = self.views.lock().get(&session) {
                    let _ = view.events.send(event);
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct Size {
    cols: u16,
    rows: u16,
}

fn send_request(link: &Link, id: u64, method: &str, params: &impl Serialize) -> Result<()> {
    let request = Control::Request {
        id,
        method: method.to_owned(),
        params: serde_json::to_value(params)?,
    };

    if link.queue.send(Outbound::new(
        CONTROL_STREAM,
        kind::CONTROL_JSON,
        request.encode(),
    )) {
        Ok(())
    } else {
        Err(anyhow!("the connection closed"))
    }
}

/// Up to a quarter more or less than `delay`, so clients that lost the same
/// host at the same moment do not retry in lockstep.
fn jitter(delay: Duration) -> Duration {
    let mut byte = [0u8; 1];

    let _ = fill(&mut byte);

    let scale = 0.75 + f64::from(byte[0]) / 255.0 * 0.5;

    delay.mul_f64(scale)
}
