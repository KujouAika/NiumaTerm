//! A remote terminal presented as a local PTY, so a client tab runs the same
//! engine, rendering, selection, and search code as a local one.

use std::sync::Arc;
use std::task::{Context, Poll};
use std::{io, mem};

use nmt_platform::{AsyncPty, WinsizeBuilder};
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::info;

use crate::connection::{RemoteHost, StreamEvent};

/// Reads yield a checkpoint and then the live output of one host session;
/// writes become input on the host. The host engine next to the PTY answers
/// terminal queries, so the client engine reading this must not reply too.
///
/// While the link is down, reads stay pending and input is dropped. After a
/// reconnect the session replays from a checkpoint, which this reports as a
/// stream reset so the client engine drops what it derived before.
pub struct NetworkPty {
    host: Arc<RemoteHost>,
    session: String,
    events: UnboundedReceiver<StreamEvent>,
    buffered: Vec<u8>,
    offset: usize,
    exited: bool,
    restarted: bool,

    /// The size this view last asked for, and the PTY's size as the host
    /// last reported it. They differ after another view resized the PTY;
    /// this view takes it back when its user types.
    claimed: Option<(u16, u16)>,

    host_size: Option<(u16, u16)>,
}

impl NetworkPty {
    pub(crate) fn new(
        host: Arc<RemoteHost>,
        session: String,
        events: UnboundedReceiver<StreamEvent>,
    ) -> Self {
        Self {
            host,
            session,
            events,
            buffered: Vec::new(),
            offset: 0,
            exited: false,
            restarted: false,
            claimed: None,
            host_size: None,
        }
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn host(&self) -> &Arc<RemoteHost> {
        &self.host
    }
}

impl AsyncPty for NetworkPty {
    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        while self.offset == self.buffered.len() {
            if self.exited {
                return Poll::Ready(Ok(0));
            }

            match self.events.poll_recv(cx) {
                Poll::Ready(Some(StreamEvent::Output(bytes))) => {
                    self.buffered = bytes;
                    self.offset = 0;
                }
                Poll::Ready(Some(StreamEvent::Checkpoint(bytes))) => {
                    self.buffered = bytes;
                    self.offset = 0;
                    self.restarted = true;
                }
                Poll::Ready(Some(StreamEvent::Size(cols, rows))) => {
                    self.host_size = Some((cols, rows));
                }
                // The shell ended, or the host forgot this view: nothing
                // more arrives on this stream.
                Poll::Ready(Some(StreamEvent::Exit) | None) => {
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
        if let Some(claimed) = self.claimed
            && self.host_size.is_some_and(|size| size != claimed)
        {
            self.host.resize(&self.session, claimed.0, claimed.1);

            self.host_size = Some(claimed);
        }

        self.host.send_input(&self.session, buf.to_vec());

        Poll::Ready(Ok(buf.len()))
    }

    fn poll_exit(&mut self, _cx: &mut Context<'_>) -> Poll<()> {
        // `poll_read` registered the waker on the same event queue that
        // delivers the exit, and runs first in every loop pass.
        if self.exited {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    fn poll_resize(&mut self, _cx: &mut Context<'_>, size: WinsizeBuilder) -> Poll<io::Result<()>> {
        self.claimed = Some((size.cols, size.rows));
        self.host_size = self.claimed;

        self.host.resize(&self.session, size.cols, size.rows);

        Poll::Ready(Ok(()))
    }

    fn take_stream_reset(&mut self) -> bool {
        mem::take(&mut self.restarted)
    }
}

/// Closing a view detaches it; the session keeps running on the host until
/// someone ends it there or asks explicitly.
impl Drop for NetworkPty {
    fn drop(&mut self) {
        info!(session = %self.session, "closing the view of a remote terminal");

        self.host.detach(&self.session);
    }
}
