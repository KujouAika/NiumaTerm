//! The dialogs that ask before a pane, tab, workspace, or window closes, and
//! the wording they share.

use std::borrow::Cow;
use std::io;
use std::rc::Rc;

use app::agent_tab::orchestration::OrchestrationPane;
use gpui::prelude::*;
use gpui::{App, Context, Entity, SharedString, Task, Window, div};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{
    DIALOG_BUTTON_MIN_WIDTH, Dialog, DialogButtonProps, DialogClose, DialogFooter,
};
use gpui_component::{ActiveTheme, StyledExt, WindowExt, v_flex};
use nmt_config::system::WarnBeforeTerminatingShell;
use rust_i18n::t;
use tracing::warn;

use crate::ui;
use crate::ui::settings::AppSettings;
use crate::ui::shell::AppWindow;
use crate::ui::shell::tab_surface::TabSurface;
use crate::workspace::WorkspaceId;

/// The description of a close confirmation: `with_processes` when `count`
/// found child processes still running, `plain` otherwise.
pub(super) fn close_description(
    count: io::Result<usize>,
    plain: &str,
    with_processes: &str,
) -> String {
    match count {
        Ok(count) if count > 0 => {
            t!(with_processes, processes = &processes_running(count)).into_owned()
        }
        Ok(_) => t!(plain).into_owned(),
        Err(error) => {
            warn!("failed to count processes before closing: {error}");

            t!(plain).into_owned()
        }
    }
}

/// "1 child process is running" / "N child processes are running": the
/// lead-in of every close-confirmation description.
fn processes_running(count: usize) -> String {
    if count == 1 {
        t!("shell-close-one-process-running").to_string()
    } else {
        t!("shell-close-many-processes-running", count = count).into_owned()
    }
}

/// Shared structure of every close-confirmation alert: title +
/// description, OK runs `on_confirm` against this shell. `note` adds a
/// bold line under the description for a consequence the description
/// itself does not cover.
pub(super) fn open_close_confirm(
    window: &mut Window,
    cx: &mut Context<AppWindow>,
    // Dialog callbacks can rebuild their content, so they retain a
    // translated title that can be reused on each invocation.
    title: Cow<'static, str>,
    description: String,
    note: Option<SharedString>,
    on_confirm: impl Fn(&mut AppWindow, &mut Window, &mut Context<AppWindow>) + 'static,
) {
    let shell = cx.entity();
    let on_confirm = Rc::new(on_confirm);

    window.open_alert_dialog(cx, move |alert, _, _| {
        let shell = shell.clone();
        let on_confirm = Rc::clone(&on_confirm);

        alert
            .centered(true)
            .confirm()
            .title(title.clone())
            .description(
                v_flex()
                    .gap_1()
                    .child(description.clone())
                    .children(note.clone().map(|note| div().font_bold().child(note))),
            )
            .on_ok(move |_, window, cx| {
                let on_confirm = Rc::clone(&on_confirm);

                shell.update(cx, |this, cx| on_confirm(this, window, cx));

                true
            })
    });
}

/// The alert for a window whose settings could not be saved: closing anyway
/// discards the unsaved edits, and cancelling keeps the window open so they
/// can be retried.
pub(super) fn open_save_failed_close(
    description: String,
    note: Option<SharedString>,
    window: &mut Window,
    cx: &mut Context<AppWindow>,
) {
    // `remove_window` tears the window down directly (no WM_CLOSE
    // round-trip), so this dialog won't re-trigger.
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .centered(true)
            .title(t!("settings-save-failed-title"))
            .description(
                v_flex()
                    .gap_1()
                    .child(description.clone())
                    .children(note.clone().map(|note| div().font_bold().child(note))),
            )
            .button_props(
                DialogButtonProps::default()
                    .show_cancel(true)
                    .ok_text(t!("settings-close-without-saving"))
                    .cancel_text(t!("shell-close-cancel")),
            )
            .on_ok(|_, window, cx| {
                if cx.windows().len() == 1 {
                    cx.global_mut::<AppSettings>().discard_on_exit();
                }

                window.remove_window();

                true
            })
    });
}

pub(super) fn close_last_workspace_dialog(
    dialog: Dialog,
    shell: &Entity<AppWindow>,
    id: WorkspaceId,
    message: &str,
    note: &Option<SharedString>,
) -> Dialog {
    let quit_shell = shell.clone();
    let replace_shell = shell.clone();
    let message = message.to_string();
    let note = note.clone();

    dialog
        .title(t!("shell-close-last-workspace-title"))
        .overlay_closable(false)
        .content(move |content, _, cx| {
            content.child(
                v_flex()
                    .gap_1()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(message.clone())
                    .children(note.clone().map(|note| div().font_bold().child(note))),
            )
        })
        .footer(
            DialogFooter::new()
                .child(
                    Button::new("replace-ws")
                        .min_w(DIALOG_BUTTON_MIN_WIDTH)
                        .label(t!("shell-close-new-default-workspace"))
                        .primary()
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);

                            replace_shell
                                .update(cx, |this, cx| this.replace_last_workspace(id, window, cx));
                        }),
                )
                .child(
                    Button::new("quit-app")
                        .min_w(DIALOG_BUTTON_MIN_WIDTH)
                        .label(t!("shell-close-quit"))
                        .danger()
                        .on_click(move |_, window, cx| {
                            let shell = quit_shell.downgrade();
                            let saved = ui::settings::save_settings(window, cx);

                            window
                                .spawn(cx, async move |cx| {
                                    let saved = saved.await;

                                    let _ = cx.update(|window, cx| {
                                        if !saved {
                                            window.close_dialog(cx);

                                            return;
                                        }

                                        if shell
                                            .update(cx, |this, _| this.doom_workspace(id))
                                            .is_ok()
                                        {
                                            cx.quit();
                                        }
                                    });
                                })
                                .detach();
                        }),
                )
                .child(
                    DialogClose::new().child(
                        Button::new("keep-ws")
                            .min_w(DIALOG_BUTTON_MIN_WIDTH)
                            .label(t!("shell-close-cancel")),
                    ),
                ),
        )
}

pub(super) fn should_confirm_close(
    confirm: bool,
    warn: WarnBeforeTerminatingShell,
    child_process_count: &io::Result<usize>,
) -> bool {
    confirm
        || match child_process_count {
            Ok(count) => warn.should_warn(*count),
            Err(_) => warn != WarnBeforeTerminatingShell::Disabled,
        }
}

/// The Orchestration panes among `surfaces` whose canvas holds edits its
/// file does not.
pub(super) fn unsaved_orchestrations<'a>(
    surfaces: impl IntoIterator<Item = &'a TabSurface>,
    cx: &App,
) -> Vec<Entity<OrchestrationPane>> {
    surfaces
        .into_iter()
        .filter_map(TabSurface::orchestration)
        .filter(|pane| pane.read(cx).has_unsaved_edits(cx))
        .cloned()
        .collect()
}

/// Ask whether to save or discard the unsaved canvas edits of `panes`
/// before a close goes on. `then` repeats the close once they are saved or
/// discarded, so the close's own confirmations still follow; Cancel keeps
/// everything open, and a failed save stops the close with its error shown
/// in the pane.
pub(super) fn ask_about_unsaved_orchestrations(
    panes: Vec<Entity<OrchestrationPane>>,
    then: impl Fn(&mut AppWindow, &mut Window, &mut Context<AppWindow>) + 'static,
    window: &mut Window,
    cx: &mut Context<AppWindow>,
) {
    let shell = cx.entity();
    let then = Rc::new(then);
    let panes = Rc::new(panes);

    window.open_dialog(cx, move |dialog, _, _| {
        let (save_shell, discard_shell) = (shell.downgrade(), shell.clone());
        let (save_then, discard_then) = (Rc::clone(&then), Rc::clone(&then));
        let (save_panes, discard_panes) = (Rc::clone(&panes), Rc::clone(&panes));

        dialog
            .centered(true)
            .title(t!("shell-close-unsaved-orchestration-title"))
            .overlay_closable(false)
            .content(|content, _, cx| {
                content.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("shell-close-unsaved-orchestration-description").into_owned()),
                )
            })
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("save-orchestrations")
                            .min_w(DIALOG_BUTTON_MIN_WIDTH)
                            .label(t!("orchestration-save"))
                            .primary()
                            .on_click(move |_, window, cx| {
                                let saves: Vec<Task<bool>> = save_panes
                                    .iter()
                                    .map(|pane| pane.update(cx, |pane, cx| pane.save_unsaved(cx)))
                                    .collect();

                                let shell = save_shell.clone();
                                let then = Rc::clone(&save_then);

                                window
                                    .spawn(cx, async move |cx| {
                                        let mut saved = true;

                                        for save in saves {
                                            saved &= save.await;
                                        }

                                        let _ = cx.update(|window, cx| {
                                            window.close_dialog(cx);

                                            if saved {
                                                let _ = shell
                                                    .update(cx, |this, cx| then(this, window, cx));
                                            }
                                        });
                                    })
                                    .detach();
                            }),
                    )
                    .child(
                        Button::new("discard-orchestrations")
                            .min_w(DIALOG_BUTTON_MIN_WIDTH)
                            .label(t!("orchestration-discard"))
                            .danger()
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);

                                for pane in discard_panes.iter() {
                                    pane.update(cx, |pane, cx| pane.discard_unsaved(cx));
                                }

                                discard_shell.update(cx, |this, cx| discard_then(this, window, cx));
                            }),
                    )
                    .child(
                        DialogClose::new().child(
                            Button::new("keep-orchestrations")
                                .min_w(DIALOG_BUTTON_MIN_WIDTH)
                                .label(t!("shell-close-cancel")),
                        ),
                    ),
            )
    });
}
