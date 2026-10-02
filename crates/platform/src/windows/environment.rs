pub use crate::environment_override::override_value;

use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::{env, fs};

use windows_sys::Win32::System::SystemInformation::{
    ComputerNamePhysicalDnsHostname, GetComputerNameExW,
};

use crate::APP_ID;

pub fn data_dir() -> PathBuf {
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        let directory = Path::new(&local).join(APP_ID);

        if fs::create_dir_all(&directory).is_ok() {
            return directory;
        }
    }

    env::temp_dir()
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(Into::into)
}

pub fn config_dir(home: &Path) -> PathBuf {
    home.join("AppData").join("Local").join(APP_ID)
}

/// The name this computer goes by. The DNS host name keeps the case the user
/// typed when naming the computer, where `COMPUTERNAME` holds the uppercased
/// NetBIOS form; the variable remains the fallback.
pub fn computer_name() -> Option<String> {
    dns_host_name().or_else(|| {
        env::var("COMPUTERNAME")
            .ok()
            .filter(|name| !name.is_empty())
    })
}

fn dns_host_name() -> Option<String> {
    let mut len = 0;

    // SAFETY: a null buffer with a zero length is the documented size query;
    // the call stores the needed length, terminator included.
    unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, null_mut(), &mut len) };

    if len == 0 {
        return None;
    }

    let mut buffer = vec![0u16; len as usize];

    // SAFETY: the buffer holds `len` units, as the length argument states.
    if unsafe {
        GetComputerNameExW(
            ComputerNamePhysicalDnsHostname,
            buffer.as_mut_ptr(),
            &mut len,
        )
    } == 0
    {
        return None;
    }

    // On success `len` counts the name without its terminator.
    String::from_utf16(&buffer[..len as usize])
        .ok()
        .filter(|name| !name.is_empty())
}

pub const DEFAULT_EDITOR: &str = "notepad";
