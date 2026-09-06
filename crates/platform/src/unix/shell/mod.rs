use std::path::{Path, PathBuf};
use std::process::id;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{env, fs, io};

use tracing::warn;

use crate::PromptIntegration;
use crate::unix::hook_command::single_quoted;
use crate::unix::{SPAWNS_LOGIN_SHELL, ShellUser, environment, filesystem};

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
/// Each shell is reached through the hook that leaves the set of the user's
/// startup files unchanged — turning the integration on must not add or drop
/// any of them. The files both shells need are materialized on first use.
pub fn prompt_integration(shell: Option<&str>) -> Option<PromptIntegration> {
    let shell = resolved_shell(shell);

    match shell_name(&shell)?.as_str() {
        "zsh" => zsh_integration(),
        "bash" => bash_integration(&shell),
        _ => None,
    }
}

/// zsh resolves its whole startup series through `ZDOTDIR`, so pointing it at a
/// directory of forwarders reaches every one of them: each sources the user's
/// counterpart, and `.zshrc` adds the integration last.
fn zsh_integration() -> Option<PromptIntegration> {
    let zdotdir = zsh_directory()?.to_string_lossy().into_owned();

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

/// bash's hook is `--rcfile`, which it honours only as a non-login shell.
///
/// Where the backend spawns login shells, the launch becomes `-lc` plus an
/// `exec` into an interactive shell carrying `--rcfile`. The outer shell is
/// still a login shell, so bash itself runs the profile chain by its own
/// precedence rules, and the environment that builds survives the `exec`. The
/// inner shell then reads our rc in place of `~/.bashrc` — which a login shell
/// would not have read either — so the hop adds the integration without
/// touching the user's own set of files. Where the backend spawns a plain
/// interactive shell, `--rcfile` is taken directly and the rc sources the
/// `~/.bashrc` it stands in for.
fn bash_integration(shell: &str) -> Option<PromptIntegration> {
    let directory = bash_directory()?;
    let rc = directory.join(BASH_RC).to_string_lossy().into_owned();

    let mut environment = vec![(
        String::from("NMT_BASH_INTEGRATION"),
        directory.join(BASH_HOOKS).to_string_lossy().into_owned(),
    )];

    let args = if SPAWNS_LOGIN_SHELL {
        vec![
            String::from("-lc"),
            format!(
                "exec {} --rcfile {} -i",
                single_quoted(shell),
                single_quoted(&rc)
            ),
        ]
    } else {
        environment.push((
            String::from("NMT_BASH_USER_RC"),
            environment::home_dir()
                .map(|home| home.join(".bashrc").to_string_lossy().into_owned())
                .unwrap_or_default(),
        ));

        vec![String::from("--rcfile"), rc]
    };

    Some(PromptIntegration { args, environment })
}

/// The configured shell, or the default when none is configured.
fn resolved_shell(shell: Option<&str>) -> String {
    match shell.map(str::trim).filter(|shell| !shell.is_empty()) {
        Some(shell) => shell.to_owned(),
        None => default_shell(),
    }
}

fn shell_name(shell: &str) -> Option<String> {
    Path::new(shell).file_name()?.to_str().map(str::to_owned)
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

const BASH_RC: &str = "bashrc.bash";
const BASH_HOOKS: &str = "nmt-integration.bash";

const BASH_FILES: [(&str, &str); 2] = [
    (
        BASH_RC,
        include_str!("../../../../../assets/unix/bash/bashrc.bash"),
    ),
    (
        BASH_HOOKS,
        include_str!("../../../../../assets/unix/bash/nmt-integration.bash"),
    ),
];

fn zsh_directory() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

    DIR.get_or_init(|| installed("zsh", &ZSH_FILES)).as_deref()
}

fn bash_directory() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

    DIR.get_or_init(|| installed("bash", &BASH_FILES))
        .as_deref()
}

/// The materialized startup files for one shell, installed once per process.
///
/// A failure to write them yields `None` rather than a path: pointing a shell
/// at startup files that are not there would silently drop the user's own
/// configuration, which is far worse than running without the integration.
fn installed(shell: &str, files: &[(&str, &str)]) -> Option<PathBuf> {
    match install_files(shell, files) {
        Ok(dir) => Some(dir),
        Err(error) => {
            warn!("{shell} shell integration unavailable ({error})");
            None
        }
    }
}

fn install_files(shell: &str, files: &[(&str, &str)]) -> io::Result<PathBuf> {
    let dir = environment::data_dir()
        .join("shell-integration")
        .join(shell);
    fs::create_dir_all(&dir)?;

    for (name, contents) in files {
        write_atomically(&dir.join(name), contents)?;
    }

    Ok(dir)
}

/// Replace a startup file in one step.
///
/// Another instance may be installing the same directory while a shell is
/// starting up, and a shell that sources a half-written rc file loses the rest
/// of the user's configuration.
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
