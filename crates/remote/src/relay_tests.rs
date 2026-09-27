//! Pairing and channel setup through a running relay, with the LAN path
//! left out so only the relay can carry them. Run `wrangler dev` under
//! `relay/` and point `NMT_TEST_RELAY_URL` and `NMT_TEST_RELAY_KEY` at it.

use std::env;
use std::sync::Arc;
use std::time::Duration;

use nmt_platform::runtime;
use nmt_remote_core::PROTO_MINOR;
use nmt_remote_core::identity::DeviceKey;
use nmt_remote_core::messages::{ClientHello, DeviceInfo, DeviceKind, RelayAccess};
use tokio::time::{Instant, sleep};

use crate::client::{channel_handshake, pair_over};
use crate::host::{HostConfig, HostService};
use crate::relay::{dial_host, dial_pairing};
use crate::sessions::SessionRegistry;
use crate::store::now_ms;

const WAIT: Duration = Duration::from_secs(20);

fn info(name: &str) -> DeviceInfo {
    DeviceInfo {
        name: name.into(),
        kind: DeviceKind::Desktop,
        platform: "test".into(),
        app_version: "0.0.0".into(),
    }
}

#[test]
#[ignore = "needs a running relay"]
fn a_client_pairs_and_connects_through_the_relay() {
    let relay = RelayAccess {
        url: env::var("NMT_TEST_RELAY_URL").expect("NMT_TEST_RELAY_URL"),
        access_key: env::var("NMT_TEST_RELAY_KEY").expect("NMT_TEST_RELAY_KEY"),
    };

    let host_dir = tempfile::tempdir().unwrap();

    let host = HostService::start(
        host_dir.path().to_path_buf(),
        DeviceKey::generate().unwrap(),
        HostConfig {
            port: 0,
            device: info("Host"),
            shell: None,
            args: Vec::new(),
            registry: SessionRegistry::new(),
            relay: Some(relay.clone()),
            on_change: Arc::new(|| {}),
        },
    )
    .unwrap();

    let key = DeviceKey::generate().unwrap();
    let code = host.start_pairing().unwrap();

    runtime().block_on(async {
        // The host registers and claims the slot in the background.
        let deadline = Instant::now() + WAIT;

        let ws = loop {
            match dial_pairing(&relay, code.slot()).await {
                Ok(ws) => break ws,
                Err(error) if Instant::now() < deadline => {
                    eprintln!("waiting for the slot: {error:#}");

                    sleep(Duration::from_millis(250)).await;
                }
                Err(error) => panic!("the slot never opened: {error:#}"),
            }
        };

        let (host_key, accepted) = pair_over(ws, &code, &key, info("Client"), None)
            .await
            .unwrap();

        assert_eq!(accepted.relay.as_ref(), Some(&relay));
        assert_eq!(host.devices().len(), 1);

        let hello = ClientHello {
            proto_minor: PROTO_MINOR,
            app_version: "0.0.0".into(),
            features: vec!["terminal".into()],
            hello_ms: now_ms(),
        };

        let ws = dial_host(&relay, &host.device_id()).await.unwrap();

        let (_, _, host_hello) = channel_handshake(ws, &key, &host_key, &hello)
            .await
            .unwrap();

        assert_eq!(host_hello.name, "Host");
    });
}
