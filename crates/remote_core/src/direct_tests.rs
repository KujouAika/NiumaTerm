use std::net::SocketAddr;

use crate::direct::{NatKind, binding_request, classify, mapped_address};

const TRANSACTION: [u8; 12] = *b"nmt-test-tid";

/// A binding success response holding `attributes`, as a server sends it.
fn response(attributes: &[u8]) -> Vec<u8> {
    let mut response = binding_request(&TRANSACTION).to_vec();

    response[0..2].copy_from_slice(&0x0101u16.to_be_bytes());
    response[2..4].copy_from_slice(&u16::try_from(attributes.len()).unwrap().to_be_bytes());
    response.extend_from_slice(attributes);

    response
}

#[test]
fn xor_mapped_address_decodes_to_the_public_mapping() {
    let addr: SocketAddr = "36.27.34.192:63642".parse().unwrap();

    // 63642 ^ 0x2112 and 36.27.34.192 ^ 0x2112A442.
    let mut attribute = vec![0x00, 0x20, 0x00, 0x08, 0x00, 0x01];

    attribute.extend_from_slice(&(63642u16 ^ 0x2112).to_be_bytes());
    attribute.extend_from_slice(&[36 ^ 0x21, 27 ^ 0x12, 34 ^ 0xA4, 192 ^ 0x42]);

    // A plain MAPPED-ADDRESS with another value first: the XOR form wins.
    let mut attributes = vec![0x00, 0x01, 0x00, 0x08, 0x00, 0x01, 0x00, 0x01, 10, 0, 0, 1];

    attributes.extend_from_slice(&attribute);

    assert_eq!(
        mapped_address(&response(&attributes), &TRANSACTION),
        Some(addr)
    );
}

#[test]
fn answers_to_other_transactions_are_ignored() {
    let attributes = [0x00, 0x01, 0x00, 0x08, 0x00, 0x01, 0x00, 0x01, 10, 0, 0, 1];

    assert!(mapped_address(&response(&attributes), b"another-tid!").is_none());
    assert!(mapped_address(&[0; 4], &TRANSACTION).is_none());
}

#[test]
fn nat_kind_follows_the_ports_of_the_mappings() {
    let addrs = |list: &[&str]| -> Vec<SocketAddr> {
        list.iter().map(|addr| addr.parse().unwrap()).collect()
    };

    // A home router that keeps one port for every destination.
    assert_eq!(
        classify(&addrs(&["36.27.34.192:63642", "36.27.34.192:63642"])),
        Some(NatKind::Cone)
    );

    // An office network with a new port per destination and one egress
    // address per region.
    assert_eq!(
        classify(&addrs(&["103.126.92.84:13272", "115.236.119.139:6789"])),
        Some(NatKind::Symmetric)
    );

    assert_eq!(classify(&addrs(&["36.27.34.192:63642"])), None);
}
