//! Waiting for another process to exit.

use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

/// Block until the process with `pid` has exited, and report whether it had by
/// the time this returned.
///
/// A process that cannot be opened is reported as gone: the identifier is
/// either already free or belongs to something this account may not touch, and
/// waiting on either would gain nothing. `timeout` bounds the opposite case,
/// a process that never finishes shutting down, so a caller waiting for a
/// predecessor is delayed instead of stuck behind it.
pub(crate) fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    // SAFETY: PROCESS_SYNCHRONIZE alone is enough to wait on the returned handle, and a
    // failure is reported as a null handle instead of through an out-parameter.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };

    if handle.is_null() {
        return true;
    }

    let milliseconds = timeout.as_millis().try_into().unwrap_or(u32::MAX);

    // SAFETY: the handle came from OpenProcess above and is closed below,
    // exactly once, after the wait it was opened for.
    let signalled = unsafe { WaitForSingleObject(handle, milliseconds) } == WAIT_OBJECT_0;

    unsafe { CloseHandle(handle) };

    signalled
}
