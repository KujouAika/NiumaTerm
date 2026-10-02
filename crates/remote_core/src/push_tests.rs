use std::array;

use crate::push::{MAX_BODY_CHARS, PushKind, PushMessage, open, seal, seal_with_nonce};

const HOST: &str = "abcdefghij012345";

/// Bytes 0 to 31.
const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

fn message() -> PushMessage {
    PushMessage::new(
        HOST,
        "a-1",
        PushKind::TurnFinished,
        "Claude finished",
        "Done.",
        1_790_000_000_000,
    )
}

/// Produced by CryptoKit's `ChaChaPoly.seal` over the same key, nonce (bytes
/// 0 to 11), plaintext and associated data, so it pins the form the phone's
/// notification extension opens.
#[test]
fn a_sealed_push_matches_what_cryptokit_seals() {
    let nonce: [u8; 12] = array::from_fn(|index| index as u8);

    assert_eq!(
        seal_with_nonce(KEY, &message(), nonce).unwrap(),
        "AAECAwQFBgcICQoL8tl+IhMmiWLf7EyHuicsAqsT1oI3E8XQjKce90b0kx7Aol+WlW3Y91kQTPoEub4BUqbCHZNMmqEwzKx1/hD4vxcRz6WAcA7F5mIkyHQZ/sv4nStTGg6VHmg+nmerFWfbb/JOkZi9XtaLebfYzPSZosVNN0yrZ3HPdl0E70kPj3aRVvPAGPzgxE3XtLeaa3qsn3bhdg=="
    );
}

#[test]
fn a_sealed_push_opens_only_under_its_own_host() {
    let sealed = seal(KEY, &message()).unwrap();

    assert_eq!(open(KEY, HOST, &sealed).unwrap(), message());
    assert!(open(KEY, "zzzzzzzzzz012345", &sealed).is_err());
}

#[test]
fn two_seals_of_one_message_differ() {
    assert_ne!(
        seal(KEY, &message()).unwrap(),
        seal(KEY, &message()).unwrap()
    );
}

#[test]
fn a_long_body_is_cut_to_the_push_length_limit() {
    let long = "x".repeat(MAX_BODY_CHARS * 3);
    let message = PushMessage::new(HOST, "a-1", PushKind::Question, "Asks", &long, 0);

    assert_eq!(message.body.chars().count(), MAX_BODY_CHARS + 1);
    assert!(message.body.ends_with('…'));
}

#[test]
fn a_key_that_is_not_32_bytes_is_refused() {
    assert!(seal("AAEC", &message()).is_err());
}
