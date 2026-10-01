//! The addresses other machines on the LAN can reach this one at, for the
//! host to show and to put in pairing links. The host listens on every
//! interface, so this only decides which addresses to tell people about.
//!
//! The source address of the default route is not enough on its own: a VPN
//! or a TUN-mode proxy takes over the default route, and its tunnel address
//! is useless to a phone on the same Wi-Fi. Interfaces are enumerated
//! instead, tunnels dropped, and private LAN ranges put first.
//!
//! Virtual adapters (VirtualBox and VMware host-only networks, Hyper-V and
//! WSL switches, VPN adapters) carry private addresses too, which only the
//! machine itself and its guests can reach. Where an adapter with a physical
//! connector has an address, only physical adapters are offered.

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

    /// The adapter has a physical connector: Ethernet or Wi-Fi hardware
    /// rather than a software adapter. True where the platform cannot tell.
    pub(crate) physical: bool,
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
                physical: physical(interface.index),
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
    let usable: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| !candidate.point_to_point && reachable(candidate.ip))
        .collect();

    // A Hyper-V external switch moves the machine's LAN address onto a
    // virtual adapter, so virtual adapters stay when no physical one has an
    // address, rather than leaving nothing to offer.
    let any_physical = usable.iter().any(|candidate| candidate.physical);

    let mut ranked: Vec<Ipv4Addr> = Vec::new();

    for candidate in usable {
        if (candidate.physical || !any_physical) && !ranked.contains(&candidate.ip) {
            ranked.push(candidate.ip);
        }
    }

    // A stable sort keeps interface order among equals.
    ranked.sort_by_key(|ip| (!ip.is_private(), Some(*ip) != route_source));

    ranked
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

/// Whether the adapter behind interface `index` has a physical connector.
/// Windows sets `ConnectorPresent` only for hardware adapters; VirtualBox,
/// VMware, Hyper-V, WSL and VPN adapters report none, though most of them
/// claim to be Ethernet. An adapter that cannot be read counts as physical,
/// so a failed query never hides an address.
#[cfg(windows)]
fn physical(index: Option<u32>) -> bool {
    use std::mem;

    use windows_sys::Win32::Foundation::NO_ERROR;
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetIfEntry2, MIB_IF_ROW2};

    /// `ConnectorPresent` in `InterfaceAndOperStatusFlags`, after the
    /// `HardwareInterface` and `FilterInterface` bits.
    const CONNECTOR_PRESENT: u8 = 1 << 2;

    let Some(index) = index else {
        return true;
    };

    // SAFETY: MIB_IF_ROW2 is plain data, for which all zeros is valid.
    let mut row: MIB_IF_ROW2 = unsafe { mem::zeroed() };

    row.InterfaceIndex = index;

    // SAFETY: `row` is a valid MIB_IF_ROW2 whose InterfaceIndex selects the
    // interface; the call fills in the rest.
    if unsafe { GetIfEntry2(&mut row) } != NO_ERROR {
        return true;
    }

    row.InterfaceAndOperStatusFlags._bitfield & CONNECTOR_PRESENT != 0
}

#[cfg(not(windows))]
fn physical(_index: Option<u32>) -> bool {
    true
}
