#![cfg(unix)]
//! Drives a real shell through the production PTY path and reads the OSC 133
//! marks back out of the byte stream.
//!
//! The marks only earn boundary trust in a strict `A -> B -> C -> D` order, and
//! the pieces that produce them are spread across a `precmd` hook, a `preexec`
//! hook and a `PS1` suffix that a prompt framework may rebuild — and bash
//! reaches its own through a login hop that `exec`s a second shell. Nothing
//! short of running them shows whether the pieces still line up.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio, id};
use std::time::{Duration, Instant};
use std::{env, fs, thread};

use nmt_platform::{Pty, create_pty_with_env, prompt_integration};

const DEADLINE: Duration = Duration::from_secs(20);

fn shell_path(name: &str) -> Option<String> {
    ["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"]
        .into_iter()
        .map(|dir| format!("{dir}/{name}"))
        .find(|candidate| Path::new(candidate).is_file())
}

/// The `133;` mark payloads in the order they appear: `A`, `B`, `C`, `D;0`.
fn marks(stream: &[u8]) -> Vec<String> {
    const INTRODUCER: &str = "\u{1b}]133;";

    let text = String::from_utf8_lossy(stream);
    let mut found = Vec::new();
    let mut rest = text.as_ref();

    while let Some(at) = rest.find(INTRODUCER) {
        rest = &rest[at + INTRODUCER.len()..];
        let end = rest.find(['\u{7}', '\u{1b}']).unwrap_or(rest.len());
        found.push(rest[..end].to_owned());
        rest = &rest[end..];
    }

    found
}

struct Session {
    pty: Pty,
    home: PathBuf,
    /// Everything read so far, so a wait can be expressed against the whole
    /// session rather than against one read's worth of bytes.
    stream: Vec<u8>,
}

impl Session {
    /// Read until `done` accepts the marks seen so far, or the deadline passes.
    fn read_until(&mut self, done: impl Fn(&[String]) -> bool) -> Vec<String> {
        let mut buf = [0u8; 8192];
        let deadline = Instant::now() + DEADLINE;

        while Instant::now() < deadline {
            match self.pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    self.stream.extend_from_slice(&buf[..n]);
                    let seen = marks(&self.stream);
                    if done(&seen) {
                        return seen;
                    }
                }
                // The PTY is non-blocking, so "nothing yet" arrives as an
                // error rather than a short read.
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        }

        marks(&self.stream)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.pty.write_all(b"exit\n");
        let _ = fs::remove_dir_all(&self.home);
    }
}

/// Start `shell` with the bundled integration and an empty HOME, so the marks
/// under test are the ones the bundled files emit rather than whatever the
/// developer's own configuration adds.
fn start(shell: &str, label: &str) -> Option<Session> {
    start_with_startup_files(shell, label, &[])
}

/// `startup_files` are written into the empty home before the shell runs, so a
/// test can check that the user's own configuration still reaches the session.
fn start_with_startup_files(
    shell: &str,
    label: &str,
    startup_files: &[(&str, &str)],
) -> Option<Session> {
    let Some(program) = shell_path(shell) else {
        eprintln!("skipping: no {shell} on this host");
        return None;
    };
    let integration = prompt_integration(Some(&program)).expect("the shell reports an integration");

    let home = env::temp_dir().join(format!("nmt-{shell}-{label}-{}", id()));
    fs::create_dir_all(&home).expect("temp home");
    for (name, contents) in startup_files {
        fs::write(home.join(name), contents).expect("startup file");
    }
    let home_value = home.to_string_lossy().into_owned();

    let mut environment = integration.environment;
    environment.push((String::from("HOME"), home_value.clone()));
    if shell == "zsh" {
        // `/usr/bin/login` resets HOME from the password database whatever the
        // caller passes, so an empty home only isolates zsh if the bootstrap
        // is pointed at it the way zsh itself would be. `login -p` does keep
        // ZDOTDIR.
        environment.push((String::from("ZDOTDIR"), home_value));
    }

    match create_pty_with_env(
        &program,
        integration.args,
        &None,
        80,
        24,
        &environment,
        None,
        integration.bootstrap.as_deref(),
    ) {
        Ok(pty) => Some(Session {
            pty,
            home,
            stream: Vec::new(),
        }),
        Err(error) => {
            eprintln!("skipping: could not spawn {shell}: {error:?}");
            let _ = fs::remove_dir_all(&home);
            None
        }
    }
}

fn assert_ordered_lifecycle(shell: &str) {
    let Some(mut session) = start(shell, "lifecycle") else {
        return;
    };

    // The synthetic prime (A, B, C), the D that closes it, then the real
    // prompt's own A and B.
    let primed = session.read_until(|seen| seen.len() >= 6);

    assert!(
        primed.len() >= 6,
        "the first prompt produced only {primed:?} within {DEADLINE:?}"
    );
    assert_eq!(
        &primed[..6],
        &["A", "B", "C", "D;0", "A", "B"],
        "the prime must complete an ordered lifecycle before the first prompt"
    );

    session.pty.write_all(b"true\n").expect("write command");

    // `preexec` closes the command line (C) and the next `precmd` reports the
    // exit status (D) before opening the following prompt (A).
    let after = session.read_until(|seen| seen.len() >= 9);

    assert!(
        after.len() >= 9,
        "running a command produced only {after:?} within {DEADLINE:?}"
    );
    assert_eq!(
        &after[6..9],
        &["C", "D;0", "A"],
        "a command must run inside C -> D and be followed by the next prompt"
    );
}

#[test]
fn zsh_reports_an_ordered_prompt_lifecycle() {
    assert_ordered_lifecycle("zsh");
}

#[test]
fn bash_reports_an_ordered_prompt_lifecycle() {
    assert_ordered_lifecycle("bash");
}

/// The exit code is what a finished command block records, so a failure has to
/// travel out as its own status rather than a generic zero.
fn assert_reports_failing_exit_code(shell: &str) {
    let Some(mut session) = start(shell, "exit-code") else {
        return;
    };

    session.read_until(|seen| seen.len() >= 6);
    session.pty.write_all(b"false\n").expect("write command");

    let seen = session.read_until(|seen| seen.iter().any(|mark| mark == "D;1"));

    assert!(
        seen.iter().any(|mark| mark == "D;1"),
        "a failing command must report its status; saw {seen:?}"
    );
}

#[test]
fn zsh_reports_a_failing_commands_exit_code() {
    assert_reports_failing_exit_code("zsh");
}

#[test]
fn bash_reports_a_failing_commands_exit_code() {
    assert_reports_failing_exit_code("bash");
}

/// A user `clear` has to be announced in band: the terminal's own scrollback
/// is empty under the block protocol, so nothing else tells it the frozen
/// blocks should drop.
fn assert_announces_user_clear(shell: &str) {
    let Some(mut session) = start(shell, "clear") else {
        return;
    };

    session.read_until(|seen| seen.len() >= 6);
    session.pty.write_all(b"clear\n").expect("write command");

    let seen = session.read_until(|seen| seen.iter().any(|mark| mark == "K"));

    assert!(
        seen.iter().any(|mark| mark == "K"),
        "clear must announce itself before erasing; saw {seen:?}"
    );
}

#[test]
fn zsh_announces_a_user_clear() {
    assert_announces_user_clear("zsh");
}

#[test]
fn bash_announces_a_user_clear() {
    assert_announces_user_clear("bash");
}

/// The integration must not cost the user their own configuration: the launch
/// suppresses zsh's startup files, so the bootstrap has to put every one of
/// them back.
#[test]
fn zsh_still_reads_the_users_startup_files() {
    let Some(mut session) = start_with_startup_files(
        "zsh",
        "startup",
        &[(".zshrc", "export NMT_TEST_STARTUP=reached\n")],
    ) else {
        return;
    };

    session.read_until(|seen| seen.len() >= 6);
    session
        .pty
        .write_all(b"printf 'marker=[%s]\\n' \"$NMT_TEST_STARTUP\"\n")
        .expect("write command");

    let deadline = Instant::now() + DEADLINE;
    let mut buf = [0u8; 8192];
    while Instant::now() < deadline {
        match session.pty.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                session.stream.extend_from_slice(&buf[..n]);
                if String::from_utf8_lossy(&session.stream).contains("marker=[reached]") {
                    return;
                }
            }
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }

    panic!(
        "zsh did not read the user's startup files; saw {}",
        String::from_utf8_lossy(&session.stream)
    );
}

/// The same for bash, exercised by running the generated launch arguments
/// directly rather than through the PTY: bash finds its startup files through
/// `$HOME`, and `/usr/bin/login` — which the macOS PTY path goes through —
/// resets that from the password database no matter what the caller passes.
#[test]
fn bash_still_reads_the_users_startup_files() {
    let Some(bash) = shell_path("bash") else {
        eprintln!("skipping: no bash on this host");
        return;
    };
    let integration = prompt_integration(Some(&bash)).expect("bash is integrated");

    let home = env::temp_dir().join(format!("nmt-bash-startup-{}", id()));
    fs::create_dir_all(&home).expect("temp home");
    // Whichever of the two the host's launch shape makes bash read — the
    // profile chain under the login hop, `.bashrc` otherwise — the marker is
    // set.
    for name in [".bash_profile", ".bashrc"] {
        fs::write(home.join(name), "export NMT_TEST_STARTUP=reached\n").expect("startup file");
    }
    let home_value = home.to_string_lossy().into_owned();

    let mut command = Command::new(&bash);
    command.args(&integration.args);
    for (name, value) in &integration.environment {
        let value = match name.as_str() {
            "NMT_BASH_USER_RC" => home.join(".bashrc").to_string_lossy().into_owned(),
            _ => value.clone(),
        };
        command.env(name, value);
    }
    command
        .env("HOME", &home_value)
        .env_remove("NMT_TEST_STARTUP")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = command.spawn().expect("spawn bash");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"printf 'marker=[%s]\\n' \"$NMT_TEST_STARTUP\"\nexit\n")
        .expect("write command");

    let output = child.wait_with_output().expect("bash exits");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();

    let _ = fs::remove_dir_all(&home);

    assert!(
        text.contains("marker=[reached]"),
        "bash did not read the user's startup files; saw {text}"
    );
}

/// Pressing Enter on an empty line must not cost boundary trust: no command
/// runs, so the shell's pre-execution hook never fires, and a `;D` arriving
/// straight after `;B` is an out-of-order lifecycle.
fn assert_empty_enter_keeps_the_lifecycle_ordered(shell: &str) {
    let Some(mut session) = start(shell, "empty-enter") else {
        return;
    };

    session.read_until(|seen| seen.len() >= 6);
    session.pty.write_all(b"\n").expect("write empty line");

    let seen = session.read_until(|seen| seen.len() >= 9);

    assert_eq!(
        &seen[6..9],
        &["C", "D;0", "A"],
        "an empty line must still close its command region; saw {seen:?}"
    );
}

#[test]
fn zsh_empty_enter_keeps_the_lifecycle_ordered() {
    assert_empty_enter_keeps_the_lifecycle_ordered("zsh");
}

#[test]
fn bash_empty_enter_keeps_the_lifecycle_ordered() {
    assert_empty_enter_keeps_the_lifecycle_ordered("bash");
}

/// The prompt-end mark is re-applied on every prompt, so the strip that
/// precedes it has to actually match: without it PS1 would grow by one marker
/// per prompt, and the terminal would see the prompt region close early.
fn assert_the_prompt_mark_does_not_accumulate(shell: &str) {
    let Some(mut session) = start(shell, "no-accumulate") else {
        return;
    };

    session.read_until(|seen| seen.len() >= 6);

    for _ in 0..4 {
        session.pty.write_all(b"true\n").expect("write command");
    }

    // Four commands past the priming six marks: C, D, A, B each.
    let seen = session.read_until(|seen| seen.len() >= 6 + 4 * 4);

    assert_eq!(
        &seen[6..],
        &[
            "C", "D;0", "A", "B", "C", "D;0", "A", "B", "C", "D;0", "A", "B", "C", "D;0", "A", "B"
        ],
        "each prompt must carry exactly one B; saw {seen:?}"
    );
}

#[test]
fn zsh_prompt_mark_does_not_accumulate() {
    assert_the_prompt_mark_does_not_accumulate("zsh");
}

#[test]
fn bash_prompt_mark_does_not_accumulate() {
    assert_the_prompt_mark_does_not_accumulate("bash");
}

/// bash allows one DEBUG trap, and the integration installs one for `;C`. A
/// trap the user's own files put there — bash-preexec, atuin — must keep
/// firing rather than be silently replaced.
///
/// Run through the generated launch arguments rather than the PTY for the same
/// reason as the startup-file case: `/usr/bin/login` resets `$HOME`, so the
/// files that would install such a trap cannot be placed where bash looks.
#[test]
fn bash_keeps_a_debug_trap_the_user_already_installed() {
    let Some(bash) = shell_path("bash") else {
        eprintln!("skipping: no bash on this host");
        return;
    };
    let integration = prompt_integration(Some(&bash)).expect("bash is integrated");

    let home = env::temp_dir().join(format!("nmt-bash-debugtrap-{}", id()));
    fs::create_dir_all(&home).expect("temp home");
    let trap = "trap 'printf \"USERTRAP[%s]\\n\" \"$BASH_COMMAND\"' DEBUG\n";
    for name in [".bash_profile", ".bashrc"] {
        fs::write(home.join(name), trap).expect("startup file");
    }
    let home_value = home.to_string_lossy().into_owned();

    let mut command = Command::new(&bash);
    command.args(&integration.args);
    for (name, value) in &integration.environment {
        let value = match name.as_str() {
            "NMT_BASH_USER_RC" => home.join(".bashrc").to_string_lossy().into_owned(),
            _ => value.clone(),
        };
        command.env(name, value);
    }
    command
        .env("HOME", &home_value)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = command.spawn().expect("spawn bash");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"echo probe-command\nexit\n")
        .expect("write command");

    let output = child.wait_with_output().expect("bash exits");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();

    let _ = fs::remove_dir_all(&home);

    assert!(
        text.contains("USERTRAP[echo probe-command]"),
        "the user's DEBUG trap stopped firing; saw {text}"
    );
}

/// An `exec` carries the environment across and nothing else, so a profile
/// chain replayed on the far side of the login hop would hand the user a shell
/// with their exported variables but none of their functions, aliases or
/// traps. The chain has to run in the shell they actually get.
#[test]
fn bash_keeps_functions_and_aliases_from_the_users_profile() {
    let Some(bash) = shell_path("bash") else {
        eprintln!("skipping: no bash on this host");
        return;
    };
    let integration = prompt_integration(Some(&bash)).expect("bash is integrated");

    let home = env::temp_dir().join(format!("nmt-bash-funcs-{}", id()));
    fs::create_dir_all(&home).expect("temp home");
    let profile = "nmt_probe_func() { :; }\nalias nmt_probe_alias='true'\n";
    for name in [".bash_profile", ".bashrc"] {
        fs::write(home.join(name), profile).expect("startup file");
    }

    let mut command = Command::new(&bash);
    command.args(&integration.args);
    for (name, value) in &integration.environment {
        command.env(name, value);
    }
    command
        .env("HOME", home.to_string_lossy().into_owned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = command.spawn().expect("spawn bash");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(
            b"printf 'func=[%s] alias=[%s]\\n' \"$(type -t nmt_probe_func)\" \
              \"$(alias nmt_probe_alias >/dev/null 2>&1 && echo yes)\"\nexit\n",
        )
        .expect("write command");

    let output = child.wait_with_output().expect("bash exits");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();

    let _ = fs::remove_dir_all(&home);

    assert!(
        text.contains("func=[function] alias=[yes]"),
        "the user's functions and aliases did not survive the launch; saw {text}"
    );
}
