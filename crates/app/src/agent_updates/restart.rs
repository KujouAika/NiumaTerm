//! Restart of every running harness: each agent tab's backend stops, starts
//! again, and resumes its saved conversation, through the same suspend and
//! restore cycle a provider update uses.

use std::time::Instant;

use app::agent_tab::RecoveryReadiness;
use app::agent_tab::execution::{AgentSession, SessionRegistry};
use gpui::prelude::*;
use gpui::{AnyWindowHandle, App, AsyncApp, Entity, SharedString, Window, div};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::dialog::{DIALOG_BUTTON_MIN_WIDTH, Dialog, DialogClose, DialogFooter};
use gpui_component::notification::{Notification, NotificationType};
use gpui_component::{ActiveTheme as _, WindowExt as _};
use nmt_agent::update::{UpdateError, UpdateErrorKind};
use rust_i18n::t;

use crate::agent_updates::AgentUpdates;
use crate::agent_updates::maintenance::{PreflightFailure, UpdateMode, run_restart};
use crate::agent_updates::transaction::{SessionEnvironment, combine_transaction_error};

/// Asks for confirmation before restarting. The dialog offers waiting for
/// idle tabs and stopping them now only when some tab has work in flight.
pub(crate) fn request_restart(window: &mut Window, cx: &mut App) {
    if cx.global::<AgentUpdates>().restarting {
        notify(
            NotificationType::Info,
            t!("agent-restart-in-progress").into(),
            window,
            cx,
        );

        return;
    }

    let sessions = restartable_sessions(cx);

    if sessions.is_empty() {
        notify(
            NotificationType::Info,
            t!("agent-restart-no-sessions").into(),
            window,
            cx,
        );

        return;
    }

    let total = sessions.len();

    let busy = sessions
        .iter()
        .filter(|session| {
            matches!(
                session.read(cx).recovery_readiness(),
                RecoveryReadiness::Busy(_)
            )
        })
        .count();

    window.open_dialog(cx, move |dialog, _, _| {
        confirm_dialog(dialog.centered(true), total, busy)
    });
}

fn confirm_dialog(dialog: Dialog, total: usize, busy: usize) -> Dialog {
    let footer = if busy == 0 {
        DialogFooter::new().child(restart_button(
            "agent-restart-confirm",
            t!("agent-restart-dialog-restart").into(),
            UpdateMode::WhenIdle,
        ))
    } else {
        DialogFooter::new()
            .child(restart_button(
                "agent-restart-when-idle",
                t!("agent-restart-dialog-when-idle").into(),
                UpdateMode::WhenIdle,
            ))
            .child(restart_button(
                "agent-restart-stop-now",
                t!("agent-restart-dialog-stop-now").into(),
                UpdateMode::StopNow,
            ))
    };

    dialog
        .title(t!("agent-restart-dialog-title"))
        .overlay_closable(false)
        .content(move |content, _, cx| {
            content
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .child(t!("agent-restart-dialog-body", count = total).into_owned()),
                )
                .when(busy > 0, |content| {
                    content.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                t!("agent-restart-dialog-active-work", count = busy).into_owned(),
                            ),
                    )
                })
        })
        .footer(
            footer.child(
                DialogClose::new().child(
                    Button::new("agent-restart-cancel")
                        .min_w(DIALOG_BUTTON_MIN_WIDTH)
                        .label(t!("agent-restart-dialog-cancel")),
                ),
            ),
        )
}

fn restart_button(id: &'static str, label: SharedString, mode: UpdateMode) -> Button {
    let button = Button::new(id).min_w(DIALOG_BUTTON_MIN_WIDTH).label(label);

    match mode {
        UpdateMode::WhenIdle => button.primary(),
        UpdateMode::StopNow => button.danger(),
    }
    .on_click(move |_, window, cx| {
        window.close_dialog(cx);

        start_restart(mode, window, cx);
    })
}

/// Every open agent tab outside an update or restart cycle. A tab already in
/// one is mid-update or holds a failed recovery the user resolves from the
/// tab itself; stopping it again would overwrite that state.
fn restartable_sessions(cx: &App) -> Vec<Entity<AgentSession>> {
    SessionRegistry::sessions(cx)
        .into_iter()
        .filter(|session| !session.read(cx).in_suspension_cycle())
        .collect()
}

fn start_restart(mode: UpdateMode, window: &mut Window, cx: &mut App) {
    if cx.global::<AgentUpdates>().restarting {
        return;
    }

    // The tab list is read again on confirmation, because tabs can open,
    // close, or enter an update while the dialog is up.
    let sessions = restartable_sessions(cx);

    if sessions.is_empty() {
        return;
    }

    cx.global_mut::<AgentUpdates>().restarting = true;

    let window = window.window_handle();

    cx.spawn(async move |cx| drive_restart(mode, sessions, window, cx).await)
        .detach();
}

async fn drive_restart(
    mode: UpdateMode,
    sessions: Vec<Entity<AgentSession>>,
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
) {
    let mut environment = SessionEnvironment {
        sessions,
        target: (),
        started: Instant::now(),
        cx,
    };

    let result = run_restart(&mut environment, mode).await;

    let cx = environment.cx;

    cx.update(|cx| cx.global_mut::<AgentUpdates>().restarting = false);

    let (kind, message): (_, SharedString) = match result {
        Ok(outcome) => {
            let error = combine_transaction_error(
                outcome
                    .suspend_error
                    .map(|message| UpdateError::new(UpdateErrorKind::Recovery, message)),
                outcome.restore_failures,
            );

            match error {
                Some(error) => (NotificationType::Error, error.message().to_owned().into()),
                None => (
                    NotificationType::Success,
                    t!("agent-restart-done", count = outcome.restarted)
                        .into_owned()
                        .into(),
                ),
            }
        }
        Err(PreflightFailure::MissingIdentity(message)) => {
            (NotificationType::Error, message.into())
        }
        Err(PreflightFailure::InterruptionTimeout) => (
            NotificationType::Error,
            t!("agent-update-interruption-timeout").into(),
        ),
    };

    // The window that asked can close during the restart; the tabs still
    // show their own outcome, so a missing window only drops the summary.
    let _ = window.update(cx, |_, window, cx| notify(kind, message, window, cx));
}

fn notify(kind: NotificationType, message: SharedString, window: &mut Window, cx: &mut App) {
    window.push_notification(Notification::new().with_type(kind).message(message), cx);
}
