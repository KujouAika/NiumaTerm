use std::path::{Path, PathBuf};
use std::process::id;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{env, fs, io};

use tracing::warn;

use crate::PromptIntegration;
use crate::unix::{ShellUser, environment, filesystem};

/// The shell launched when configuration names none.
///
/// `$SHELL` is what the user's session already chose; the password database is
/// the fallback for a process started without it (a launchd agent, a bare
/// `login` session). `/bin/sh` is the last resort every POSIX system has.
pub fn default_shell() -> String {
    ShellUser::from_env()
        .ok()
        .map(|user| user.shell)
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| String::from("/bin/sh"))
}

/// How `shell` must be launched so it emits the bundled OSC 133 prompt marks,
/// or `None` when no integration is available for it.
///
/// zsh is reached through `ZDOTDIR`, pointed at a directory of forwarders this
/// call materializes on first use. bash has no counterpart yet: `--rcfile` is
/// the equivalent hook, and bash ignores it when started as a login shell,
/// which is how the macOS path launches every shell so the child inherits a
/// login environment.
pub fn prompt_integration(shell: Option<&str>) -> Option<PromptIntegration> {
    if shell_name(shell)? != "zsh" {
        return None;
    }

    let zdotdir = zsh_zdotdir()?.to_string_lossy().into_owned();

    Some(PromptIntegration {
        args: Vec::new(),
        environment: vec![
            // `ZDOTDIR` is what zsh resolves its startup series through;
            // `NMT_ZDOTDIR` keeps a stable reference to the same directory
            // after `.zshrc` hands `ZDOTDIR` back to the user.
            (String::from("ZDOTDIR"), zdotdir.clone()),
            (String::from("NMT_ZDOTDIR"), zdotdir.clone()),
            (String::from("NMT_USER_ZDOTDIR"), user_zdotdir(&zdotdir)),
        ],
    })
}

/// The file name of the configured shell, or of the default shell when none is
/// configured.
fn shell_name(shell: Option<&str>) -> Option<String> {
    let configured = shell.map(str::trim).filter(|shell| !shell.is_empty());
    let shell = match configured {
        Some(shell) => shell.to_owned(),
        None => default_shell(),
    };

    Path::new(&shell).file_name()?.to_str().map(str::to_owned)
}

/// The directory zsh should resolve the user's own startup files from.
///
/// A non-interactive child of an integrated shell still carries the terminal's
/// `ZDOTDIR` — `.zshrc` restores it, and `.zshenv` alone does not — so an app
/// launched from one would otherwise name the integration directory as the
/// user's and make every forwarder source itself.
fn user_zdotdir(integration_dir: &str) -> String {
    env::var("ZDOTDIR")
        .ok()
        .filter(|dir| !dir.is_empty() && dir != integration_dir)
        .or_else(|| environment::home_dir().map(|home| home.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

const ZSH_FILES: [(&str, &str); 4] = [
    (
        ".zshenv",
        include_str!("../../../../../assets/unix/zsh/.zshenv"),
    ),
    (
        ".zprofile",
        include_str!("../../../../../assets/unix/zsh/.zprofile"),
    ),
    (
        ".zshrc",
        include_str!("../../../../../assets/unix/zsh/.zshrc"),
    ),
    (
        "nmt-integration.zsh",
        include_str!("../../../../../assets/unix/zsh/nmt-integration.zsh"),
    ),
];

/// The materialized `ZDOTDIR`, installed once per process.
///
/// A failure to write it yields `None` rather than a path: launching zsh with
/// a `ZDOTDIR` that has no startup files would silently drop the user's own
/// configuration, which is far worse than running without the integration.
fn zsh_zdotdir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

    DIR.get_or_init(|| match install_zsh_files() {
        Ok(dir) => Some(dir),
        Err(error) => {
            warn!("zsh shell integration unavailable ({error})");
            None
        }
    })
    .as_deref()
}

fn install_zsh_files() -> io::Result<PathBuf> {
    let dir = environment::data_dir()
        .join("shell-integration")
        .join("zsh");
    fs::create_dir_all(&dir)?;

    for (name, contents) in ZSH_FILES {
        write_atomically(&dir.join(name), contents)?;
    }

    Ok(dir)
}

/// Replace a startup file in one step.
///
/// Another instance may be installing the same directory while a shell is
/// starting up, and a shell that sources a half-written `.zshrc` loses the
/// rest of the user's configuration.
fn write_atomically(path: &Path, contents: &str) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "startup file has no name"))?;
    // The pid separates concurrent installs by different instances, and the
    // counter separates concurrent calls inside one — two callers sharing a
    // staging path would rename each other's file away.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let staging = path.with_file_name(format!(
        "{name}.{}-{}.staging",
        id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));

    fs::write(&staging, contents)?;
    filesystem::replace_file(&staging, path).inspect_err(|_| {
        let _ = fs::remove_file(&staging);
    })
}

#[cfg(test)]
mod tests;
