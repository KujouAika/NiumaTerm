use crate::channel::{Channel, ClientHandshake, HostHandshake};
use crate::identity::DeviceKey;
use crate::messages::{ClientHello, HostHello};
use crate::preface::{Preface, PrefaceKind};

fn client_hello() -> ClientHello {
    ClientHello {
        proto_minor: 0,
        app_version: "1.5.7".into(),
        features: vec!["terminal".into()],
        hello_ms: 1_790_000_000_000,
    }
}

fn host_hello() -> HostHello {
    HostHello {
        proto_minor: 0,
        app_version: "1.5.7".into(),
        features: vec!["terminal".into(), "agent".into()],
        name: "Studio PC".into(),
        lan_hints: vec!["192.168.1.20:47470".into()],
    }
}

fn prefaces() -> (Preface, Preface) {
    let offer = Preface::offer(PrefaceKind::Channel);

    (offer, offer.answer())
}

fn connect(client: &DeviceKey, host: &DeviceKey) -> (Channel, Channel) {
    let (offer, answer) = prefaces();

    let (handshake, msg1) =
        ClientHandshake::start(client, host.public(), &offer, &answer, &client_hello()).unwrap();

    let pending = HostHandshake::read(host, &offer, &answer, &msg1).unwrap();

    assert_eq!(pending.client_key(), client.public());
    assert_eq!(pending.hello(), &client_hello());

    let (host_channel, msg2) = pending.accept(&host_hello()).unwrap();
    let (client_channel, hello) = handshake.finish(&msg2).unwrap();

    assert_eq!(hello, host_hello());

    (client_channel, host_channel)
}

#[test]
fn channel_delivers_messages_both_ways() {
    let client = DeviceKey::generate().unwrap();
    let host = DeviceKey::generate().unwrap();

    let (mut client, mut host) = connect(&client, &host);

    let sealed = client.seal(b"ping").unwrap();

    assert!(!sealed.windows(4).any(|w| w == b"ping"));
    assert_eq!(host.open(&sealed).unwrap(), b"ping");
    assert_eq!(client.open(&host.seal(b"pong").unwrap()).unwrap(), b"pong");
}

#[test]
fn tampered_replayed_and_reordered_messages_fail() {
    let client = DeviceKey::generate().unwrap();
    let host = DeviceKey::generate().unwrap();

    let (mut sender, mut receiver) = connect(&client, &host);
    let mut tampered = sender.seal(b"input").unwrap();

    tampered[0] ^= 1;

    assert!(receiver.open(&tampered).is_err());

    let (mut sender, mut receiver) = connect(&client, &host);

    let first = sender.seal(b"one").unwrap();

    assert!(receiver.open(&first).is_ok());
    assert!(receiver.open(&first).is_err(), "replay");

    let (mut sender, mut receiver) = connect(&client, &host);

    let _first = sender.seal(b"one").unwrap();
    let second = sender.seal(b"two").unwrap();

    assert!(receiver.open(&second).is_err(), "reorder");
}

#[test]
fn handshake_fails_for_the_wrong_host_or_rewritten_preface() {
    let client = DeviceKey::generate().unwrap();
    let host = DeviceKey::generate().unwrap();
    let impostor = DeviceKey::generate().unwrap();
    let (offer, answer) = prefaces();

    // A client addressing `host` cannot be answered by anyone else.
    let (_, msg1) =
        ClientHandshake::start(&client, host.public(), &offer, &answer, &client_hello()).unwrap();

    assert!(HostHandshake::read(&impostor, &offer, &answer, &msg1).is_err());

    // A preface rewritten in transit changes one side's prologue.
    let downgraded = Preface {
        min_major: 0,
        ..offer
    };

    assert!(HostHandshake::read(&host, &downgraded, &answer, &msg1).is_err());
}
