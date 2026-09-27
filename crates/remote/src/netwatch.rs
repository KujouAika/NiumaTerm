//! Changes to this machine's IP addresses: joining another Wi-Fi network,
//! waking from sleep, a VPN coming up. After one, a connected link may be
//! bound to an address that is gone and a host that was out of reach may be
//! back, so the client checks at once instead of waiting for its timers.

use std::sync::OnceLock;

use tokio::sync::watch;

static CHANGES: OnceLock<watch::Sender<u64>> = OnceLock::new();

/// Bumped on every address change. Platforms without a notification source
/// never bump it, which leaves the liveness probes and the backoff as the
/// only way to notice.
pub(crate) fn changes() -> watch::Receiver<u64> {
    CHANGES
        .get_or_init(|| {
            register();

            watch::channel(0).0
        })
        .subscribe()
}

#[cfg(windows)]
fn register() {
    use std::ffi::c_void;
    use std::ptr;

    use tracing::warn;
    use windows_sys::Win32::Foundation::NO_ERROR;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_NOTIFICATION_TYPE, MIB_UNICASTIPADDRESS_ROW, NotifyUnicastIpAddressChange,
    };
    use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;

    // Runs on a system thread pool thread. A change that arrives before
    // `CHANGES` is set finds nobody listening yet, so dropping it is right.
    unsafe extern "system" fn on_change(
        _context: *const c_void,
        _row: *const MIB_UNICASTIPADDRESS_ROW,
        _kind: MIB_NOTIFICATION_TYPE,
    ) {
        if let Some(changes) = CHANGES.get() {
            changes.send_modify(|version| *version += 1);
        }
    }

    let mut handle = ptr::null_mut();

    // SAFETY: the callback touches only a static and ignores its arguments,
    // so the null context is never dereferenced. The registration is never
    // cancelled: it serves the whole process, and cancelling from inside a
    // callback would deadlock.
    let status = unsafe {
        NotifyUnicastIpAddressChange(AF_UNSPEC, Some(on_change), ptr::null(), false, &mut handle)
    };

    if status != NO_ERROR {
        warn!(status, "network change notifications are unavailable");
    }
}

#[cfg(not(windows))]
fn register() {}
