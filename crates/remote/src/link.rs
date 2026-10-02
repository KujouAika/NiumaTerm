//! Moves binary messages between a WebSocket and the protocol state machines.
//! One WebSocket binary message holds one Noise message, on the LAN and
//! later through the relay, so both paths share this code.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use futures::{Sink, SinkExt as _, Stream, StreamExt as _};
use nmt_remote_core::channel::Channel;
use nmt_remote_core::frame::{
    CONTROL_STREAM, Frame, Message as FrameMessage, Reassembler, encode_message, kind,
};
use tokio::select;
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, sleep_until};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// Largest backlog one channel may queue. A peer that stays this far
/// behind is closed and reconnects, which replaces the backlog with a
/// checkpoint instead of letting memory and latency grow without bound.
pub(crate) const MAX_QUEUED: usize = 8 * 1024 * 1024;

/// One message to send on a channel stream.
pub(crate) struct Outbound {
    pub stream: u32,
    pub kind: u8,
    pub payload: Vec<u8>,

    /// Told how many payload bytes left once the message is sent, so
    /// a stream can track its own backlog.
    pub on_sent: Option<Arc<dyn SentHook>>,
}

pub(crate) trait SentHook: Send + Sync {
    fn sent(&self, bytes: usize);
}

impl Outbound {
    pub(crate) fn new(stream: u32, kind: u8, payload: Vec<u8>) -> Self {
        Self {
            stream,
            kind,
            payload,
            on_sent: None,
        }
    }
}

/// The sending half of a channel's outbound queue, which counts the bytes
/// waiting in it.
#[derive(Clone)]
pub(crate) struct SendQueue {
    tx: UnboundedSender<Outbound>,
    queued: Arc<AtomicUsize>,
}

pub(crate) struct QueueReceiver {
    rx: UnboundedReceiver<Outbound>,
    queued: Arc<AtomicUsize>,
}

impl QueueReceiver {
    #[cfg(test)]
    pub(crate) fn try_recv(&mut self) -> Option<Outbound> {
        let outbound = self.rx.try_recv().ok()?;

        self.queued
            .fetch_sub(outbound.payload.len(), Ordering::Relaxed);

        Some(outbound)
    }
}

impl SendQueue {
    pub(crate) fn new() -> (Self, QueueReceiver) {
        let (tx, rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));

        (
            Self {
                tx,
                queued: Arc::clone(&queued),
            },
            QueueReceiver { rx, queued },
        )
    }

    /// Queue a message; `false` once the channel is gone.
    pub(crate) fn send(&self, outbound: Outbound) -> bool {
        self.queued
            .fetch_add(outbound.payload.len(), Ordering::Relaxed);

        self.tx.send(outbound).is_ok()
    }

    #[cfg(feature = "host")]
    pub(crate) fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

pub(crate) async fn recv_binary<S>(ws: &mut S) -> Result<Vec<u8>>
where
    S: Stream<Item = Result<Message, WsError>> + Unpin,
{
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(bytes))) => return Ok(bytes.to_vec()),
            Some(Ok(Message::Close(_))) | None => bail!("connection closed"),
            Some(Ok(_)) => {}
            Some(Err(error)) => return Err(error.into()),
        }
    }
}

pub(crate) async fn send_binary<S>(ws: &mut S, bytes: Vec<u8>) -> Result<()>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    ws.send(Message::Binary(bytes.into())).await?;

    Ok(())
}

/// Probe a quiet channel after this long without inbound frames.
const IDLE_BEFORE_PING: Duration = Duration::from_secs(30);

/// An unanswered probe counts as missed after this long.
const PING_TIMEOUT: Duration = Duration::from_secs(10);

/// Two missed probes in a row mark the channel dead, so one lost to a
/// momentary stall does not tear down working sessions.
const MAX_MISSED_PINGS: u32 = 2;

/// How long a probe asked for after a network change may go unanswered.
/// The change itself is the evidence that the socket may be dead, so one
/// miss closes the channel instead of the two a quiet link gets.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Run an established channel until either side closes it: seal and send
/// `outbound`, open and reassemble what arrives into `inbound`. One task owns
/// both directions because the Noise transport keeps both ciphers in one
/// state.
///
/// The pump also keeps the channel's liveness accurate: a network that
/// drops packets without closing the socket (sleep, a pulled cable, a NAT
/// timeout) would otherwise leave both sides waiting forever. It answers
/// probes itself, so the layers above never see them. `probe` asks for one
/// at once, as after the machine's network changed: a socket bound to an
/// address that is gone stays silent, and waiting out the idle timer would
/// leave the views frozen for most of a minute.
pub(crate) async fn pump<S>(
    mut ws: S,
    mut channel: Channel,
    mut outbound: QueueReceiver,
    inbound: UnboundedSender<FrameMessage>,
    probe: Arc<Notify>,
) -> Result<()>
where
    S: Stream<Item = Result<Message, WsError>> + Sink<Message, Error = WsError> + Unpin,
{
    let mut reassembler = Reassembler::default();
    let mut deadline = Instant::now() + IDLE_BEFORE_PING;
    let mut missed = 0;
    let mut probing = false;

    loop {
        select! {
            received = recv_binary(&mut ws) => {
                let plaintext = channel.open(&received?)?;
                let frame = Frame::decode(&plaintext)?;

                deadline = Instant::now() + IDLE_BEFORE_PING;
                missed = 0;
                probing = false;

                match (frame.stream, frame.kind) {
                    (CONTROL_STREAM, kind::PING) => {
                        send_frames(&mut ws, &mut channel, CONTROL_STREAM, kind::PONG, frame.payload)
                            .await?;
                    }
                    (CONTROL_STREAM, kind::PONG) => {}
                    _ => {
                        if let Some(message) = reassembler.push(frame)?
                            && inbound.send(message).is_err()
                        {
                            return Ok(());
                        }
                    }
                }
            }

            next = outbound.rx.recv() => {
                let Some(next) = next else {
                    let _ = ws.close().await;

                    return Ok(());
                };

                if outbound.queued.load(Ordering::Relaxed) > MAX_QUEUED {
                    bail!("the peer fell too far behind");
                }

                send_frames(&mut ws, &mut channel, next.stream, next.kind, &next.payload).await?;

                outbound
                    .queued
                    .fetch_sub(next.payload.len(), Ordering::Relaxed);

                if let Some(hook) = &next.on_sent {
                    hook.sent(next.payload.len());
                }
            }

            () = probe.notified() => {
                missed = MAX_MISSED_PINGS - 1;
                probing = true;
                deadline = Instant::now() + PROBE_TIMEOUT;

                send_frames(&mut ws, &mut channel, CONTROL_STREAM, kind::PING, &[]).await?;
            }

            () = sleep_until(deadline) => {
                if probing {
                    missed += 1;

                    if missed >= MAX_MISSED_PINGS {
                        bail!("the peer stopped answering");
                    }
                }

                probing = true;
                deadline = Instant::now() + PING_TIMEOUT;

                send_frames(&mut ws, &mut channel, CONTROL_STREAM, kind::PING, &[]).await?;
            }
        }
    }
}

async fn send_frames<S>(
    ws: &mut S,
    channel: &mut Channel,
    stream: u32,
    kind: u8,
    payload: &[u8],
) -> Result<()>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    for frame in encode_message(stream, kind, payload) {
        let sealed = channel.seal(&frame)?;

        send_binary(ws, sealed).await?;
    }

    Ok(())
}
