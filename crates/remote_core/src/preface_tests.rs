use crate::preface::{Preface, PrefaceKind};
use crate::{PROTO_MAJOR, PROTO_MIN_MAJOR};

#[test]
fn preface_negotiates_the_highest_shared_major() {
    let newer_client = Preface {
        kind: PrefaceKind::Channel,
        major: PROTO_MAJOR + 3,
        min_major: PROTO_MIN_MAJOR,
    };

    let answer = newer_client.answer();

    assert_eq!(answer.kind, PrefaceKind::Channel);
    assert_eq!((answer.major, answer.min_major), (PROTO_MAJOR, PROTO_MAJOR));
    assert_eq!(Preface::decode(&answer.encode()).unwrap(), answer);
}

#[test]
fn preface_without_a_shared_major_is_rejected_with_the_host_range() {
    let too_new = Preface {
        kind: PrefaceKind::Channel,
        major: PROTO_MAJOR + 2,
        min_major: PROTO_MAJOR + 1,
    };

    let answer = too_new.answer();

    assert_eq!(answer.kind, PrefaceKind::Rejected);
    assert_eq!(
        (answer.major, answer.min_major),
        (PROTO_MAJOR, PROTO_MIN_MAJOR)
    );
    assert!(Preface::decode(b"NMTX\x01\x01\x01").is_err());
    assert!(Preface::decode(b"NMTR\x07\x01\x01").is_err());
}
