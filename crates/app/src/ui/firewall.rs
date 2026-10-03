//! Whether Windows Firewall lets other computers reach this NiumaTerm, as
//! the remote settings show it, and the elevated change that fixes it.

use std::env;

use anyhow::Context as _;
use gpui::{App, BorrowAppContext as _, Global, SharedString};
use nmt_platform::firewall::{self, ElevationOutcome, FirewallStatus};
use nmt_platform::runtime;
use tracing::warn;

#[derive(Default)]
pub(crate) struct Firewall {
    /// `None` until the first check finishes, and after a check failed.
    status: Option<FirewallStatus>,

    /// Set while the elevated helper runs.
    busy: bool,

    /// Why the last change failed, until the next attempt.
    error: Option<SharedString>,
}

impl Global for Firewall {}

impl Firewall {
    pub(crate) fn status(&self) -> Option<FirewallStatus> {
        self.status
    }

    pub(crate) fn busy(&self) -> bool {
        self.busy
    }

    pub(crate) fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }
}

/// Install the global and run the first check in the background.
pub(crate) fn initialize(cx: &mut App) {
    cx.set_global(Firewall::default());

    refresh(cx);
}

/// Read the firewall rules for this executable again. Enumerating every
/// rule takes the firewall service a noticeable moment, so it runs off the
/// UI thread.
pub(crate) fn refresh(cx: &mut App) {
    // Settings views built without the application (tests) have no state
    // to update.
    if !cx.has_global::<Firewall>() {
        return;
    }

    let task = runtime().spawn_blocking(|| {
        let program = env::current_exe()?;

        firewall::status(&program)
    });

    cx.spawn(async move |cx| {
        let result = task
            .await
            .context("the firewall check stopped")
            .and_then(|status| status);

        cx.update_global::<Firewall, _>(|state, _| {
            state.status = result
                .inspect_err(|error| warn!(%error, "cannot read the firewall rules"))
                .ok();
        });
    })
    .detach();
}

/// Ask Windows to run this executable elevated to admit it through the
/// firewall, then check again. A declined prompt changes nothing.
pub(crate) fn allow(cx: &mut App) {
    cx.update_global::<Firewall, _>(|state, _| {
        state.busy = true;
        state.error = None;
    });

    let task = runtime().spawn_blocking(|| {
        let program = env::current_exe()?;

        firewall::request_allow(&program)
    });

    cx.spawn(async move |cx| {
        let result = task
            .await
            .context("the firewall change stopped")
            .and_then(|outcome| outcome);

        cx.update(|cx| {
            cx.update_global::<Firewall, _>(|state, _| {
                state.busy = false;

                state.error = result
                    .as_ref()
                    .err()
                    .map(|error| format!("{error:#}").into());
            });

            if let Ok(ElevationOutcome::Configured) = result {
                refresh(cx);
            }
        });
    })
    .detach();
}
