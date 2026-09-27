use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::task::Poll;
use std::time::Duration;

use nmt_config::CursorShape;
use nmt_config::colors::Colors;
use nmt_platform::{AsyncPty, PtyOptions, create_pty_with_env, runtime};
use nmt_remote_core::identity::DeviceKey;
use nmt_remote_core::messages::{DeviceInfo, DeviceKind};
use nmt_remote_core::pairing::PairingCode;
use nmt_remote_core::rpc::Origin;
use nmt_terminal::event::VoidListener;
use nmt_terminal::termio::{SessionHandles, SessionOptions, start_session};
use tokio::time::{sleep, timeout};

use crate::NetworkPty;
use crate::client::pair;
use crate::connection::{RemoteHost, Status};
use crate::host::{HostConfig, HostService};
use crate::sessions::{SessionRegistry, TerminalControl};
use crate::store::PairedHost;

const WAIT: Duration = Duration::from_secs(20);

fn info(name: &str) -> DeviceInfo {
    DeviceInfo {
        name: name.into(),
        kind: DeviceKind::Desktop,
        platform: "test".into(),
        app_version: "0.0.0".into(),
    }
}

/// A shell without prompt integration, so output is plain and fast to start.
fn test_shell() -> (Option<String>, Vec<String>) {
    if cfg!(windows) {
        (Some("cmd.exe".into()), vec!["/Q".into()])
    } else {
        (Some("/bin/sh".into()), vec!["-i".into()])
    }
}

/// A command whose output differs from the echo of its typed line.
fn marker_command(tag: &str) -> (Vec<u8>, String) {
    if cfg!(windows) {
        (
            format!("echo {tag}_%OS%_DONE\r").into_bytes(),
            format!("{tag}_Windows_NT_DONE"),
        )
    } else {
        (
            format!("echo {tag}_$((6*7))_DONE\r").into_bytes(),
            format!("{tag}_42_DONE"),
        )
    }
}

fn start_host(dir: &tempfile::TempDir, registry: Arc<SessionRegistry>) -> HostService {
    let (shell, args) = test_shell();

    HostService::start(
        dir.path().to_path_buf(),
        DeviceKey::generate().unwrap(),
        HostConfig {
            port: 0,
            device: info("Host"),
            shell,
            args,
            registry,
            relay: None,
            on_change: Arc::new(|| {}),
        },
    )
    .unwrap()
}

async fn paired_client(host: &HostService) -> (PairedHost, Arc<DeviceKey>) {
    let address = format!("127.0.0.1:{}", host.local_addr().port());
    let key = DeviceKey::generate().unwrap();
    let code = host.start_pairing().unwrap();

    let paired = pair(Some(&address), &code, &key, info("Client"), None, None)
        .await
        .unwrap();

    (paired, Arc::new(key))
}

fn remote(paired: PairedHost, key: Arc<DeviceKey>) -> Arc<RemoteHost> {
    RemoteHost::new(paired, key, "0.0.0".into(), |_| {})
}

/// Read until `marker` shows up, reporting whether a stream reset came
/// along the way.
async fn read_until(pty: &mut NetworkPty, marker: &str) -> bool {
    let mut seen = Vec::new();
    let mut buf = [0; 4096];
    let mut restarted = false;

    timeout(WAIT, async {
        while !String::from_utf8_lossy(&seen).contains(marker) {
            let read = poll_fn(|cx| pty.poll_read(cx, &mut buf)).await.unwrap();

            assert_ne!(read, 0, "the terminal ended before {marker:?}");

            restarted |= pty.take_stream_reset();

            seen.extend_from_slice(&buf[..read]);
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{marker:?} never arrived"));

    restarted
}

async fn run_marker(pty: &mut NetworkPty, tag: &str) {
    let (command, output) = marker_command(tag);

    poll_fn(|cx| pty.poll_write(cx, &command)).await.unwrap();

    read_until(pty, &output).await;
}

async fn wait_status(host: &RemoteHost, wanted: Status) {
    let mut status = host.status();

    timeout(WAIT, status.wait_for(|status| *status == wanted))
        .await
        .unwrap_or_else(|_| panic!("status never became {wanted:?}"))
        .unwrap();
}

#[test]
fn paired_client_runs_a_command_in_a_host_terminal() {
    let host_dir = tempfile::tempdir().unwrap();
    let host = start_host(&host_dir, SessionRegistry::new());

    runtime().block_on(async {
        let (paired, key) = paired_client(&host).await;

        assert_eq!(host.devices().len(), 1);
        assert!(host.pairing().is_none(), "a used code is gone");

        let remote = remote(paired, key);

        let mut pty = remote.open_terminal(80, 24).await.unwrap();

        // The first bytes are the attach checkpoint, a stream reset.
        assert!(read_until(&mut pty, "\x1bc").await);

        run_marker(&mut pty, "FIRST").await;

        assert_eq!(host.terminal_count(), 1);

        // Closing a view detaches; ending the session is explicit.
        let session = pty.session().to_owned();

        drop(pty);
        sleep(Duration::from_millis(200)).await;

        assert_eq!(host.terminal_count(), 1);

        remote.terminate(&session);

        timeout(WAIT, async {
            while host.terminal_count() != 0 {
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the host terminal closes when terminated");
    });
}

#[test]
fn a_view_survives_a_dropped_link_and_resumes_from_a_checkpoint() {
    let host_dir = tempfile::tempdir().unwrap();
    let host = start_host(&host_dir, SessionRegistry::new());

    runtime().block_on(async {
        let (paired, key) = paired_client(&host).await;
        let remote = remote(paired, key);

        let mut pty = remote.open_terminal(80, 24).await.unwrap();

        run_marker(&mut pty, "BEFORE").await;

        host.drop_connections();

        wait_status(&remote, Status::Reconnecting).await;
        wait_status(&remote, Status::Connected).await;

        // Output the old link delivered may still be queued; skip to the
        // replay, which restarts the stream and still shows the earlier
        // command because the session kept running.
        let mut buf = [0; 4096];

        timeout(WAIT, async {
            loop {
                poll_fn(|cx| pty.poll_read(cx, &mut buf)).await.unwrap();

                if pty.take_stream_reset() {
                    break;
                }
            }
        })
        .await
        .expect("the view restarts from a checkpoint");

        let replay = String::from_utf8_lossy(&buf);

        assert!(replay.contains("\x1bc"));

        // Input sent before the reattach completes would be dropped, which
        // the replay above rules out.
        let (command, output) = marker_command("AFTER");

        poll_fn(|cx| pty.poll_write(cx, &command)).await.unwrap();

        read_until(&mut pty, &output).await;
    });
}

#[test]
fn host_tabs_are_listed_attachable_and_not_closable_remotely() {
    let host_dir = tempfile::tempdir().unwrap();
    let registry = SessionRegistry::new();
    let host = start_host(&host_dir, Arc::clone(&registry));
    let (shell, args) = test_shell();

    // A host tab: the host owns the session and registers it.
    let tab: SessionHandles = start_session(
        create_pty_with_env(PtyOptions {
            shell: shell.as_deref().unwrap(),
            args: &args,
            working_directory: None,
            columns: 80,
            rows: 24,
            environment_overrides: &[],
            starting_title: None,
            bootstrap: None,
        })
        .unwrap(),
        VoidListener,
        SessionOptions {
            cols: 80,
            rows: 24,
            route_id: 0,
            colors: Colors::default(),
            cursor_shape: CursorShape::Block,
            scrollback_lines: 1000,
            engine_blocks: false,
            terminal_responses: true,
        },
    )
    .unwrap();

    let session = registry.register_tab(
        "Tab".into(),
        80,
        24,
        TerminalControl {
            messenger: tab.messenger.clone(),
            claimed_remotely: Arc::new(AtomicBool::new(false)),
        },
    );

    let (paired, key) = runtime().block_on(paired_client(&host));
    let remote = remote(paired, key);

    // Tabs open views from the UI thread, outside any runtime context.
    let mut pty = remote.view(session.clone());

    runtime().block_on(async {
        let sessions = remote.list_sessions().await.unwrap();

        assert!(
            sessions
                .iter()
                .any(|info| info.session == session && info.origin == Origin::Tab)
        );

        run_marker(&mut pty, "TAB").await;

        remote.terminate(&session);

        sleep(Duration::from_millis(300)).await;

        assert!(registry.list().iter().any(|info| info.session == session));
    });

    registry.unregister(&session);
}

#[test]
fn a_view_ends_when_its_shell_exits() {
    let host_dir = tempfile::tempdir().unwrap();
    let host = start_host(&host_dir, SessionRegistry::new());

    runtime().block_on(async {
        let (paired, key) = paired_client(&host).await;
        let remote = remote(paired, key);

        let mut pty = remote.open_terminal(80, 24).await.unwrap();

        run_marker(&mut pty, "ALIVE").await;

        poll_fn(|cx| pty.poll_write(cx, b"exit\r")).await.unwrap();

        let mut buf = [0; 4096];

        timeout(WAIT, async {
            loop {
                let read = poll_fn(|cx| pty.poll_read(cx, &mut buf)).await.unwrap();

                if read == 0 {
                    break;
                }
            }
        })
        .await
        .expect("the view ends with its shell");

        assert!(poll_fn(|cx| Poll::Ready(pty.poll_exit(cx).is_ready())).await);
    });
}

#[test]
fn unpaired_and_revoked_devices_are_refused() {
    let host_dir = tempfile::tempdir().unwrap();
    let host = start_host(&host_dir, SessionRegistry::new());
    let address = format!("127.0.0.1:{}", host.local_addr().port());

    runtime().block_on(async {
        let wrong = PairingCode::generate().unwrap();

        host.start_pairing().unwrap();

        assert!(
            pair(
                Some(&address),
                &wrong,
                &DeviceKey::generate().unwrap(),
                info("Guess"),
                None,
                None
            )
            .await
            .is_err()
        );

        let (paired, key) = paired_client(&host).await;

        let stranger = remote(paired.clone(), Arc::new(DeviceKey::generate().unwrap()));

        assert!(stranger.list_sessions().await.is_err());

        wait_status(&stranger, Status::Refused).await;

        let client = remote(paired, Arc::clone(&key));

        client.list_sessions().await.unwrap();

        host.remove_device(&key.id()).unwrap();

        wait_status(&client, Status::Refused).await;
    });
}

/// Needs a network interface that carries multicast, which CI runners and
/// some VPNs lack.
#[test]
#[ignore = "needs LAN multicast"]
fn pairing_without_an_address_finds_the_host_showing_the_code() {
    let host_dir = tempfile::tempdir().unwrap();
    let host = start_host(&host_dir, SessionRegistry::new());
    let key = DeviceKey::generate().unwrap();
    let code = host.start_pairing().unwrap();

    runtime().block_on(async {
        let mut paired = pair(None, &code, &key, info("Client"), None, None)
            .await
            .unwrap();

        assert_eq!(paired.id, host.device_id());

        // A stale address falls back to finding the host by its id.
        paired.lan_hints = vec!["127.0.0.1:1".into()];

        let client = remote(paired, Arc::new(key));

        client.list_sessions().await.unwrap();
    });
}
