//! The client side: pairs with a host and opens terminals on it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use futures::{Sink, Stream};
use nmt_remote_core::channel::ClientHandshake;
use nmt_remote_core::frame::{CONTROL_STREAM, Message as FrameMessage, kind};
use nmt_remote_core::identity::DeviceKey;
use nmt_remote_core::messages::{ClientHello, DeviceInfo};
use nmt_remote_core::pairing::{ClientPairing, PairingCode};
use nmt_remote_core::preface::{Preface, PrefaceKind};
use nmt_remote_core::rpc::{self, Attached, Control, RpcError, SessionRef, TerminalOpen};
use nmt_remote_core::{PROTO_MAJOR, PROTO_MINOR};
use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tracing::debug;

use crate::link::{Outbound, pump, recv_binary, send_binary};
use crate::network_pty::NetworkPty;
use crate::store::{PairedHost, now_ms};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FEATURES: &[&str] = &["terminal"];

/// What a terminal stream delivers to its [`NetworkPty`].
pub(crate) enum StreamEvent {
    Output(Vec<u8>),
    Exit,
}

/// An open channel to one host, shared by every remote terminal on it.
pub struct RemoteHost {
    out: UnboundedSender<Outbound>,
    routes: Arc<Mutex<Routes>>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct Routes {
    calls: HashMap<u64, PendingCall>,
    streams: HashMap<u32, UnboundedSender<StreamEvent>>,
}

struct PendingCall {
    reply: oneshot::Sender<Result<Value, RpcError>>,

    /// For an attach: the stream to route once the response names its id.
    /// Routing it while handling the response, before the next frame, means
    /// the checkpoint that follows can never arrive for an unknown stream.
    stream: Option<UnboundedSender<StreamEvent>>,
}

/// Pair with the host showing `code` at `address` (`host:port`).
pub async fn pair(
    address: &str,
    code: &PairingCode,
    key: &DeviceKey,
    device: DeviceInfo,
) -> Result<PairedHost> {
    timeout(CONNECT_TIMEOUT, async {
        let (mut ws, offer, answer) = open(address, PrefaceKind::Pairing).await?;

        let (pairing, hello) = ClientPairing::start(code, &offer, &answer, device, None)?;

        send_binary(&mut ws, hello).await?;

        let reply = recv_binary(&mut ws).await?;
        let (handshake, msg1) = pairing.on_reply(key, &reply)?;

        send_binary(&mut ws, msg1).await?;

        let msg2 = recv_binary(&mut ws).await?;
        let (confirm, msg3) = handshake.on_msg2(&msg2)?;

        send_binary(&mut ws, msg3).await?;

        // A host that rejects the code closes without answering.
        let accepted = recv_binary(&mut ws)
            .await
            .context("the host did not accept the pairing code")?;

        let paired = confirm.on_accepted(&accepted)?;

        Ok(PairedHost::new(
            paired.public_key,
            paired.accepted.host.name,
            address.to_owned(),
        ))
    })
    .await
    .map_err(|_| anyhow!("pairing timed out"))?
}

/// Open a channel to a paired host. `host` records the handshake time and
/// the host's current name; the caller stores it afterwards.
pub async fn connect(
    host: &mut PairedHost,
    key: &DeviceKey,
    app_version: &str,
) -> Result<Arc<RemoteHost>> {
    let address = host
        .lan_hints
        .first()
        .cloned()
        .context("no known address for this host")?;

    let hello = ClientHello {
        proto_minor: PROTO_MINOR,
        app_version: app_version.to_owned(),
        features: FEATURES.iter().map(|&feature| feature.into()).collect(),
        hello_ms: host.next_hello_ms(),
    };

    let (ws, channel, host_hello) = timeout(CONNECT_TIMEOUT, async {
        let (mut ws, offer, answer) = open(&address, PrefaceKind::Channel).await?;

        let (handshake, msg1) =
            ClientHandshake::start(key, &host.public_key, &offer, &answer, &hello)?;

        send_binary(&mut ws, msg1).await?;

        // An unpaired or revoked device gets no reply, only a closed socket.
        let msg2 = recv_binary(&mut ws)
            .await
            .context("the host refused this device; pair again")?;

        let (channel, host_hello) = handshake.finish(&msg2)?;

        anyhow::Ok((ws, channel, host_hello))
    })
    .await
    .map_err(|_| anyhow!("connecting to {address} timed out"))??;

    host.name = host_hello.name;
    host.last_seen = now_ms();

    let (out, out_rx) = mpsc::unbounded_channel();
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let routes = Arc::new(Mutex::new(Routes::default()));

    tokio::spawn(async move {
        if let Err(error) = pump(ws, channel, out_rx, in_tx).await {
            debug!(%error, "remote channel ended");
        }
    });

    tokio::spawn(dispatch(Arc::clone(&routes), in_rx));

    Ok(Arc::new(RemoteHost {
        out,
        routes,
        next_id: AtomicU64::new(1),
    }))
}

impl RemoteHost {
    /// Start a terminal on the host with the given grid and attach to it.
    pub async fn open_terminal(self: &Arc<Self>, cols: u16, rows: u16) -> Result<NetworkPty> {
        let SessionRef { session } = self
            .call(rpc::TERMINAL_OPEN, &TerminalOpen { cols, rows }, None)
            .await?;

        let (events, events_rx) = mpsc::unbounded_channel();

        let Attached { stream, .. } = self
            .call(
                rpc::TERMINAL_ATTACH,
                &SessionRef {
                    session: session.clone(),
                },
                Some(events),
            )
            .await?;

        Ok(NetworkPty::new(
            Arc::clone(self),
            session,
            stream,
            events_rx,
        ))
    }

    pub fn is_connected(&self) -> bool {
        !self.out.is_closed()
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: &impl Serialize,
        stream: Option<UnboundedSender<StreamEvent>>,
    ) -> Result<T> {
        let (reply, response) = oneshot::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        self.routes
            .lock()
            .calls
            .insert(id, PendingCall { reply, stream });

        self.request(id, method, params)?;

        let value = response
            .await
            .map_err(|_| anyhow!("the connection closed"))?
            .map_err(|error| anyhow!("{method}: {}", error.message))?;

        Ok(serde_json::from_value(value)?)
    }

    /// Send a request whose response nobody waits for. The host answers it
    /// anyway; the dispatcher drops answers to unknown ids.
    pub(crate) fn notify(&self, method: &str, params: &impl Serialize) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let _ = self.request(id, method, params);
    }

    fn request(&self, id: u64, method: &str, params: &impl Serialize) -> Result<()> {
        let request = Control::Request {
            id,
            method: method.to_owned(),
            params: serde_json::to_value(params)?,
        };

        self.send(CONTROL_STREAM, kind::CONTROL_JSON, request.encode())
    }

    pub(crate) fn send(&self, stream: u32, kind: u8, payload: Vec<u8>) -> Result<()> {
        self.out
            .send(Outbound {
                stream,
                kind,
                payload,
            })
            .map_err(|_| anyhow!("the connection closed"))
    }

    pub(crate) fn detach(&self, stream: u32) {
        self.routes.lock().streams.remove(&stream);
    }
}

/// Connect a WebSocket to `address` and agree on a protocol major.
async fn open(
    address: &str,
    kind: PrefaceKind,
) -> Result<(
    impl Stream<Item = Result<Message, WsError>>
    + Sink<Message, Error = WsError>
    + Unpin
    + Send
    + 'static,
    Preface,
    Preface,
)> {
    let (mut ws, _) = connect_async(format!("ws://{address}/v1")).await?;

    let offer = Preface::offer(kind);

    send_binary(&mut ws, offer.encode().to_vec()).await?;

    let answer = Preface::decode(&recv_binary(&mut ws).await?)?;

    if answer.kind == PrefaceKind::Rejected {
        if answer.min_major > PROTO_MAJOR {
            bail!("this computer runs an older remote protocol; update NiumaTerm here");
        }

        bail!("the other computer runs an older remote protocol; update NiumaTerm there");
    }

    Ok((ws, offer, answer))
}

async fn dispatch(routes: Arc<Mutex<Routes>>, mut inbound: UnboundedReceiver<FrameMessage>) {
    while let Some(message) = inbound.recv().await {
        let mut routes = routes.lock();

        match (message.stream, message.kind) {
            (CONTROL_STREAM, kind::CONTROL_JSON) => {
                let Ok(Control::Response { id, outcome }) = Control::decode(&message.payload)
                else {
                    continue;
                };

                let Some(call) = routes.calls.remove(&id) else {
                    continue;
                };

                if let (Ok(result), Some(events)) = (&outcome, call.stream)
                    && let Some(stream) = result.get("stream").and_then(Value::as_u64)
                {
                    routes.streams.insert(stream as u32, events);
                }

                let _ = call.reply.send(outcome);
            }
            (stream, kind::CHECKPOINT | kind::OUTPUT) => {
                if let Some(events) = routes.streams.get(&stream) {
                    let _ = events.send(StreamEvent::Output(message.payload));
                }
            }
            (stream, kind::EXIT) => {
                if let Some(events) = routes.streams.remove(&stream) {
                    let _ = events.send(StreamEvent::Exit);
                }
            }
            _ => {}
        }
    }

    // The channel is gone: pending calls fail and every stream ends.
    let mut routes = routes.lock();

    routes.calls.clear();
    routes.streams.clear();
}
