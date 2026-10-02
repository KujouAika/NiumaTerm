use std::io;
use std::task::{Context, Poll};

use crate::WinsizeBuilder;

/// A nonblocking PTY whose pending operations wake the current async task.
/// Native writes retain their buffers until completion; a successful flush
/// permits a following resize, without claiming that the child consumed input.
/// Reads report `BrokenPipe` once the child side has hung up.
pub trait AsyncPty: Send + 'static {
    /// Read output, registering the task before returning `Pending`.
    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;

    /// Accept input, registering the task before returning `Pending`.
    fn poll_write(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>>;

    /// Wait for submitted native writes. Writers that finish every write
    /// inside `poll_write` have nothing to wait for.
    fn poll_flush(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_exit(&mut self, cx: &mut Context<'_>) -> Poll<()>;

    /// Complete one size change before accepting the next ordered command.
    /// The caller repolls the same request until it completes, so a backend
    /// that hands the change to another process keeps ownership of that work
    /// and reports the running change on every repoll.
    fn poll_resize(&mut self, cx: &mut Context<'_>, size: WinsizeBuilder) -> Poll<io::Result<()>>;

    /// Finish pending control work before native handles are destroyed.
    fn poll_shutdown(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    /// Whether the bytes the last read returned start a new stream that
    /// replaces everything read before, as a checkpoint replayed after a
    /// reconnect does. The reader then drops state its parser keeps outside
    /// the screen before parsing them. Local PTYs never restart.
    fn take_stream_reset(&mut self) -> bool {
        false
    }
}

/// Convert an immediate nonblocking operation after its task waker is installed.
pub fn poll_nonblocking<T>(cx: &mut Context<'_>, result: io::Result<T>) -> Poll<io::Result<T>> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
        Err(error) if error.kind() == io::ErrorKind::Interrupted => {
            cx.waker().wake_by_ref();

            Poll::Pending
        }
        result => Poll::Ready(result),
    }
}
