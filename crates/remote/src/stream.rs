//! One remote view attached to a host session.
//!
//! Output normally streams byte for byte. A view that falls behind (a flood
//! such as `cat` of a large file over a slow link) would otherwise see
//! latency grow without bound, so past a backlog of 1 MiB the stream drops
//! output, and once the backlog drains below 256 KiB it sends a fresh
//! checkpoint instead. A flood then degrades to skipped frames, not lag.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use nmt_remote_core::frame::kind;
use nmt_remote_core::rpc::Exit;
use nmt_terminal::event::{CheckpointRequest, Msg, MsgSender, OutputSink};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::watch;
use tracing::warn;

use crate::link::{Outbound, SendQueue, SentHook};
use crate::sessions::TerminalControl;

const HIGH_WATER: usize = 1024 * 1024;

#[derive(Serialize)]
struct SizeFrame {
    cols: u16,
    rows: u16,
}

const LOW_WATER: usize = 256 * 1024;

pub(crate) struct StreamFlow {
    stream: u32,
    queue: SendQueue,
    messenger: MsgSender,
    me: Weak<StreamFlow>,
    pending: AtomicUsize,
    state: Mutex<FlowState>,
    closed: AtomicBool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FlowState {
    Live,
    /// Output is discarded until the backlog drains.
    Dropping,
    /// A checkpoint was requested; output is discarded until it is queued,
    /// because the checkpoint already contains it.
    Resyncing,
}

/// Held by the session's subscriber. The PTY loop drops its subscribers
/// when the shell ends, which is when the view hears about it.
struct ExitNotice(Arc<StreamFlow>);

impl StreamFlow {
    /// Attach `stream` to a session: a checkpoint, then every later byte.
    /// Call after queueing the attach response, so the client learns the
    /// stream id before the checkpoint arrives.
    pub(crate) fn attach(
        stream: u32,
        control: &TerminalControl,
        mut size: watch::Receiver<(u16, u16)>,
        queue: SendQueue,
    ) -> Arc<Self> {
        let flow = Arc::new_cyclic(|me| Self {
            stream,
            queue,
            messenger: control.messenger.clone(),
            me: me.clone(),
            pending: AtomicUsize::new(0),
            state: Mutex::new(FlowState::Resyncing),
            closed: AtomicBool::new(false),
        });

        let notice = ExitNotice(Arc::clone(&flow));
        let checkpoint = flow.checkpoint_request();

        let subscribed = control.messenger.send(Msg::Subscribe {
            sink: OutputSink(Box::new(move |bytes| notice.0.on_output(&bytes))),
            checkpoint,
        });

        // A session whose loop is already gone drops the message, and with
        // it the notice, which reports the exit.
        drop(subscribed);

        // Tell the view whenever another view or the host tab resizes the
        // PTY, so it can take the size back when its own user types.
        let sizes = Arc::downgrade(&flow);

        tokio::spawn(async move {
            size.mark_unchanged();

            while size.changed().await.is_ok() {
                let Some(flow) = sizes.upgrade() else {
                    return;
                };

                if flow.closed.load(Ordering::Relaxed) || flow.queue.is_closed() {
                    return;
                }

                let (cols, rows) = *size.borrow_and_update();
                let payload = serde_json::to_vec(&SizeFrame { cols, rows }).unwrap_or_default();

                flow.queue
                    .send(Outbound::new(flow.stream, kind::SIZE, payload));
            }
        });

        flow
    }

    pub(crate) fn input(&self, bytes: Vec<u8>) {
        let _ = self.messenger.send(Msg::Input(bytes.into()));
    }

    /// The client detached. The subscriber unsubscribes at the next output.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    fn on_output(&self, bytes: &[u8]) -> bool {
        if self.closed.load(Ordering::Relaxed) || self.queue.is_closed() {
            return false;
        }

        let mut state = self.state.lock();

        if *state != FlowState::Live {
            return true;
        }

        if self.pending.load(Ordering::Relaxed) + bytes.len() > HIGH_WATER {
            *state = FlowState::Dropping;

            return true;
        }

        drop(state);

        self.enqueue(kind::OUTPUT, bytes.to_vec())
    }

    fn enqueue(&self, kind: u8, payload: Vec<u8>) -> bool {
        self.pending.fetch_add(payload.len(), Ordering::Relaxed);

        let hook = self.me.upgrade().map(|flow| flow as Arc<dyn SentHook>);

        self.queue.send(Outbound {
            stream: self.stream,
            kind,
            payload,
            on_sent: hook,
        })
    }

    /// Runs on the PTY loop in order with its output, so the checkpoint and
    /// the output after it line up exactly.
    fn checkpoint_request(&self) -> CheckpointRequest {
        let me = self.me.clone();

        CheckpointRequest(Box::new(move |result| {
            let Some(flow) = me.upgrade() else {
                return;
            };

            match result {
                Ok(checkpoint) => {
                    flow.enqueue(kind::CHECKPOINT, checkpoint.vt);
                }
                Err(error) => warn!(?error, "remote checkpoint failed"),
            }

            *flow.state.lock() = FlowState::Live;
        }))
    }
}

impl SentHook for StreamFlow {
    fn sent(&self, bytes: usize) {
        let pending = self.pending.fetch_sub(bytes, Ordering::Relaxed) - bytes;

        let mut state = self.state.lock();

        if *state == FlowState::Dropping && pending < LOW_WATER {
            *state = FlowState::Resyncing;

            drop(state);

            let _ = self
                .messenger
                .send(Msg::Checkpoint(self.checkpoint_request()));
        }
    }
}

impl Drop for ExitNotice {
    fn drop(&mut self) {
        let flow = &self.0;

        if !flow.closed.load(Ordering::Relaxed) {
            let payload = serde_json::to_vec(&Exit::default()).unwrap_or_default();

            flow.queue
                .send(Outbound::new(flow.stream, kind::EXIT, payload));
        }
    }
}
