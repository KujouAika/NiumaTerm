//! A view, on the host itself, of a terminal a paired device started here.
//! Such a terminal runs headless, with its engine next to the PTY; a host
//! tab reads it the way a remote view does, from a checkpoint and then the
//! live output, but in-process instead of over a channel.

use std::sync::Arc;
use std::task::{Context, Poll};
use std::{io, mem};

use nmt_platform::{AsyncPty, WinsizeBuilder};
use nmt_terminal::event::{CheckpointRequest, Msg, MsgSender, OutputSink};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::sync::watch;
use tracing::warn;

use crate::sessions::SessionRegistry;

enum Event {
    Checkpoint(Vec<u8>),
    Output(Arc<[u8]>),
}

/// Reads yield a checkpoint and then every later byte of the session in
/// engine order; writes become the session's input. The session's own
/// engine answers terminal queries, so the engine reading this must not.
/// Dropping the view detaches it; the terminal keeps running.
pub struct LocalView {
    session: String,
    registry: Arc<SessionRegistry>,
    messenger: MsgSender,
    events: UnboundedReceiver<Event>,
    buffered: Arc<[u8]>,
    offset: usize,
    exited: bool,
    restarted: bool,

    /// The size this view last asked for, and the PTY's size as the
    /// registry reports it. They differ after a paired device resized the
    /// PTY; this view takes it back when its user types.
    claimed: Option<(u16, u16)>,

    size: watch::Receiver<(u16, u16)>,
}

/// Open a view of `session`, or `None` when it is gone.
pub fn open(registry: &Arc<SessionRegistry>, session: &str) -> Option<LocalView> {
    let (control, size) = registry.attach_info(session)?;
    let (events_tx, events) = mpsc::unbounded_channel();
    let checkpoint_tx = events_tx.clone();

    // The loop runs the checkpoint and registers the sink in one step, so
    // nothing is lost or repeated between them. Once the shell ends the loop
    // drops both senders, which ends the view.
    let subscribed = control.messenger.send(Msg::Subscribe {
        sink: OutputSink(Box::new(move |bytes| {
            events_tx.send(Event::Output(bytes)).is_ok()
        })),
        checkpoint: CheckpointRequest(Box::new(move |result| match result {
            Ok(checkpoint) => {
                let _ = checkpoint_tx.send(Event::Checkpoint(checkpoint.vt));
            }
            Err(error) => warn!(?error, "local view checkpoint failed"),
        })),
    });

    // A session whose loop is already gone drops the message, and with it
    // the senders, which reads as the exit.
    drop(subscribed);

    Some(LocalView {
        session: session.to_owned(),
        registry: Arc::clone(registry),
        messenger: control.messenger,
        events,
        buffered: Arc::from([]),
        offset: 0,
        exited: false,
        restarted: false,
        claimed: None,
        size,
    })
}

impl LocalView {
    pub fn session(&self) -> &str {
        &self.session
    }
}

impl AsyncPty for LocalView {
    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        while self.offset == self.buffered.len() {
            if self.exited {
                return Poll::Ready(Ok(0));
            }

            match self.events.poll_recv(cx) {
                Poll::Ready(Some(Event::Output(bytes))) => {
                    self.buffered = bytes;
                    self.offset = 0;
                }
                Poll::Ready(Some(Event::Checkpoint(bytes))) => {
                    self.buffered = bytes.into();
                    self.offset = 0;
                    self.restarted = true;
                }
                Poll::Ready(None) => {
                    self.exited = true;

                    return Poll::Ready(Ok(0));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        let len = buf.len().min(self.buffered.len() - self.offset);

        buf[..len].copy_from_slice(&self.buffered[self.offset..self.offset + len]);

        self.offset += len;

        Poll::Ready(Ok(len))
    }

    fn poll_write(&mut self, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if let Some((cols, rows)) = self.claimed
            && *self.size.borrow() != (cols, rows)
        {
            self.registry.resize(&self.session, cols, rows);
        }

        let _ = self.messenger.send(Msg::Input(buf.to_vec().into()));

        Poll::Ready(Ok(buf.len()))
    }

    fn poll_exit(&mut self, _cx: &mut Context<'_>) -> Poll<()> {
        // `poll_read` registered the waker on the queue whose closing is the
        // exit, and runs first in every loop pass.
        if self.exited {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    fn poll_resize(&mut self, _cx: &mut Context<'_>, size: WinsizeBuilder) -> Poll<io::Result<()>> {
        self.claimed = Some((size.cols, size.rows));

        self.registry.resize(&self.session, size.cols, size.rows);

        Poll::Ready(Ok(()))
    }

    fn take_stream_reset(&mut self) -> bool {
        mem::take(&mut self.restarted)
    }
}
