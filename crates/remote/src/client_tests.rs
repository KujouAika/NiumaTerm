use crate::client::merge_lan_hints;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn the_hosts_list_replaces_what_was_known_behind_the_address_that_worked() {
    let hints = merge_lan_hints(
        &strings(&["192.168.1.20:47470", "10.0.0.5:47470"]),
        Some("10.0.0.5:47470"),
        &strings(&["192.168.1.9:47470"]),
    );

    // The stale 192.168.1.9 is gone; the working address leads and is not
    // listed twice.
    assert_eq!(hints, strings(&["10.0.0.5:47470", "192.168.1.20:47470"]));
}

#[test]
fn a_host_that_sends_no_list_keeps_the_known_addresses() {
    let hints = merge_lan_hints(
        &[],
        Some("192.168.1.20:47470"),
        &strings(&["192.168.1.9:47470", "192.168.1.20:47470"]),
    );

    assert_eq!(hints, strings(&["192.168.1.20:47470", "192.168.1.9:47470"]));
}

#[test]
fn malformed_addresses_are_dropped_and_the_list_is_bounded() {
    let hints = merge_lan_hints(
        &strings(&[
            "not an address",
            "10.0.0.1:1",
            "10.0.0.2:1",
            "10.0.0.3:1",
            "10.0.0.4:1",
            "10.0.0.5:1",
        ]),
        None,
        &[],
    );

    assert_eq!(
        hints,
        strings(&["10.0.0.1:1", "10.0.0.2:1", "10.0.0.3:1", "10.0.0.4:1"])
    );
}
