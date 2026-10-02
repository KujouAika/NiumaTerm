pub use crate::environment_override::override_value;

use std::ffi::CStr;
#[cfg(target_os = "macos")]
use std::ffi::c_void;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::ptr::{null, null_mut};
use std::{env, fs};

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_foundation::NSString;

use crate::APP_ID;

/// Writable per-user state (logs, caches, downloaded updates). Falls back to
/// the temp directory so a sandbox that denies the real location still yields
/// a usable path rather than failing startup.
pub fn data_dir() -> PathBuf {
    let directory = base_data_dir().map(|base| base.join(APP_ID));

    if let Some(directory) = directory
        && fs::create_dir_all(&directory).is_ok()
    {
        return directory;
    }

    env::temp_dir()
}

#[cfg(target_os = "macos")]
fn base_data_dir() -> Option<PathBuf> {
    home_dir().map(|home| application_support_dir(&home))
}

/// Where macOS puts the files one application owns. Configuration is kept here
/// too rather than in an XDG directory: Finder hides dot-directories, Migration
/// Assistant and Time Machine both carry this one to a new machine, and no
/// other Mac application looks in `~/.config`.
#[cfg(target_os = "macos")]
fn application_support_dir(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support")
}

#[cfg(not(target_os = "macos"))]
fn base_data_dir() -> Option<PathBuf> {
    env::var_os("XDG_DATA_HOME")
        .map(|value| -> PathBuf { value.into() })
        .filter(|path| path.is_absolute())
        .or_else(|| home_dir().map(|home| home.join(".local").join("share")))
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(|value| -> PathBuf { value.into() })
        .filter(|home| !home.as_os_str().is_empty())
}

/// Shares the data directory: on macOS the two are one place, so a single
/// installation is one directory rather than a pair that can drift apart.
#[cfg(target_os = "macos")]
pub fn config_dir(home: &Path) -> PathBuf {
    application_support_dir(home).join(APP_ID)
}

#[cfg(not(target_os = "macos"))]
pub fn config_dir(home: &Path) -> PathBuf {
    env::var("XDG_CONFIG_HOME")
        .map(|value| -> PathBuf { value.into() })
        .unwrap_or_else(|_| home.join(".config"))
        .join(APP_ID)
}

/// The name this computer goes by. `HOSTNAME` cannot serve: shells keep it
/// unexported, and an application started from Finder or the Dock inherits
/// launchd's environment, which never had it.
#[cfg(target_os = "macos")]
pub fn computer_name() -> Option<String> {
    sharing_name().or_else(kernel_host_name)
}

#[cfg(not(target_os = "macos"))]
pub fn computer_name() -> Option<String> {
    kernel_host_name()
}

/// The name set under System Settings > General > Sharing, the one AirDrop
/// and Finder show. Unlike the kernel host name it keeps spaces and
/// non-ASCII letters, and DHCP does not replace it.
#[cfg(target_os = "macos")]
fn sharing_name() -> Option<String> {
    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCDynamicStoreCopyComputerName(
            store: *const c_void,
            encoding: *mut u32,
        ) -> *mut NSString;
    }

    // SAFETY: a null store selects the system store; a null encoding pointer is
    // allowed. The result is a +1 CFString, toll-free bridged to NSString, so
    // `Retained` takes over the reference the copy returned.
    let name = unsafe { Retained::from_raw(SCDynamicStoreCopyComputerName(null(), null_mut())) }?;

    Some(name.to_string()).filter(|name| !name.is_empty())
}

/// The kernel's host name without the domain of a fully qualified name.
fn kernel_host_name() -> Option<String> {
    let mut buffer = [0 as libc::c_char; 256];

    // SAFETY: the buffer outlives the call and the length leaves room for
    // the terminator.
    if unsafe { libc::gethostname(buffer.as_mut_ptr(), buffer.len() - 1) } != 0 {
        return None;
    }

    // SAFETY: `gethostname` succeeded, and the reserved final byte guarantees
    // a terminator even when the name filled the buffer.
    let name = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().ok()?;

    name.split('.')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

pub const DEFAULT_EDITOR: &str = "vi";
