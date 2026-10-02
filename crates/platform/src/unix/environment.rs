pub use crate::environment_override::override_value;

use std::path::{Path, PathBuf};
use std::{env, fs};

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

pub const DEFAULT_EDITOR: &str = "vi";
