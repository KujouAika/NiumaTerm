#![cfg(unix)]
//! Drives a real zsh through the production PTY path and reads the OSC 133
//! marks back out of the byte stream.
//!
//! The marks only earn boundary trust in a strict `A -> B -> C -> D` order, and
//! the pieces that produce them are spread across a `precmd` hook, a `preexec`
//! hook and a `PS1` suffix that a prompt framework may rebuild. Nothing short
//! of running the shell shows whether they still line up.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::id;
use std::time::{Duration, Instant};
use std::{env, fs, thread};

use nmt_platform::{Pty, create_pty_with_env, prompt_integration};

const DEADLINE: Duration = Duration::from_secs(20);

fn zsh_path() -> Option<&'static str> {
    ["/bin/zsh", "/usr/bin/zsh", "/usr/local/bin/zsh"]
        .into_iter()
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

/// Start zsh with the bundled integration and an empty HOME, so the marks
/// under test are the ones the bundled files emit rather than whatever the
/// developer's own configuration adds.
fn start_zsh(label: &str) -> Option<Session> {
    let zsh = zsh_path()?;
    let integration = prompt_integration(Some(zsh)).expect("zsh reports an integration");

    let home = env::temp_dir().join(format!("nmt-zsh-{label}-{}", id()));
    fs::create_dir_all(&home).expect("temp home");
    let home_value = home.to_string_lossy().into_owned();

    let mut environment: Vec<(String, String)> = integration
        .environment
        .into_iter()
        .filter(|(name, _)| name != "NMT_USER_ZDOTDIR")
        .collect();
    environment.push((String::from("NMT_USER_ZDOTDIR"), home_value.clone()));
    environment.push((String::from("HOME"), home_value));

    match create_pty_with_env(zsh, Vec::new(), &None, 80, 24, &environment, None) {
        Ok(pty) => Some(Session {
            pty,
            home,
            stream: Vec::new(),
        }),
        Err(error) => {
            eprintln!("skipping: could not spawn zsh: {error:?}");
            let _ = fs::remove_dir_all(&home);
            None
        }
    }
}

#[test]
fn zsh_reports_an_ordered_prompt_lifecycle() {
    let Some(mut session) = start_zsh("lifecycle") else {
        eprintln!("skipping: no zsh on this host");
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

/// The exit code is what a finished command block records, so a failure has to
/// travel out as its own status rather than a generic zero.
#[test]
fn zsh_reports_a_failing_commands_exit_code() {
    let Some(mut session) = start_zsh("exit-code") else {
        eprintln!("skipping: no zsh on this host");
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

/// A user `clear` has to be announced in band: the terminal's own scrollback
/// is empty under the block protocol, so nothing else tells it the frozen
/// blocks should drop.
#[test]
fn zsh_announces_a_user_clear() {
    let Some(mut session) = start_zsh("clear") else {
        eprintln!("skipping: no zsh on this host");
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
