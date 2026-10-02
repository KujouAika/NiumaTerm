//! Whether each paired device is using this host from afar.
//!
//! A device that opened a channel is connected. Once its last channel closes
//! it is disconnected: the person has most likely put the phone away while
//! the session runs on, which is when a push notification is worth sending.
//! A device that never connected, or whose person came back to this
//! computer, is merely paired, and hears nothing.
//!
//! Only the host process knows this, so it lives in memory and every device
//! starts paired after a restart.

#[cfg(test)]
#[path = "presence_tests.rs"]
mod presence_tests;

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A closed channel counts as connected for this long. Switching to another
/// app for a moment closes the channel on iOS within seconds, and a person
/// who is back before this does not want the pushes that happened meanwhile.
pub(crate) const DISCONNECT_GRACE: Duration = Duration::from_secs(5 * 60);

/// A device disconnected for this long is taken to be out of use again:
/// whoever left the session running is not following it any more.
pub(crate) const DISCONNECT_EXPIRY: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Paired, and not in use from afar.
    Paired,
    /// A channel is open, or closed less than [`DISCONNECT_GRACE`] ago.
    Connected,
    /// The device was connected and its channels are gone.
    Disconnected,
}

#[derive(Default)]
pub(crate) struct Presences {
    devices: HashMap<[u8; 32], Tracked>,
}

#[derive(Default)]
struct Tracked {
    channels: usize,

    /// When the last channel closed.
    left_at: Option<Instant>,

    /// Connected since the person was last at this computer.
    in_use: bool,
}

impl Presences {
    pub(crate) fn opened(&mut self, key: [u8; 32]) {
        let tracked = self.devices.entry(key).or_default();

        tracked.channels += 1;
        tracked.left_at = None;
        tracked.in_use = true;
    }

    pub(crate) fn closed(&mut self, key: [u8; 32], now: Instant) {
        let Some(tracked) = self.devices.get_mut(&key) else {
            return;
        };

        tracked.channels = tracked.channels.saturating_sub(1);

        if tracked.channels == 0 {
            tracked.left_at = Some(now);
        }
    }

    /// The person is at this computer: devices that are not connected right
    /// now go back to paired. Open channels stay connected, since their
    /// device may still be in someone's hand.
    pub(crate) fn at_desk(&mut self) {
        for tracked in self.devices.values_mut() {
            if tracked.channels == 0 {
                tracked.in_use = false;
            }
        }
    }

    pub(crate) fn forget(&mut self, key: &[u8; 32]) {
        self.devices.remove(key);
    }

    pub(crate) fn presence(&self, key: &[u8; 32], now: Instant) -> Presence {
        let Some(tracked) = self.devices.get(key) else {
            return Presence::Paired;
        };

        if tracked.channels > 0 {
            return Presence::Connected;
        }

        let away = match (tracked.in_use, tracked.left_at) {
            (true, Some(left_at)) => now.saturating_duration_since(left_at),
            _ => return Presence::Paired,
        };

        if away < DISCONNECT_GRACE {
            Presence::Connected
        } else if away < DISCONNECT_EXPIRY {
            Presence::Disconnected
        } else {
            Presence::Paired
        }
    }
}
