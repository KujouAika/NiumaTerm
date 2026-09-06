use crate::unix::ShellUser;

/// The shell launched when configuration names none.
///
/// `$SHELL` is what the user's session already chose; the password database
/// is the fallback for a process started without it (a launchd agent, a bare
/// `login` session). `/bin/sh` is the last resort every POSIX system has.
pub fn default_shell() -> String {
    ShellUser::from_env()
        .ok()
        .map(|user| user.shell)
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| String::from("/bin/sh"))
}

/// Whether the bundled OSC 133 prompt integration can be injected into
/// `shell`.
///
/// There is no POSIX integration script yet, so no shell qualifies and the
/// terminal falls back to untrusted prompt sniffing. Adding one means
/// shipping the script and delivering it through `ZDOTDIR` (zsh) or
/// `--rcfile` (bash) rather than through startup arguments, because neither
/// shell has an equivalent of PowerShell's `-EncodedCommand`.
pub fn supports_prompt_integration(_shell: Option<&str>) -> bool {
    false
}

/// The arguments that make a supported shell evaluate the bundled prompt
/// integration at startup. Empty while no POSIX shell is supported.
pub fn prompt_integration_args() -> Vec<String> {
    Vec::new()
}
