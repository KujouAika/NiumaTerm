//! The host side: accepts paired devices on the LAN and runs the terminals
//! they open.
//!
//! Every connection starts with the version preface and then either pairs
//! (only while a code is showing) or runs the IK channel handshake, which
//! answers only devices in the trust store. Terminals belong to the host,
//! not to a connection: a dropped connection leaves them running.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use futures::{Sink, Stream};
use nmt_config::{CursorShape, active_colors};
use nmt_platform::{PtyOptions, WinsizeBuilder, create_pty_with_env, default_shell, runtime};
use nmt_remote_core::channel::{Channel, HostHandshake};
use nmt_remote_core::frame::{CONTROL_STREAM, Message as FrameMessage, kind};
use nmt_remote_core::identity::{DeviceId, DeviceKey};
use nmt_remote_core::messages::{DeviceInfo, HostHello, PairAccepted};
use nmt_remote_core::pairing::{HostPairing, IssuedCode, PairingCode};
use nmt_remote_core::preface::{Preface, PrefaceKind};
use nmt_remote_core::rpc::{
    self, Attached, Control, ErrorCode, Exit, RpcError, SessionRef, TerminalOpen, TerminalResize,
};
use nmt_remote_core::{Error as CoreError, PROTO_MINOR};
use nmt_terminal::event::{
    CheckpointRequest, EventListener, Msg, MsgSender, OutputSink, TerminalEvent,
};
use nmt_terminal::session::TerminalSessionConfig;
use nmt_terminal::termio::{SessionOptions, SessionWorker, start_session};
use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::timeout;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tracing::{debug, warn};

use crate::link::{Outbound, pump, recv_binary, send_binary};
use crate::store::{self, PairedDevice, now_ms};

pub const DEFAULT_PORT: u16 = 47470;

/// Sockets that have not finished a handshake yet. Each costs a task and a
/// buffer, so an unauthenticated peer on the LAN cannot hold many.
const MAX_UNAUTHENTICATED: usize = 16;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

const FEATURES: &[&str] = &["terminal"];

pub struct HostConfig {
    pub port: u16,
    pub device: DeviceInfo,

    /// The shell remote terminals start, normally the default profile's.
    /// `None` uses the platform default.
    pub shell: Option<String>,

    pub args: Vec<String>,

    /// Runs on a runtime thread after pairing records change, so a view that
    /// lists them can refresh.
    pub on_change: Arc<dyn Fn() + Send + Sync>,
}

/// A running host. Dropping it stops listening, closes every channel, and
/// ends the terminals remote devices opened.
pub struct HostService {
    shared: Arc<Shared>,
    local_addr: SocketAddr,
    task: JoinHandle<()>,
}

struct Shared {
    key: DeviceKey,
    dir: PathBuf,
    config: HostConfig,
    state: Mutex<State>,
    unauthenticated: Arc<Semaphore>,
}

#[derive(Default)]
struct State {
    devices: Vec<PairedDevice>,
    code: Option<IssuedCode>,
    terminals: HashMap<String, Arc<HostTerminal>>,
    next_terminal: u64,

    /// Open channels by client key, so removing a device closes them.
    connections: Vec<([u8; 32], AbortHandle)>,
}

/// A terminal a remote device opened, running headless on the host.
struct HostTerminal {
    messenger: MsgSender,
    worker: Mutex<Option<SessionWorker>>,
    size: Mutex<(u16, u16)>,
    exit: Arc<ExitWatch>,
}

/// Delivers EXIT to every stream attached to a terminal whose shell ended,
/// including streams attached after it ended.
#[derive(Default)]
struct ExitWatch {
    exited: AtomicBool,
    watchers: Mutex<Vec<(UnboundedSender<Outbound>, u32)>>,
}

/// Host-side events of a headless terminal. Nobody on the host is looking at
/// it, so bell, clipboard, and notification requests are dropped; only the
/// shell ending matters.
struct HostEvents(Arc<ExitWatch>);

/// Per-channel request handling.
struct Connection {
    shared: Arc<Shared>,
    out: UnboundedSender<Outbound>,
    streams: HashMap<u32, Arc<HostTerminal>>,
    next_stream: u32,

    /// An attach whose response must be queued before its checkpoint.
    pending_attach: Option<(u32, Arc<HostTerminal>)>,
}

impl HostService {
    /// Start listening. `dir` holds the trust store. The port falls back to an
    /// ephemeral one when the configured port is taken.
    pub fn start(dir: PathBuf, key: DeviceKey, config: HostConfig) -> Result<Self> {
        let listener = bind(config.port)?;

        listener.set_nonblocking(true)?;

        let listener = {
            let _runtime = runtime().enter();

            TcpListener::from_std(listener)?
        };

        let local_addr = listener.local_addr()?;

        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                devices: store::load_devices(&dir),
                ..State::default()
            }),
            key,
            dir,
            config,
            unauthenticated: Arc::new(Semaphore::new(MAX_UNAUTHENTICATED)),
        });

        let task = runtime().spawn(accept_loop(Arc::clone(&shared), listener));

        Ok(Self {
            shared,
            local_addr,
            task,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn device_id(&self) -> DeviceId {
        self.shared.key.id()
    }

    /// Show a new pairing code, replacing any previous one.
    pub fn start_pairing(&self) -> Result<PairingCode> {
        let code = PairingCode::generate()?;

        self.shared.state.lock().code = Some(IssuedCode::new(code.clone(), now_ms()));

        Ok(code)
    }

    pub fn cancel_pairing(&self) {
        self.shared.state.lock().code = None;
    }

    /// The code still accepting attempts, and when it expires.
    pub fn pairing(&self) -> Option<(PairingCode, u64)> {
        let state = self.shared.state.lock();
        let issued = state.code.as_ref()?;

        issued
            .is_usable(now_ms())
            .then(|| (issued.code().clone(), issued.expires_at_ms()))
    }

    pub fn devices(&self) -> Vec<PairedDevice> {
        self.shared.state.lock().devices.clone()
    }

    /// Revoke a device: forget its key and close its open channels.
    pub fn remove_device(&self, id: &DeviceId) -> Result<()> {
        let devices = {
            let mut state = self.shared.state.lock();

            let Some(key) = state
                .devices
                .iter()
                .find(|device| &device.id == id)
                .map(|device| device.public_key)
            else {
                return Ok(());
            };

            state.devices.retain(|device| device.public_key != key);

            state.connections.retain(|(client, task)| {
                if *client == key {
                    task.abort();
                }

                *client != key
            });

            state.devices.clone()
        };

        store::save_devices(&self.shared.dir, &devices)?;

        Ok(())
    }
}

impl Drop for HostService {
    fn drop(&mut self) {
        self.task.abort();

        let mut state = self.shared.state.lock();

        for (_, task) in state.connections.drain(..) {
            task.abort();
        }

        for terminal in state.terminals.values() {
            terminal.shutdown();
        }

        state.terminals.clear();
    }
}

impl HostTerminal {
    /// Dropping the worker asks the PTY task to stop, which ends the shell.
    fn shutdown(&self) {
        self.worker.lock().take();
    }
}

impl ExitWatch {
    fn watch(&self, out: UnboundedSender<Outbound>, stream: u32) {
        let mut watchers = self.watchers.lock();

        if self.exited.load(Ordering::Acquire) {
            send_exit(&out, stream);
        } else {
            watchers.push((out, stream));
        }
    }

    fn fire(&self) {
        let mut watchers = self.watchers.lock();

        self.exited.store(true, Ordering::Release);

        for (out, stream) in watchers.drain(..) {
            send_exit(&out, stream);
        }
    }
}

impl EventListener for HostEvents {
    fn send_event(&self, event: TerminalEvent) {
        if let TerminalEvent::CloseTerminal(_) = event {
            self.0.fire();
        }
    }
}

impl Connection {
    fn handle(&mut self, message: FrameMessage) {
        match (message.stream, message.kind) {
            (CONTROL_STREAM, kind::CONTROL_JSON) => match Control::decode(&message.payload) {
                Ok(Control::Request { id, method, params }) => {
                    let outcome = self.request(&method, params);

                    self.send(
                        CONTROL_STREAM,
                        kind::CONTROL_JSON,
                        Control::Response { id, outcome }.encode(),
                    );

                    if let Some((stream, terminal)) = self.pending_attach.take() {
                        self.subscribe(stream, &terminal);
                    }
                }
                // Clients send only requests in this protocol revision.
                Ok(_) => {}
                Err(error) => debug!(%error, "ignoring an undecodable control message"),
            },
            (stream, kind::INPUT) => {
                if let Some(terminal) = self.streams.get(&stream) {
                    let _ = terminal.messenger.send(Msg::Input(message.payload.into()));
                }
            }
            // Unknown frame kinds come from a newer peer and are skipped.
            _ => {}
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            rpc::TERMINAL_OPEN => {
                let open: TerminalOpen = parse(params)?;

                let session = self
                    .shared
                    .open_terminal(open.cols.max(1), open.rows.max(1))
                    .map_err(|error| RpcError::new(ErrorCode::Internal, error.to_string()))?;

                reply(&SessionRef { session })
            }
            rpc::TERMINAL_ATTACH => {
                let SessionRef { session } = parse(params)?;
                let terminal = self.shared.terminal(&session)?;
                let stream = self.next_stream;
                let (cols, rows) = *terminal.size.lock();

                self.next_stream += 1;

                self.streams.insert(stream, Arc::clone(&terminal));

                self.pending_attach = Some((stream, terminal));

                reply(&Attached { stream, cols, rows })
            }
            rpc::TERMINAL_RESIZE => {
                let resize: TerminalResize = parse(params)?;
                let terminal = self.shared.terminal(&resize.session)?;
                let (cols, rows) = (resize.cols.max(1), resize.rows.max(1));

                *terminal.size.lock() = (cols, rows);

                // The host engine derives its cell size from these pixels for
                // image placement; a remote view's real cell size is unknown
                // here, so a typical one stands in.
                let _ = terminal.messenger.send(Msg::Resize(WinsizeBuilder {
                    cols,
                    rows,
                    width: cols.saturating_mul(8),
                    height: rows.saturating_mul(16),
                }));

                Ok(Value::Null)
            }
            rpc::TERMINAL_CLOSE => {
                let SessionRef { session } = parse(params)?;
                let terminal = self.shared.state.lock().terminals.remove(&session);

                if let Some(terminal) = terminal {
                    terminal.shutdown();

                    self.streams
                        .retain(|_, attached| !Arc::ptr_eq(attached, &terminal));
                }

                Ok(Value::Null)
            }
            _ => Err(RpcError::new(ErrorCode::Unsupported, method)),
        }
    }

    /// Register the stream for every byte after a checkpoint. The checkpoint
    /// and the live output are queued on the PTY task in order, and after the
    /// attach response, so the client sees the stream id first.
    fn subscribe(&self, stream: u32, terminal: &HostTerminal) {
        let output = self.out.clone();
        let checkpoint = self.out.clone();

        let _ = terminal.messenger.send(Msg::Subscribe {
            sink: OutputSink(Box::new(move |bytes| {
                output
                    .send(Outbound {
                        stream,
                        kind: kind::OUTPUT,
                        payload: bytes.to_vec(),
                    })
                    .is_ok()
            })),
            checkpoint: CheckpointRequest(Box::new(move |result| match result {
                Ok(state) => {
                    let _ = checkpoint.send(Outbound {
                        stream,
                        kind: kind::CHECKPOINT,
                        payload: state.vt,
                    });
                }
                Err(error) => warn!(?error, "remote attach checkpoint failed"),
            })),
        });

        terminal.exit.watch(self.out.clone(), stream);
    }

    fn send(&self, stream: u32, kind: u8, payload: Vec<u8>) {
        let _ = self.out.send(Outbound {
            stream,
            kind,
            payload,
        });
    }
}

impl Shared {
    fn terminal(&self, session: &str) -> Result<Arc<HostTerminal>, RpcError> {
        self.state
            .lock()
            .terminals
            .get(session)
            .cloned()
            .ok_or_else(|| RpcError::new(ErrorCode::NotFound, session))
    }

    fn open_terminal(&self, cols: u16, rows: u16) -> Result<String> {
        let config = TerminalSessionConfig {
            shell: self.config.shell.clone(),
            args: self.config.args.clone(),
            ..TerminalSessionConfig::default()
        }
        .with_shell_integration();

        let shell = config.shell.clone().unwrap_or_else(default_shell);
        let home = dirs::home_dir().map(|home| home.to_string_lossy().into_owned());

        let pty = create_pty_with_env(PtyOptions {
            shell: &shell,
            args: &config.args,
            working_directory: home.as_deref(),
            columns: cols,
            rows,
            environment_overrides: &config.environment_overrides,
            starting_title: None,
            bootstrap: config.bootstrap.as_deref(),
        })
        .map_err(|error| anyhow!("starting {shell}: {error}"))?;

        let exit = Arc::new(ExitWatch::default());

        // The host engine answers terminal queries next to the PTY, so the
        // program gets exactly one reply without a network round trip.
        let handles = start_session(
            pty,
            HostEvents(Arc::clone(&exit)),
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

        let terminal = Arc::new(HostTerminal {
            messenger: handles.messenger,
            worker: Mutex::new(Some(handles.worker)),
            size: Mutex::new((cols, rows)),
            exit,
        });

        let mut state = self.state.lock();

        state.next_terminal += 1;

        let session = format!("t{}", state.next_terminal);

        state.terminals.insert(session.clone(), terminal);

        Ok(session)
    }
}

fn bind(port: u16) -> Result<StdTcpListener> {
    // IPv4 covers the home and office LANs this path targets; an IPv6
    // listener joins it once DNS-SD discovery advertises both families.
    match StdTcpListener::bind((Ipv4Addr::UNSPECIFIED, port)) {
        Err(error) if error.kind() == ErrorKind::AddrInUse && port != 0 => {
            warn!(port, "remote port is taken; listening on an ephemeral port");

            Ok(StdTcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?)
        }
        result => Ok(result?),
    }
}

async fn accept_loop(shared: Arc<Shared>, listener: TcpListener) {
    loop {
        let (tcp, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                warn!(%error, "remote listener accept failed");

                continue;
            }
        };

        let Ok(permit) = Arc::clone(&shared.unauthenticated).try_acquire_owned() else {
            debug!(%peer, "dropping a connection: too many pending handshakes");

            continue;
        };

        let shared = Arc::clone(&shared);

        tokio::spawn(async move {
            if let Err(error) = serve(shared, tcp, permit).await {
                debug!(%peer, %error, "remote connection ended");
            }
        });
    }
}

async fn serve(shared: Arc<Shared>, tcp: TcpStream, permit: OwnedSemaphorePermit) -> Result<()> {
    let (mut ws, offer, answer) = timeout(HANDSHAKE_TIMEOUT, async {
        let mut ws = accept_hdr_async(tcp, check_path).await?;

        let offer = Preface::decode(&recv_binary(&mut ws).await?)?;
        let answer = offer.answer();

        send_binary(&mut ws, answer.encode().to_vec()).await?;

        anyhow::Ok((ws, offer, answer))
    })
    .await??;

    match answer.kind {
        PrefaceKind::Rejected => Ok(()),
        PrefaceKind::Pairing => {
            timeout(
                HANDSHAKE_TIMEOUT,
                serve_pairing(&shared, &mut ws, &offer, &answer),
            )
            .await?
        }
        PrefaceKind::Channel => {
            let (channel, client_key) = timeout(
                HANDSHAKE_TIMEOUT,
                accept_channel(&shared, &mut ws, &offer, &answer),
            )
            .await??;

            drop(permit);

            run_channel(shared, ws, channel, client_key).await
        }
    }
}

// tungstenite fixes this callback's signature, error type included.
#[allow(clippy::result_large_err)]
fn check_path(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    if request.uri().path() == "/v1" {
        return Ok(response);
    }

    let mut rejection = ErrorResponse::new(None);

    *rejection.status_mut() = StatusCode::NOT_FOUND;

    Err(rejection)
}

async fn serve_pairing<S>(
    shared: &Shared,
    ws: &mut S,
    offer: &Preface,
    answer: &Preface,
) -> Result<()>
where
    S: Stream<Item = Result<Message, WsError>> + Sink<Message, Error = WsError> + Unpin,
{
    let hello = recv_binary(ws).await?;

    let code = {
        let state = shared.state.lock();

        match &state.code {
            Some(issued) if issued.is_usable(now_ms()) => issued.code().clone(),
            _ => bail!("no pairing code is showing"),
        }
    };

    let (mut pairing, reply) = HostPairing::on_hello(&code, &shared.key, offer, answer, &hello)?;

    send_binary(ws, reply).await?;

    let msg1 = recv_binary(ws).await?;

    send_binary(ws, pairing.on_msg1(&msg1)?).await?;

    let msg3 = recv_binary(ws).await?;

    let request = match pairing.on_msg3(&msg3) {
        Err(CoreError::WrongCode) => {
            {
                let mut state = shared.state.lock();

                if let Some(issued) = state.code.as_mut()
                    && issued.code() == &code
                    && !issued.record_failure(now_ms())
                {
                    state.code = None;
                }
            }

            (shared.config.on_change)();

            bail!("wrong pairing code");
        }
        result => result?,
    };

    let devices = {
        let mut state = shared.state.lock();

        // Another attempt may have used or replaced the code meanwhile.
        match &state.code {
            Some(issued) if issued.code() == &code && issued.is_usable(now_ms()) => {}
            _ => bail!("the pairing code is no longer valid"),
        }

        state.code = None;

        state
            .devices
            .retain(|device| device.public_key != request.client_key);

        state.devices.push(PairedDevice::new(
            request.client_key,
            request.client.name.clone(),
            request.client.kind,
            request.client.platform.clone(),
        ));

        state.devices.clone()
    };

    store::save_devices(&shared.dir, &devices)?;

    let accepted = request.accept(&PairAccepted {
        host: shared.config.device.clone(),
        relay: None,
        lan_hints: Vec::new(),
    })?;

    send_binary(ws, accepted).await?;

    (shared.config.on_change)();

    Ok(())
}

async fn accept_channel<S>(
    shared: &Shared,
    ws: &mut S,
    offer: &Preface,
    answer: &Preface,
) -> Result<(Channel, [u8; 32])>
where
    S: Stream<Item = Result<Message, WsError>> + Sink<Message, Error = WsError> + Unpin,
{
    let msg1 = recv_binary(ws).await?;
    let pending = HostHandshake::read(&shared.key, offer, answer, &msg1)?;
    let client_key = *pending.client_key();
    let hello_ms = pending.hello().hello_ms;

    let devices = {
        let mut state = shared.state.lock();

        // An unknown or revoked key gets no reply at all.
        let Some(device) = state
            .devices
            .iter_mut()
            .find(|device| device.public_key == client_key)
        else {
            bail!("unpaired device");
        };

        if hello_ms <= device.last_hello_ms {
            bail!("replayed handshake from {}", device.id);
        }

        device.last_hello_ms = hello_ms;
        device.last_seen = now_ms();

        state.devices.clone()
    };

    if let Err(error) = store::save_devices(&shared.dir, &devices) {
        warn!(%error, "failed to record a device connection");
    }

    // Both sides speak the lower revision. The lint sees today's value of
    // 0; the rule holds for every later one.
    #[allow(clippy::unnecessary_min_or_max)]
    let proto_minor = PROTO_MINOR.min(pending.hello().proto_minor);

    let hello = HostHello {
        proto_minor,
        app_version: shared.config.device.app_version.clone(),
        features: FEATURES.iter().map(|&feature| feature.into()).collect(),
        name: shared.config.device.name.clone(),
        lan_hints: Vec::new(),
    };

    let (channel, msg2) = pending.accept(&hello)?;

    send_binary(ws, msg2).await?;

    Ok((channel, client_key))
}

async fn run_channel<S>(
    shared: Arc<Shared>,
    ws: S,
    channel: Channel,
    client_key: [u8; 32],
) -> Result<()>
where
    S: Stream<Item = Result<Message, WsError>>
        + Sink<Message, Error = WsError>
        + Unpin
        + Send
        + 'static,
{
    let (out_tx, out_rx) = mpsc::unbounded_channel();

    let (in_tx, mut in_rx) = mpsc::unbounded_channel();

    let pump = tokio::spawn(pump(ws, channel, out_rx, in_tx));
    let pump_id = pump.id();

    shared
        .state
        .lock()
        .connections
        .push((client_key, pump.abort_handle()));

    let mut connection = Connection {
        shared: Arc::clone(&shared),
        out: out_tx,
        streams: HashMap::new(),
        next_stream: 1,
        pending_attach: None,
    };

    while let Some(message) = in_rx.recv().await {
        connection.handle(message);
    }

    shared
        .state
        .lock()
        .connections
        .retain(|(_, task)| task.id() != pump_id);

    // Closing the queue ends the pump if the peer has not already.
    drop(connection);

    match pump.await {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params)
        .map_err(|error| RpcError::new(ErrorCode::InvalidParams, error.to_string()))
}

fn reply<T: Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value)
        .map_err(|error| RpcError::new(ErrorCode::Internal, error.to_string()))
}

fn send_exit(out: &UnboundedSender<Outbound>, stream: u32) {
    let _ = out.send(Outbound {
        stream,
        kind: kind::EXIT,
        payload: serde_json::to_vec(&Exit::default()).unwrap_or_default(),
    });
}
