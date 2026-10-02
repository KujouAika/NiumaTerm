//! Renaming a computer for remote sessions, from either end. The name is the
//! host's own: whichever side edits it, the host stores it and every paired
//! device shows it, so each rename asks for the name and then confirms it.

use gpui::{App, AppContext as _, ParentElement as _, Styled as _, Window};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::dialog::{DIALOG_BUTTON_MIN_WIDTH, DialogAction, DialogClose, DialogFooter};
use gpui_component::input::{Input, InputState};
use gpui_component::label::Label;
use gpui_component::{ActiveTheme as _, WindowExt as _, v_flex};
use nmt_remote_core::identity::DeviceId;
use nmt_remote_core::messages::{MAX_DEVICE_NAME_CHARS, device_name};
use rust_i18n::t;

use crate::ui::{AppSettings, remote};

/// The computer a rename is for.
#[derive(Clone)]
pub(crate) enum RenameTarget {
    /// This computer, as the host its paired devices connect to.
    ThisComputer,
    /// A paired host, renamed on that host through the connection to it.
    Host(DeviceId),
}

/// Ask for a new name for `target`, now called `current`, then confirm it.
pub(crate) fn open_rename_dialog(
    target: RenameTarget,
    current: String,
    window: &mut Window,
    cx: &mut App,
) {
    let default_name = remote::default_device_name();

    let input = cx.new(|cx| {
        let input = InputState::new(window, cx).default_value(current.clone());

        // An empty name returns this computer to its own name, which the
        // placeholder shows; a paired host cannot be reset from here.
        match target {
            RenameTarget::ThisComputer => input.placeholder(default_name.clone()),
            RenameTarget::Host(_) => input,
        }
    });

    let (title, hint) = match &target {
        RenameTarget::ThisComputer => (
            t!("remote-rename-this-title"),
            t!(
                "remote-rename-this-hint",
                default = default_name.as_str(),
                max = MAX_DEVICE_NAME_CHARS
            ),
        ),
        RenameTarget::Host(_) => (
            t!("remote-rename-host-title", name = current.as_str()),
            t!(
                "remote-rename-host-hint",
                name = current.as_str(),
                max = MAX_DEVICE_NAME_CHARS
            ),
        ),
    };

    let focused = input.clone();

    window.open_dialog(cx, move |dialog, _, cx| {
        let name_input = input.clone();
        let target = target.clone();
        let current = current.clone();
        let default_name = default_name.clone();

        dialog
            .centered(true)
            .title(title.clone())
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        Label::new(hint.clone())
                            .text_sm()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(Input::new(&input)),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new().child(
                            Button::new("remote-rename-cancel")
                                .min_w(DIALOG_BUTTON_MIN_WIDTH)
                                .label(t!("settings-common-cancel")),
                        ),
                    )
                    .child(
                        DialogAction::new().child(
                            Button::new("remote-rename-next")
                                .min_w(DIALOG_BUTTON_MIN_WIDTH)
                                .primary()
                                .label(t!("settings-remote-rename")),
                        ),
                    ),
            )
            .on_ok(move |_, window, cx| {
                let typed = name_input.read(cx).value();

                // `None` stores an empty name, which means the computer's own.
                let chosen = match (&target, typed.trim().is_empty()) {
                    (RenameTarget::ThisComputer, true) => None,
                    _ => match device_name(&typed) {
                        Some(name) => Some(name),
                        None => return false,
                    },
                };

                let shown = chosen.clone().unwrap_or_else(|| default_name.clone());

                if shown == current {
                    return true;
                }

                // This dialog closes before the confirmation opens, so the
                // confirmation is the top dialog its own buttons close.
                window.close_dialog(cx);

                confirm_rename(target.clone(), chosen, shown, &current, window, cx);

                false
            })
    });

    // Opening a dialog focuses the dialog itself and remembers what had
    // focus to restore on close, so the input takes focus only afterwards.
    focused.update(cx, |input, cx| input.focus(window, cx));
}

/// Confirm that `shown` replaces `current` everywhere the name appears.
fn confirm_rename(
    target: RenameTarget,
    chosen: Option<String>,
    shown: String,
    current: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let title = t!("remote-rename-confirm-title", name = shown.as_str());

    let message = match target {
        RenameTarget::ThisComputer => t!("remote-rename-this-confirm"),
        RenameTarget::Host(_) => t!("remote-rename-host-confirm", name = current),
    };

    window.open_dialog(cx, move |dialog, _, _| {
        let target = target.clone();
        let chosen = chosen.clone();

        dialog
            .centered(true)
            .title(title.clone())
            .child(message.clone())
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new().child(
                            Button::new("remote-rename-confirm-cancel")
                                .min_w(DIALOG_BUTTON_MIN_WIDTH)
                                .label(t!("settings-common-cancel")),
                        ),
                    )
                    .child(
                        Button::new("remote-rename-confirm")
                            .min_w(DIALOG_BUTTON_MIN_WIDTH)
                            .primary()
                            .label(t!("settings-remote-rename"))
                            .on_click(move |_, window, cx: &mut App| {
                                window.close_dialog(cx);

                                let name = chosen.clone().unwrap_or_default();

                                match &target {
                                    RenameTarget::ThisComputer => {
                                        cx.global_mut::<AppSettings>()
                                            .edit_remote(|section| section.device_name = name);
                                    }
                                    RenameTarget::Host(id) => {
                                        remote::rename_host(id, name, window, cx);
                                    }
                                }
                            }),
                    ),
            )
    });
}
