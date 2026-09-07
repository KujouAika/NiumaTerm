//! The macOS updater.
//!
//! Sparkle replaces the whole application bundle from a signed archive and
//! relaunches, so none of the Restart Manager machinery the Windows updater is
//! built on has a counterpart here. What is shared is the setting: `[update]
//! check-updates` decides whether background checks run on either platform.
//!
//! The check interval and whether Sparkle may ask about automatic checks on
//! first launch are stamped into the packaged bundle, so they are not repeated
//! here; only the setting a user can change while the application runs is
//! mirrored.

use gpui::{App, Global};
use nmt_sparkle::{StartError, Updater};
use tracing::{info, warn};

use crate::ui::AppSettings;

/// Holds the updater for the lifetime of the process. Sparkle stops its
/// scheduled checks when the last reference goes away.
struct AppUpdate(Updater);

impl Global for AppUpdate {}

/// Start the updater and bring it in line with the current settings.
///
/// A build that cannot update says so once and is otherwise silent: with no
/// global installed, every entry point below turns into a no-op.
pub(crate) fn initialize(testing: bool, cx: &mut App) {
    // A test instance is started and stopped repeatedly and shares the packaged
    // bundle's identity; letting it check for updates would mean network
    // traffic, and eventually an update prompt, from a process nobody is
    // watching.
    if testing {
        return;
    }

    match Updater::start() {
        Ok(updater) => {
            updater.set_automatic_checks(cx.global::<AppSettings>().check_updates);
            cx.set_global(AppUpdate(updater));
        }
        // A build assembled locally names no feed. That is the intended state
        // for it, not a failure.
        Err(StartError::NoFeedConfigured) => {
            info!("this build has no update feed; automatic updates are off");
        }
        Err(error) => warn!("the updater did not start: {error}"),
    }
}

/// Mirror the application's own setting onto Sparkle's schedule.
pub(crate) fn settings_changed(cx: &mut App) {
    let enabled = cx.global::<AppSettings>().check_updates;
    if let Some(update) = cx.try_global::<AppUpdate>() {
        update.0.set_automatic_checks(enabled);
    }
}

/// Check now on the user's behalf, showing Sparkle's own progress and result
/// windows. Runs whether or not background checking is on, which is the point
/// of having a menu item for it.
pub(crate) fn check_now(cx: &App) {
    if let Some(update) = cx.try_global::<AppUpdate>() {
        update.0.check_for_updates();
    }
}

/// Whether a check can be started right now. False while one is already
/// running, and for a build that has no updater at all.
pub(crate) fn can_check(cx: &App) -> bool {
    cx.try_global::<AppUpdate>()
        .is_some_and(|update| update.0.can_check_for_updates())
}
