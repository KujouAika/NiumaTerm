//! The sans-IO half of the direct path: STUN binding messages (RFC 5389),
//! NAT classification from their answers, and the rule both sides use to
//! pick which of them dials.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};

/// Fixed value in every STUN header since RFC 5389; it also masks
/// XOR-MAPPED-ADDRESS so middleboxes that rewrite addresses in payloads leave
/// it alone.
const MAGIC_COOKIE: u32 = 0x2112_A442;

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;
const HEADER_LEN: usize = 20;

/// How a NAT maps one local socket to public addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatKind {
    /// One public port for every destination, so the address a STUN server
    /// reports is the one a peer reaches.
    Cone,
    /// A new public port per destination, so a peer cannot learn the port
    /// this side will use toward it.
    Symmetric,
}

/// The parameters of `direct.offer` and of its reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectOffer {
    pub nat: NatKind,

    /// Public `ip:port` mappings STUN servers reported for the socket the
    /// sender will use.
    pub addrs: Vec<String>,

    /// The sender's QUIC certificate, DER in base64. The dialer trusts only
    /// the waiting side's certificate, which arrived over the authenticated
    /// channel, so no third party can stand in for the waiting side.
    pub cert: String,
}

/// A side of a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Client,
    Host,
}

/// A STUN binding request with this transaction id.
pub fn binding_request(transaction: &[u8; 12]) -> [u8; HEADER_LEN] {
    let mut request = [0u8; HEADER_LEN];

    request[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    request[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    request[8..20].copy_from_slice(transaction);

    request
}

/// The mapped address in a binding success response to `transaction`, or
/// `None` for anything else: another transaction, an error response, or a
/// datagram that is not STUN.
pub fn mapped_address(response: &[u8], transaction: &[u8; 12]) -> Option<SocketAddr> {
    if response.len() < HEADER_LEN
        || u16::from_be_bytes([response[0], response[1]]) != BINDING_SUCCESS
        || response[4..8] != MAGIC_COOKIE.to_be_bytes()
        || response[8..20] != transaction[..]
    {
        return None;
    }

    let body_len = usize::from(u16::from_be_bytes([response[2], response[3]]));
    let body = response.get(HEADER_LEN..HEADER_LEN + body_len)?;

    let mut plain = None;
    let mut pos = 0;

    while pos + 4 <= body.len() {
        let kind = u16::from_be_bytes([body[pos], body[pos + 1]]);
        let len = usize::from(u16::from_be_bytes([body[pos + 2], body[pos + 3]]));
        let value = body.get(pos + 4..pos + 4 + len)?;

        match kind {
            // Servers send both when they can; the XOR form survives NATs
            // that rewrite addresses they find in payloads.
            XOR_MAPPED_ADDRESS => return decode_address(value, Some(&response[4..20])),
            MAPPED_ADDRESS => plain = decode_address(value, None),
            _ => {}
        }

        // Attribute values are padded to a multiple of four bytes.
        pos += 4 + len.next_multiple_of(4);
    }

    plain
}

/// Decode a (XOR-)MAPPED-ADDRESS value. `mask` is the cookie and
/// transaction id for the XOR form.
fn decode_address(value: &[u8], mask: Option<&[u8]>) -> Option<SocketAddr> {
    let family = *value.get(1)?;

    let mut port = u16::from_be_bytes([*value.get(2)?, *value.get(3)?]);

    let len = match family {
        1 => 4,
        2 => 16,
        _ => return None,
    };

    let mut ip = value.get(4..4 + len)?.to_vec();

    if let Some(mask) = mask {
        port ^= u16::from_be_bytes([mask[0], mask[1]]);

        for (byte, mask) in ip.iter_mut().zip(mask) {
            *byte ^= mask;
        }
    }

    let ip = match <[u8; 16]>::try_from(ip.as_slice()) {
        Ok(v6) => IpAddr::V6(Ipv6Addr::from(v6)),
        Err(_) => IpAddr::V4(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])),
    };

    Some(SocketAddr::new(ip, port))
}

/// Classify a NAT from the addresses several STUN servers reported for one
/// socket. One port everywhere means a cone mapping, even across several
/// public addresses, as a network with one egress per region shows. Fewer
/// than two answers cannot tell the kinds apart.
pub fn classify(mapped: &[SocketAddr]) -> Option<NatKind> {
    let (first, rest) = mapped.split_first()?;

    if rest.is_empty() {
        return None;
    }

    if rest.iter().all(|addr| addr.port() == first.port()) {
        Some(NatKind::Cone)
    } else {
        Some(NatKind::Symmetric)
    }
}

/// Which side dials the direct connection, the same answer on both sides.
/// The symmetric side dials, because the port it will use toward the peer
/// exists only once it sends; the waiting side's cone mapping is the
/// address the dialer reaches. Two symmetric sides cannot meet without a
/// UDP relay.
pub fn dialer(client: NatKind, host: NatKind) -> Option<Side> {
    match (client, host) {
        (NatKind::Symmetric, NatKind::Symmetric) => None,
        (NatKind::Symmetric | NatKind::Cone, NatKind::Cone) => Some(Side::Client),
        (NatKind::Cone, NatKind::Symmetric) => Some(Side::Host),
    }
}
