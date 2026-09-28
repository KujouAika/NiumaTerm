//! The addresses other machines on the LAN can reach this one at, for the
//! host to show and to put in pairing links. The host listens on every
//! interface, so this only decides which addresses to tell people about.
//!
//! The source address of the default route is not enough on its own: a VPN
//! or a TUN-mode proxy takes over the default route, and its tunnel address
//! is useless to a phone on the same Wi-Fi. Interfaces are enumerated
//! instead, tunnels dropped, and private LAN ranges put first.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

use if_addrs::{IfAddr, get_if_addrs};
use tracing::warn;

/// One IPv4 address assigned to an interface that is up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    pub(crate) ip: Ipv4Addr,

    /// Point-to-point links are VPN and TUN tunnels (`utun*` on macOS),
    /// whose addresses nobody else on the LAN can route to.
    pub(crate) point_to_point: bool,
}

/// This machine's LAN addresses, most likely reachable first. Empty when
/// no interface has a usable IPv4 address.
pub fn lan_addresses() -> Vec<Ipv4Addr> {
    let interfaces = get_if_addrs().unwrap_or_else(|error| {
        warn!(%error, "listing network interfaces failed");

        Vec::new()
    });

    let candidates: Vec<Candidate> = interfaces
        .iter()
        .filter(|interface| interface.is_oper_up())
        .filter_map(|interface| match &interface.addr {
            IfAddr::V4(addr) => Some(Candidate {
                ip: addr.ip,
                point_to_point: interface.is_p2p(),
            }),
            IfAddr::V6(_) => None,
        })
        .collect();

    rank(&candidates, default_route_source())
}

/// Drop addresses a LAN peer cannot use, then order the rest: private
/// ranges before anything else, and within a tier the default route's
/// source first, so a machine without a tunnel keeps the address it always
/// showed.
pub(crate) fn rank(candidates: &[Candidate], route_source: Option<Ipv4Addr>) -> Vec<Ipv4Addr> {
    let mut usable: Vec<Ipv4Addr> = Vec::new();

    for candidate in candidates {
        if !candidate.point_to_point && reachable(candidate.ip) && !usable.contains(&candidate.ip) {
            usable.push(candidate.ip);
        }
    }

    // A stable sort keeps interface order among equals.
    usable.sort_by_key(|ip| (!ip.is_private(), Some(*ip) != route_source));

    usable
}

fn reachable(ip: Ipv4Addr) -> bool {
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || benchmarking(ip))
}

/// 198.18.0.0/15 is reserved for benchmarking (RFC 2544). TUN-mode proxies
/// such as Clash and Surge take their interface and fake-IP addresses from
/// it, and on Windows their adapter is not point-to-point, so the range
/// itself is the only reliable sign.
fn benchmarking(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();

    a == 198 && (b & 0xfe) == 18
}

/// The source address of the default route. Connecting a UDP socket sends
/// nothing; it only selects the route.
fn default_route_source() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;

    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;

    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(_) => None,
    }
}
