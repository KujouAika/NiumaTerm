use std::time::Duration;

use nmt_remote_core::channel::{Channel, ClientHandshake, HostHandshake};
use nmt_remote_core::identity::DeviceKey;
use nmt_remote_core::messages::{ClientHello, HostHello};
use nmt_remote_core::preface::{Preface, PrefaceKind};
use tokio::io::{DuplexStream, duplex};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;

use crate::link::pump;

fn channels() -> (Channel, Channel) {
    let client = DeviceKey::generate().unwrap();
    let host = DeviceKey::generate().unwrap();
    let offer = Preface::offer(PrefaceKind::Channel);
    let answer = offer.answer();

    let hello = ClientHello {
        proto_minor: 0,
        app_version: String::new(),
        features: Vec::new(),
        hello_ms: 1,
    };

    let (handshake, msg1) =
        ClientHandshake::start(&client, host.public(), &offer, &answer, &hello).unwrap();

    let (host_channel, msg2) = HostHandshake::read(&host, &offer, &answer, &msg1)
        .unwrap()
        .accept(&HostHello {
            proto_minor: 0,
            app_version: String::new(),
            features: Vec::new(),
            name: String::new(),
            lan_hints: Vec::new(),
        })
        .unwrap();

    let (client_channel, _) = handshake.finish(&msg2).unwrap();

    (client_channel, host_channel)
}

async fn sockets() -> (WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
    let (a, b) = duplex(1 << 20);

    (
        WebSocketStream::from_raw_socket(a, Role::Client, None).await,
        WebSocketStream::from_raw_socket(b, Role::Server, None).await,
    )
}

#[tokio::test(start_paused = true)]
async fn quiet_but_live_channels_stay_open() {
    let (client_channel, host_channel) = channels();
    let (client_ws, host_ws) = sockets().await;
    let (_client_out, client_out_rx) = mpsc::unbounded_channel();
    let (_host_out, host_out_rx) = mpsc::unbounded_channel();
    let (client_in, _client_in_rx) = mpsc::unbounded_channel();
    let (host_in, _host_in_rx) = mpsc::unbounded_channel();

    let client = tokio::spawn(pump(client_ws, client_channel, client_out_rx, client_in));
    let host = tokio::spawn(pump(host_ws, host_channel, host_out_rx, host_in));

    sleep(Duration::from_secs(300)).await;

    assert!(!client.is_finished(), "pings keep an idle channel open");
    assert!(!host.is_finished());
}

#[tokio::test(start_paused = true)]
async fn a_silent_peer_is_declared_dead() {
    let (client_channel, _) = channels();

    // The peer socket exists but nothing ever reads or answers on it.
    let (client_ws, _silent) = sockets().await;
    let (_out, out_rx) = mpsc::unbounded_channel();
    let (inbound, _inbound_rx) = mpsc::unbounded_channel();

    let result = timeout(
        Duration::from_secs(120),
        pump(client_ws, client_channel, out_rx, inbound),
    )
    .await
    .expect("the pump gives up on its own");

    assert!(result.is_err());
}
