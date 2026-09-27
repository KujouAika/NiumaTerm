use std::future::poll_fn;
use std::sync::Arc;
use std::time::Duration;

use nmt_platform::{AsyncPty, runtime};
use nmt_remote_core::identity::DeviceKey;
use nmt_remote_core::messages::{DeviceInfo, DeviceKind};
use nmt_remote_core::pairing::PairingCode;
use tokio::time::timeout;

use crate::NetworkPty;
use crate::client::{connect, pair};
use crate::host::{HostConfig, HostService};

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

fn start_host(dir: &tempfile::TempDir) -> HostService {
    let (shell, args) = test_shell();

    HostService::start(
        dir.path().to_path_buf(),
        DeviceKey::generate().unwrap(),
        HostConfig {
            port: 0,
            device: info("Host"),
            shell,
            args,
            on_change: Arc::new(|| {}),
        },
    )
    .unwrap()
}

/// Read until `marker` shows up in the terminal's output.
async fn read_until(pty: &mut NetworkPty, marker: &str) -> String {
    let mut seen = Vec::new();
    let mut buf = [0; 4096];

    while !String::from_utf8_lossy(&seen).contains(marker) {
        let read = poll_fn(|cx| pty.poll_read(cx, &mut buf)).await.unwrap();

        assert_ne!(read, 0, "the terminal ended before {marker:?}");

        seen.extend_from_slice(&buf[..read]);
    }

    String::from_utf8_lossy(&seen).into_owned()
}

#[test]
fn paired_client_runs_a_command_in_a_host_terminal() {
    let host_dir = tempfile::tempdir().unwrap();
    let host = start_host(&host_dir);
    let address = format!("127.0.0.1:{}", host.local_addr().port());
    let client_key = DeviceKey::generate().unwrap();
    let code = host.start_pairing().unwrap();

    runtime().block_on(async {
        let mut paired = pair(&address, &code, &client_key, info("Client"))
            .await
            .unwrap();

        assert_eq!(host.devices().len(), 1);
        assert!(host.pairing().is_none(), "a used code is gone");

        let remote = connect(&mut paired, &client_key, "0.0.0").await.unwrap();

        let mut pty = remote.open_terminal(80, 24).await.unwrap();

        // The first bytes are the attach checkpoint, which starts with a reset.
        let checkpoint = read_until(&mut pty, "\x1bc").await;

        assert!(checkpoint.starts_with("\x1bc"));

        poll_fn(|cx| pty.poll_write(cx, b"echo NMT_REMOTE_%OS%%HOME%OK\r"))
            .await
            .unwrap();

        timeout(Duration::from_secs(20), read_until(&mut pty, "NMT_REMOTE_"))
            .await
            .expect("command output arrives");
    });
}

#[test]
fn unpaired_and_revoked_devices_are_refused() {
    let host_dir = tempfile::tempdir().unwrap();
    let host = start_host(&host_dir);
    let address = format!("127.0.0.1:{}", host.local_addr().port());
    let client_key = DeviceKey::generate().unwrap();
    let stranger = DeviceKey::generate().unwrap();

    runtime().block_on(async {
        let wrong = PairingCode::generate().unwrap();

        host.start_pairing().unwrap();

        assert!(
            pair(&address, &wrong, &client_key, info("Guess"))
                .await
                .is_err()
        );

        let code = host.start_pairing().unwrap();

        let mut paired = pair(&address, &code, &client_key, info("Client"))
            .await
            .unwrap();

        let mut impostor = paired.clone();

        assert!(connect(&mut impostor, &stranger, "0.0.0").await.is_err());
        assert!(connect(&mut paired, &client_key, "0.0.0").await.is_ok());

        host.remove_device(&client_key.id()).unwrap();

        assert!(connect(&mut paired, &client_key, "0.0.0").await.is_err());
    });
}
