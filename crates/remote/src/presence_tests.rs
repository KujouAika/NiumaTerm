use std::time::{Duration, Instant};

use crate::presence::{DISCONNECT_EXPIRY, DISCONNECT_GRACE, Presence, Presences};

const PHONE: [u8; 32] = [1; 32];

#[test]
fn a_device_that_never_connected_is_paired() {
    let presences = Presences::default();

    assert_eq!(presences.presence(&PHONE, Instant::now()), Presence::Paired);
}

#[test]
fn a_closed_channel_stays_connected_through_the_grace_then_disconnects_then_expires() {
    let start = Instant::now();

    let mut presences = Presences::default();

    presences.opened(PHONE);

    assert_eq!(presences.presence(&PHONE, start), Presence::Connected);

    presences.closed(PHONE, start);

    let second = Duration::from_secs(1);

    assert_eq!(
        presences.presence(&PHONE, start + DISCONNECT_GRACE - second),
        Presence::Connected
    );
    assert_eq!(
        presences.presence(&PHONE, start + DISCONNECT_GRACE),
        Presence::Disconnected
    );
    assert_eq!(
        presences.presence(&PHONE, start + DISCONNECT_EXPIRY - second),
        Presence::Disconnected
    );
    assert_eq!(
        presences.presence(&PHONE, start + DISCONNECT_EXPIRY),
        Presence::Paired
    );
}

#[test]
fn a_device_stays_connected_while_any_of_its_channels_is_open() {
    let start = Instant::now();

    let mut presences = Presences::default();

    presences.opened(PHONE);
    presences.opened(PHONE);
    presences.closed(PHONE, start);

    assert_eq!(
        presences.presence(&PHONE, start + DISCONNECT_EXPIRY),
        Presence::Connected
    );
}

#[test]
fn being_at_the_desk_pairs_away_devices_and_keeps_connected_ones() {
    let start = Instant::now();
    let away = start + DISCONNECT_GRACE;

    let mut presences = Presences::default();

    let tablet = [2; 32];

    presences.opened(PHONE);
    presences.closed(PHONE, start);
    presences.opened(tablet);

    presences.at_desk();

    assert_eq!(presences.presence(&PHONE, away), Presence::Paired);
    assert_eq!(presences.presence(&tablet, away), Presence::Connected);

    // Leaving again after that makes the phone disconnected again.
    presences.opened(PHONE);
    presences.closed(PHONE, away);

    assert_eq!(
        presences.presence(&PHONE, away + DISCONNECT_GRACE),
        Presence::Disconnected
    );
}
