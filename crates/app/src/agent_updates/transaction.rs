use std::collections::HashSet;
use std::time::{Duration, Instant};

use app::agent_tab::RecoveryReadiness;
use app::agent_tab::execution::{AgentSession, SessionRegistry};
use futures::future::join_all;
use gpui::prelude::*;
use gpui::{App, AsyncApp, Entity, Window, div};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::dialog::{DIALOG_BUTTON_MIN_WIDTH, Dialog, DialogClose, DialogFooter};
use gpui_component::{ActiveTheme as _, WindowExt as _};
use nmt_agent::session::lifecycle::{RecoverySnapshot, RestorationReadiness};
use nmt_agent::update::{
    InstallationKey, ProviderKind, UpdateCoordinator, UpdateError, UpdateErrorKind, UpdatePhase,
    UpdateProgress, VersionStatus,
};
use nmt_config::profile::AgentKind;
use rust_i18n::t;

use crate::agent_updates::AgentUpdates;
use crate::agent_updates::maintenance::{
    PreflightFailure, RecoveryEnvironment, UpdateEnvironment, UpdateMode, run_transaction,
};
use crate::utils::on_runtime;

pub(super) fn combine_transaction_error(
    operation_error: Option<UpdateError>,
    restore_failures: usize,
) -> Option<UpdateError> {
    if restore_failures == 0 {
        return operation_error;
    }

    Some(UpdateError::new(
        UpdateErrorKind::Recovery,
        operation_error.map_or_else(
            || t!("agent-update-reconnect-failures", count = restore_failures).into_owned(),
            |error| {
                t!(
                    "agent-update-error-with-reconnect-failures",
                    error = error.message(),
                    count = restore_failures
                )
                .into_owned()
            },
        ),
    ))
}

pub(crate) fn request_update(key: InstallationKey, window: &mut Window, cx: &mut App) {
    let sessions = matching_sessions(&key, cx);

    let busy = sessions
        .iter()
        .filter(|session| {
            matches!(
                session.read(cx).recovery_readiness(),
                RecoveryReadiness::Busy(_)
            )
        })
        .count();

    if busy == 0 {
        start_transaction(key, UpdateMode::WhenIdle, sessions, cx);

        return;
    }

    window.open_dialog(cx, move |dialog, _, _| {
        active_work_dialog(dialog.centered(true), &key, busy)
    });
}

fn active_work_dialog(dialog: Dialog, key: &InstallationKey, busy: usize) -> Dialog {
    let wait_key = key.clone();
    let stop_key = key.clone();

    dialog
        .title(t!("agent-update-dialog-title"))
        .overlay_closable(false)
        .content(move |content, _, cx| {
            content.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("agent-update-dialog-active-work", count = busy).into_owned()),
            )
        })
        .footer(
            DialogFooter::new()
                .child(
                    Button::new("agent-update-when-idle")
                        .min_w(DIALOG_BUTTON_MIN_WIDTH)
                        .primary()
                        .label(t!("agent-update-dialog-when-idle"))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);

                            let sessions = matching_sessions(&wait_key, cx);

                            start_transaction(wait_key.clone(), UpdateMode::WhenIdle, sessions, cx);
                        }),
                )
                .child(
                    Button::new("agent-update-stop-now")
                        .min_w(DIALOG_BUTTON_MIN_WIDTH)
                        .danger()
                        .label(t!("agent-update-dialog-stop-now"))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);

                            let sessions = matching_sessions(&stop_key, cx);

                            start_transaction(stop_key.clone(), UpdateMode::StopNow, sessions, cx);
                        }),
                )
                .child(
                    DialogClose::new().child(
                        Button::new("agent-update-cancel")
                            .min_w(DIALOG_BUTTON_MIN_WIDTH)
                            .label(t!("agent-update-dialog-cancel")),
                    ),
                ),
        )
}

fn matching_sessions(key: &InstallationKey, cx: &mut App) -> Vec<Entity<AgentSession>> {
    let sessions = SessionRegistry::sessions(cx);

    let installations = sessions
        .iter()
        .map(|session| session.read(cx).installation_key())
        .collect::<Vec<_>>();

    let affected = affected_installation_indices(key, &installations)
        .into_iter()
        .collect::<HashSet<_>>();

    sessions
        .into_iter()
        .enumerate()
        .filter_map(|(index, session)| affected.contains(&index).then_some(session))
        .collect()
}

/// A tab whose harness has no vendor-managed installation has no key, so it
/// matches no target and is never suspended by another harness's update.
pub(super) fn affected_installation_indices(
    target: &InstallationKey,
    installations: &[Option<InstallationKey>],
) -> Vec<usize> {
    installations
        .iter()
        .enumerate()
        .filter_map(|(index, key)| (key.as_ref() == Some(target)).then_some(index))
        .collect()
}

fn start_transaction(
    key: InstallationKey,
    mode: UpdateMode,
    sessions: Vec<Entity<AgentSession>>,
    cx: &mut App,
) {
    let updates = cx.global::<AgentUpdates>();

    // A restart of every harness owns the suspension state of the sessions
    // this update would also stop; running both would let one overwrite the
    // other's suspension state on a shared session.
    if updates.restarting {
        return;
    }

    let coordinator = updates.coordinator.clone();

    if coordinator.begin_update(&key).is_err() {
        return;
    }

    AgentUpdates::notify_changed(cx);

    cx.spawn(async move |cx| drive_transaction(key, mode, sessions, coordinator, cx).await)
        .detach();
}

async fn drive_transaction(
    key: InstallationKey,
    mode: UpdateMode,
    sessions: Vec<Entity<AgentSession>>,
    coordinator: UpdateCoordinator,
    cx: &mut AsyncApp,
) {
    let mut environment = SessionEnvironment {
        sessions,
        target: InstallationTarget {
            coordinator: coordinator.clone(),
            key: key.clone(),
        },
        started: Instant::now(),
        cx,
    };

    let (verified, error) = match run_transaction(&mut environment, mode).await {
        Ok(outcome) => (
            outcome.verified,
            combine_transaction_error(outcome.operation_error, outcome.restore_failures),
        ),
        Err(error) => {
            let message = match error {
                PreflightFailure::MissingIdentity(message) => message,
                PreflightFailure::InterruptionTimeout => {
                    t!("agent-update-interruption-timeout").to_string()
                }
            };

            (
                None,
                Some(UpdateError::new(UpdateErrorKind::Recovery, message)),
            )
        }
    };

    coordinator.finish_update(&key, verified, error, 0);

    environment.cx.update(AgentUpdates::notify_changed);
}

/// The live sessions one cycle stops and restores, and the record `target`
/// that receives its progress.
pub(super) struct SessionEnvironment<'a, T> {
    pub(super) sessions: Vec<Entity<AgentSession>>,
    pub(super) target: T,
    pub(super) started: Instant,
    pub(super) cx: &'a mut AsyncApp,
}

/// Receives the progress of a session cycle.
pub(super) trait PhaseTarget {
    fn publish(&self, phase: UpdatePhase, progress: Option<UpdateProgress>, cx: &mut AsyncApp);
}

/// An update reports its progress on the installation's coordinator record,
/// which the update notification and the settings rows read.
pub(super) struct InstallationTarget {
    coordinator: UpdateCoordinator,
    key: InstallationKey,
}

impl PhaseTarget for InstallationTarget {
    fn publish(&self, phase: UpdatePhase, progress: Option<UpdateProgress>, cx: &mut AsyncApp) {
        self.coordinator.transition(&self.key, phase, progress);

        cx.update(AgentUpdates::notify_changed);
    }
}

/// A restart of every harness spans installations and harnesses with none,
/// so it has no record to report to; each tab shows its own state.
impl PhaseTarget for () {
    fn publish(&self, _: UpdatePhase, _: Option<UpdateProgress>, _: &mut AsyncApp) {}
}

impl<T: PhaseTarget> RecoveryEnvironment for SessionEnvironment<'_, T> {
    fn identity_failure(&mut self) -> Option<String> {
        self.cx.update(|cx| {
            self.sessions.iter().find_map(|session| {
                match session.read(cx).recovery_identity_snapshot() {
                    RecoveryReadiness::MissingIdentity(message) => Some(message),
                    _ => None,
                }
            })
        })
    }

    fn prepare(&mut self, mode: UpdateMode) {
        self.cx.update(|cx| {
            for session in &self.sessions {
                session.update(cx, |session, cx| match mode {
                    UpdateMode::StopNow => session.stop_active_work_for_update(cx),
                    UpdateMode::WhenIdle => session.prepare_update_wait(cx),
                });
            }
        });
    }

    fn readiness(&mut self) -> Vec<RecoveryReadiness> {
        self.cx.update(|cx| {
            self.sessions
                .iter()
                .map(|session| session.read(cx).recovery_readiness())
                .collect()
        })
    }

    fn cancel_wait(&mut self) {
        self.cx.update(|cx| {
            for session in &self.sessions {
                session.update(cx, |session, cx| session.cancel_update_wait(cx));
            }
        });
    }

    async fn suspend(&mut self, mode: UpdateMode) -> Vec<Result<(), String>> {
        let tasks = self.cx.update(|cx| {
            self.sessions
                .iter()
                .map(|session| {
                    session.update(cx, |session, cx| {
                        session.suspend_for_update(mode.interrupts_active_work(), cx)
                    })
                })
                .collect::<Vec<_>>()
        });

        join_all(tasks).await
    }

    fn restore(&mut self, snapshots: &[RecoverySnapshot], suspended: &[usize]) {
        self.cx.update(|cx| {
            for &index in suspended {
                self.sessions[index].update(cx, |session, cx| {
                    session.restore_after_update(&snapshots[index], cx)
                });
            }
        });
    }

    fn restoration_readiness(&mut self, suspended: &[usize]) -> Vec<RestorationReadiness> {
        self.cx.update(|cx| {
            suspended
                .iter()
                .map(|&index| self.sessions[index].read(cx).restoration_readiness())
                .collect()
        })
    }

    fn recovery_timed_out(&mut self, pending: &[usize]) {
        self.cx.update(|cx| {
            for &index in pending {
                self.sessions[index].update(cx, |session, cx| {
                    session
                        .fail_update_recovery(t!("agent-update-recovery-timeout").to_string(), cx)
                });
            }
        });
    }

    fn publish(&mut self, phase: UpdatePhase, progress: Option<UpdateProgress>) {
        self.target.publish(phase, progress, self.cx);
    }

    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    async fn wait(&mut self, duration: Duration) {
        self.cx.background_executor().timer(duration).await;
    }
}

impl UpdateEnvironment for SessionEnvironment<'_, InstallationTarget> {
    async fn update(&mut self) -> Result<(), UpdateError> {
        self.cx.update(|cx| {
            for session in &self.sessions {
                session.update(cx, |session, cx| session.mark_provider_updating(cx));
            }
        });

        let coordinator = self.target.coordinator.clone();
        let key = self.target.key.clone();

        on_runtime(async move { coordinator.run_vendor_update(&key).await.map(|_| ()) }).await
    }

    async fn verify(&mut self) -> Result<VersionStatus, UpdateError> {
        let coordinator = self.target.coordinator.clone();
        let key = self.target.key.clone();

        on_runtime(async move { coordinator.verify(&key).await }).await
    }
}

/// The updatable installation this profile resolves to. `None` means the
/// harness is installed and updated through the user's own package manager, so
/// there is nothing for the update surface to probe or replace.
pub(crate) fn provider_for_profile(kind: AgentKind) -> Option<ProviderKind> {
    match kind {
        AgentKind::Claude => Some(ProviderKind::Claude),
        AgentKind::Codex => Some(ProviderKind::Codex),
        AgentKind::DeepSeek => None,
    }
}
