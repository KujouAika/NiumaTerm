//! A remote terminal presented as a local PTY, so a client tab runs the same
//! engine, rendering, selection, and search code as a local one.

use std::io;
use std::sync::Arc;
use std::task::{Context, Poll};

use nmt_platform::{AsyncPty, WinsizeBuilder};
use nmt_remote_core::frame::kind;
use nmt_remote_core::rpc::{self, SessionRef, TerminalResize};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::client::{RemoteHost, StreamEvent};

/// Reads yield the checkpoint and then the live output of one host terminal;
/// writes become input on the host. The host engine next to the PTY answers
/// terminal queries, so the client engine reading this must not reply too.
pub struct NetworkPty {
    host: Arc<RemoteHost>,
    session: String,
    stream: u32,
    events: UnboundedReceiver<StreamEvent>,
    buffered: Vec<u8>,
    offset: usize,
    exited: bool,
}

impl NetworkPty {
    pub(crate) fn new(
        host: Arc<RemoteHost>,
        session: String,
        stream: u32,
        events: UnboundedReceiver<StreamEvent>,
    ) -> Self {
        Self {
            host,
            session,
            stream,
            events,
            buffered: Vec::new(),
            offset: 0,
            exited: false,
        }
    }
}

impl AsyncPty for NetworkPty {
    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if self.offset == self.buffered.len() {
            if self.exited {
                return Poll::Ready(Ok(0));
            }

            match self.events.poll_recv(cx) {
                Poll::Ready(Some(StreamEvent::Output(bytes))) => {
                    self.buffered = bytes;
                    self.offset = 0;
                }
                // The shell ended, or the connection did: either way nothing
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
        Poll::Ready(
            self.host
                .send(self.stream, kind::INPUT, buf.to_vec())
                .map(|()| buf.len())
                .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error.to_string())),
        )
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
        self.host.notify(
            rpc::TERMINAL_RESIZE,
            &TerminalResize {
                session: self.session.clone(),
                cols: size.cols,
                rows: size.rows,
            },
        );

        Poll::Ready(Ok(()))
    }
}

/// Closing the tab ends the host terminal: nothing else can reach it yet.
impl Drop for NetworkPty {
    fn drop(&mut self) {
        self.host.detach(self.stream);

        self.host.notify(
            rpc::TERMINAL_CLOSE,
            &SessionRef {
                session: self.session.clone(),
            },
        );
    }
}
