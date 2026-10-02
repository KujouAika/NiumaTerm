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
    sender().subscribe()
}

fn sender() -> &'static watch::Sender<u64> {
    CHANGES.get_or_init(|| {
        register();

        watch::channel(0).0
    })
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

#[cfg(target_os = "macos")]
fn register() {
    use std::fs::File;
    use std::io::{Error, ErrorKind, Read};
    use std::os::fd::FromRawFd;
    use std::thread;

    use tracing::warn;

    // A routing socket receives a copy of every routing message the kernel
    // emits, and address changes arrive as RTM_NEWADDR and RTM_DELADDR. It
    // needs no privileges and no run loop, unlike SystemConfiguration.
    //
    // SAFETY: plain socket creation; the descriptor is checked below.
    let fd = unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW, libc::AF_UNSPEC) };

    if fd < 0 {
        warn!(
            error = %Error::last_os_error(),
            "network change notifications are unavailable"
        );

        return;
    }

    // SAFETY: `fd` is a fresh descriptor nothing else owns.
    let mut socket = unsafe { File::from_raw_fd(fd) };

    // The reader lives as long as the process: the notifications serve
    // every connection, and a blocking read has no cheap way to be woken
    // for shutdown.
    let spawned = thread::Builder::new()
        .name("netwatch".into())
        .spawn(move || {
            // Each read returns one whole message; the largest routing
            // messages stay well under this.
            let mut message = [0u8; 2048];

            loop {
                let length = match socket.read(&mut message) {
                    Ok(length) => length,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) => {
                        warn!(%error, "network change notifications stopped");

                        return;
                    }
                };

                // Every routing message starts with its length, version and
                // type, so the type is the fourth byte whatever the message.
                let Some(&kind) = message[..length].get(3) else {
                    continue;
                };

                if matches!(i32::from(kind), libc::RTM_NEWADDR | libc::RTM_DELADDR)
                    && let Some(changes) = CHANGES.get()
                {
                    changes.send_modify(|version| *version += 1);
                }
            }
        });

    if let Err(error) = spawned {
        warn!(%error, "network change notifications are unavailable");
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn register() {}

/// Report an address change the application learned of itself, on
/// platforms where only it can hear them (iOS, through `NWPathMonitor`).
/// Links probe and relay links try the LAN, as after a change noticed here.
pub fn notify_changed() {
    sender().send_modify(|version| *version += 1);
}
