use crate::Error;
use crate::identity::DeviceKey;
use crate::messages::{DeviceInfo, DeviceKind, PairAccepted, RelayAccess};
use crate::pairing::{
    CODE_LIFETIME_MS, ClientPairing, ClientPairingConfirm, HostPairing, IssuedCode, PairedHost,
    PairingCode, PairingLink, PairingRequest,
};
use crate::preface::{Preface, PrefaceKind};

fn info(name: &str) -> DeviceInfo {
    DeviceInfo {
        name: name.into(),
        kind: DeviceKind::Desktop,
        platform: "windows".into(),
        app_version: "1.5.7".into(),
    }
}

fn accepted() -> PairAccepted {
    PairAccepted {
        host: info("Studio PC"),
        relay: Some(RelayAccess {
            url: "wss://relay.example.com".into(),
            access_key: "relay-key".into(),
        }),
        lan_hints: vec!["192.168.1.20:47470".into()],
    }
}

/// Run the exchange up to the host's decision on msg3.
fn pair(
    client_code: &PairingCode,
    host_code: &PairingCode,
    client: &DeviceKey,
    host: &DeviceKey,
    expected_host_key: Option<[u8; 32]>,
) -> Result<(PairingRequest, ClientPairingConfirm), Error> {
    let offer = Preface::offer(PrefaceKind::Pairing);
    let answer = offer.answer();

    let (pairing, hello) = ClientPairing::start(
        client_code,
        &offer,
        &answer,
        info("Work laptop"),
        expected_host_key,
    )?;

    let (mut host_side, reply) = HostPairing::on_hello(host_code, host, &offer, &answer, &hello)?;

    let (handshake, msg1) = pairing.on_reply(client, &reply)?;
    let msg2 = host_side.on_msg1(&msg1)?;
    let (confirm, msg3) = handshake.on_msg2(&msg2)?;

    Ok((host_side.on_msg3(&msg3)?, confirm))
}

#[test]
fn the_right_code_pairs_both_devices() {
    let code = PairingCode::generate().unwrap();
    let client = DeviceKey::generate().unwrap();
    let host = DeviceKey::generate().unwrap();

    let (request, confirm) = pair(&code, &code, &client, &host, None).unwrap();

    assert_eq!(&request.client_key, client.public());
    assert_eq!(request.client, info("Work laptop"));

    let message = request.accept(&accepted()).unwrap();

    let PairedHost {
        public_key,
        accepted: received,
    } = confirm.on_accepted(&message).unwrap();

    assert_eq!(&public_key, host.public());
    assert_eq!(received, accepted());
}

#[test]
fn a_wrong_code_fails_on_the_host_as_a_counted_attempt() {
    let host_code = PairingCode::parse("K7Q2-M9XD").unwrap();
    let guess = PairingCode::parse("K7Q2-M9XE").unwrap();
    let client = DeviceKey::generate().unwrap();
    let host = DeviceKey::generate().unwrap();

    let result = pair(&guess, &host_code, &client, &host, None);

    assert!(matches!(result, Err(Error::WrongCode)));
}

#[test]
fn a_pinned_host_key_rejects_any_other_host() {
    let code = PairingCode::generate().unwrap();
    let client = DeviceKey::generate().unwrap();
    let host = DeviceKey::generate().unwrap();
    let other = DeviceKey::generate().unwrap();

    let result = pair(&code, &code, &client, &host, Some(*other.public()));

    assert!(matches!(result, Err(Error::HostKeyMismatch)));
    assert!(pair(&code, &code, &client, &host, Some(*host.public())).is_ok());
}

#[test]
fn issued_codes_expire_and_allow_three_failures_and_one_use() {
    let now = 1_790_000_000_000;

    let mut issued = IssuedCode::new(PairingCode::generate().unwrap(), now);

    assert!(issued.is_usable(now));
    assert!(!issued.is_usable(now + CODE_LIFETIME_MS));
    assert!(issued.record_failure(now));
    assert!(issued.record_failure(now));
    assert!(!issued.record_failure(now), "third failure invalidates");

    let mut issued = IssuedCode::new(PairingCode::generate().unwrap(), now);

    issued.record_success();

    assert!(!issued.is_usable(now), "single use");
}

#[test]
fn typed_codes_normalize_case_separators_and_look_alikes() {
    let code = PairingCode::parse("k7q2 m9xd").unwrap();

    assert_eq!(code.as_str(), "K7Q2M9XD");
    assert_eq!(code.slot(), "K7Q");
    assert_eq!(code.to_string(), "K7Q2-M9XD");
    assert_eq!(
        PairingCode::parse("oIl0-ABCD").unwrap().as_str(),
        "0110ABCD"
    );
    assert!(PairingCode::parse("K7Q2-M9X").is_err());
    assert!(PairingCode::parse("K7Q2-M9XU").is_err());
    assert!(
        !format!("{code:?}").contains("M9XD"),
        "debug hides the secret"
    );

    let generated = PairingCode::generate().unwrap();

    assert_eq!(
        PairingCode::parse(&generated.to_string()).unwrap(),
        generated
    );
}

#[test]
fn pairing_links_round_trip_and_bind_the_host_id_to_its_key() {
    let host = DeviceKey::generate().unwrap();

    let link = PairingLink {
        code: PairingCode::generate().unwrap(),
        host_id: host.id(),
        host_key: *host.public(),
        relay: Some(RelayAccess {
            url: "wss://relay.example.com/base?x=1".into(),
            access_key: "k&y=".into(),
        }),
        addresses: vec!["192.168.1.20:47470".into(), "[fe80::1]:47470".into()],
    };

    assert_eq!(PairingLink::parse(&link.to_url()).unwrap(), link);

    let other = DeviceKey::generate().unwrap();

    let forged = PairingLink {
        host_id: other.id(),
        ..link
    };

    assert!(PairingLink::parse(&forged.to_url()).is_err());
}
