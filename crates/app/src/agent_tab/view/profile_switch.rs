use gpui::Entity;
use gpui::prelude::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::dialog::{DIALOG_BUTTON_MIN_WIDTH, Dialog, DialogClose, DialogFooter};
use gpui_component::{ActiveTheme as _, WindowExt as _, v_flex};
use nmt_config::profile::AgentProfile;
use rust_i18n::t;

use crate::agent_tab::AgentPane;

/// The dialog asking before a tab with a conversation relaunches on
/// `profile`. The relaunched tab starts a fresh conversation, so switching
/// ends the one on screen; cancelling leaves the tab as it is.
pub(crate) fn profile_switch_dialog(
    dialog: Dialog,
    pane: &Entity<AgentPane>,
    profile: &AgentProfile,
    label: &str,
) -> Dialog {
    let pane = pane.clone();
    let profile = profile.clone();
    let label = label.to_string();

    dialog
        .title(t!("agent-profile-switch-title", name = label))
        .overlay_closable(false)
        .content(move |content, _, cx| {
            content.child(
                v_flex()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("agent-profile-switch-message", name = label)),
            )
        })
        .footer(
            DialogFooter::new()
                .child(
                    Button::new("agent-profile-switch-confirm")
                        .min_w(DIALOG_BUTTON_MIN_WIDTH)
                        .label(t!("agent-profile-switch-confirm"))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);

                            pane.update(cx, |pane, cx| pane.switch_profile(profile.clone(), cx));
                        }),
                )
                .child(
                    DialogClose::new().child(
                        Button::new("agent-profile-switch-cancel")
                            .min_w(DIALOG_BUTTON_MIN_WIDTH)
                            .primary()
                            .label(t!("agent-profile-switch-cancel")),
                    ),
                ),
        )
}
