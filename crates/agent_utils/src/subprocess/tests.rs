use std::sync::mpsc::channel;
use std::time::Duration;

use nmt_platform::process::hidden_command;

use crate::subprocess::JsonLineProcess;

#[test]
fn stdout_close_callback_follows_the_last_json_message() {
    // Built the way production callers do: `JsonLineProcess` contains the
    // child it spawns, which on Unix requires a command that leads its own
    // process group.
    #[cfg(windows)]
    let command = {
        let mut command = hidden_command("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Out.WriteLine('{\"ready\":true}')",
        ]);
        command
    };
    #[cfg(unix)]
    let command = {
        let mut command = hidden_command("/bin/sh");
        command.args(["-c", "echo '{\"ready\":true}'"]);
        command
    };
    let (message_tx, message_rx) = channel();
    let (closed_tx, closed_rx) = channel();
    let mut process = JsonLineProcess::spawn_with_stdout_closed(
        command,
        "test-json-process",
        "Test",
        move |message| {
            let _ = message_tx.send(message);
        },
        |_| {},
        move || {
            let _ = closed_tx.send(());
        },
    )
    .expect("test process should start");

    assert_eq!(
        message_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("json message")["ready"],
        true
    );
    closed_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("stdout close callback");
    process
        .shutdown(Duration::from_secs(1), false)
        .expect("exited process should be observable");
}
