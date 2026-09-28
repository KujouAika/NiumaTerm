use std::net::Ipv4Addr;

use crate::lan::{Candidate, rank};

fn lan(a: u8, b: u8, c: u8, d: u8) -> Candidate {
    Candidate {
        ip: Ipv4Addr::new(a, b, c, d),
        point_to_point: false,
    }
}

fn tunnel(a: u8, b: u8, c: u8, d: u8) -> Candidate {
    Candidate {
        ip: Ipv4Addr::new(a, b, c, d),
        point_to_point: true,
    }
}

#[test]
fn a_tun_proxy_holding_the_default_route_does_not_hide_the_wifi_address() {
    // macOS with Clash in TUN mode: the default route leaves through utun.
    let candidates = [
        lan(127, 0, 0, 1),
        lan(192, 168, 3, 41),
        tunnel(198, 18, 0, 1),
    ];

    let ranked = rank(&candidates, Some(Ipv4Addr::new(198, 18, 0, 1)));

    assert_eq!(ranked, [Ipv4Addr::new(192, 168, 3, 41)]);
}

#[test]
fn a_proxy_adapter_that_is_not_point_to_point_is_dropped_by_its_range() {
    // Windows Wintun adapters report an ordinary interface type.
    let candidates = [lan(198, 19, 0, 1), lan(10, 0, 0, 7)];

    let ranked = rank(&candidates, Some(Ipv4Addr::new(198, 19, 0, 1)));

    assert_eq!(ranked, [Ipv4Addr::new(10, 0, 0, 7)]);
}

#[test]
fn the_default_route_source_leads_among_private_addresses() {
    // Ethernet and Wi-Fi both up; the route picks Wi-Fi.
    let candidates = [lan(10, 1, 0, 5), lan(192, 168, 1, 20), lan(169, 254, 3, 3)];

    let ranked = rank(&candidates, Some(Ipv4Addr::new(192, 168, 1, 20)));

    assert_eq!(
        ranked,
        [Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(10, 1, 0, 5)]
    );
}

#[test]
fn a_public_address_is_kept_after_private_ones() {
    let candidates = [lan(203, 0, 113, 9), lan(172, 16, 4, 2)];

    let ranked = rank(&candidates, Some(Ipv4Addr::new(203, 0, 113, 9)));

    assert_eq!(
        ranked,
        [Ipv4Addr::new(172, 16, 4, 2), Ipv4Addr::new(203, 0, 113, 9)]
    );
}

#[test]
fn nothing_usable_leaves_the_list_empty() {
    let candidates = [lan(127, 0, 0, 1), tunnel(100, 100, 1, 1)];

    assert!(rank(&candidates, None).is_empty());
}
