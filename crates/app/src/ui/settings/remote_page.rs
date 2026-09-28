use gpui::{
    App, ClipboardItem, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::dialog::{DIALOG_BUTTON_MIN_WIDTH, DialogClose, DialogFooter};
use gpui_component::label::Label;
use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use nmt_remote::connection::Status;
use nmt_remote::store::{PairedDevice, PairedHost, now_ms};
use nmt_remote_core::rpc::SessionInfo;
use rust_i18n::t;

use crate::ui::AppSettings;
use crate::ui::remote::{self, Remote};
use crate::ui::settings::fields::settings_switch;

pub(super) fn remote_page(cx: &App) -> SettingPage {
    let page = SettingPage::new(t!("settings-remote-title")).default_open(true);

    // Settings views built without the application (tests) have no remote
    // state to show.
    let Some(state) = cx.try_global::<Remote>() else {
        return page;
    };

    page.group(hosting_group(state))
        .group(computers_group(state))
}

fn hosting_group(state: &Remote) -> SettingGroup {
    let mut group = SettingGroup::new()
        .title(t!("settings-remote-hosting"))
        .item(
            SettingItem::new(
                t!("settings-remote-enable"),
                settings_switch(
                    |config| config.remote.enabled,
                    |settings, value| settings.edit_remote(|section| section.enabled = value),
                ),
            )
            .description(t!("settings-remote-enable-description").into_owned()),
        )
        .item(
            SettingItem::new(
                t!("settings-remote-relay-url"),
                SettingField::input(
                    |cx| cx.global::<Remote>().relay_url.clone(),
                    |value, cx| cx.global_mut::<Remote>().relay_url = value,
                ),
            )
            .description(t!("settings-remote-relay-url-description").into_owned()),
        )
        .item(
            SettingItem::new(
                t!("settings-remote-relay-key"),
                SettingField::input(
                    |cx| cx.global::<Remote>().relay_key.clone(),
                    |value, cx| cx.global_mut::<Remote>().relay_key = value,
                ),
            )
            .description(t!("settings-remote-relay-key-description").into_owned()),
        )
        .item(relay_apply_item(state));

    let Some(address) = state.hosting_address() else {
        return group;
    };

    let id = state
        .device_id()
        .map(|id| id.to_string())
        .unwrap_or_default();

    group = group
        .item(SettingItem::new(
            t!("settings-remote-address"),
            SettingField::render(move |_, _, _| Label::new(format!("{address}    {id}")).text_sm()),
        ))
        .item(pairing_item(state));

    for device in state.devices() {
        group = group.item(device_item(device));
    }

    for session in state.remote_created_sessions() {
        group = group.item(remote_created_item(session));
    }

    group
}

fn relay_apply_item(state: &Remote) -> SettingItem {
    let relay_on = state.relay_configured();

    SettingItem::render(move |options, _, cx| {
        let status = if relay_on {
            t!("settings-remote-relay-on")
        } else {
            Default::default()
        };

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_3()
            .child(
                Label::new(status)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                Button::new("remote-relay-apply")
                    .outline()
                    .label(t!("settings-remote-relay-apply"))
                    .disabled(options.is_disabled())
                    .on_click(|_, _, cx: &mut App| {
                        if !remote::save_relay_key(cx) {
                            return;
                        }

                        let url = cx.global::<Remote>().relay_url.trim().to_owned();

                        // Hosting restarts on its relay from the settings
                        // observer, which runs even when only the key changed.
                        cx.global_mut::<AppSettings>()
                            .edit_remote(|section| section.relay_url = url);
                    }),
            )
            .into_any_element()
    })
}

/// A terminal a paired device started here. It runs with nobody at the host
/// watching, so the host user can end it.
fn remote_created_item(session: SessionInfo) -> SettingItem {
    SettingItem::render(move |_, _, cx| {
        let id = session.session.clone();
        let opened = session.clone();

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    .child(Label::new(session.title.clone()).text_sm())
                    .child(
                        Label::new(t!("settings-remote-started-remotely"))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!("remote-open-{id}")))
                            .outline()
                            .label(t!("settings-remote-open-here"))
                            .on_click(move |_, window, cx: &mut App| {
                                remote::open_remote_created(&opened, window, cx)
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("remote-close-{id}")))
                            .outline()
                            .label(t!("settings-remote-close-session"))
                            .on_click(move |_, _, cx: &mut App| {
                                remote::close_remote_created(&id, cx)
                            }),
                    ),
            )
            .into_any_element()
    })
}

fn pairing_item(state: &Remote) -> SettingItem {
    let pairing = state.pairing();
    let link = state.pairing_link();

    SettingItem::render(move |options, _, cx| {
        let row = h_flex().w_full().justify_between().items_center().gap_3();

        match &pairing {
            Some((code, expires_at)) => {
                let minutes = expires_at.saturating_sub(now_ms()).div_ceil(60_000);

                row.child(
                    v_flex()
                        .flex_1()
                        .child(Label::new(code.to_string()).text_xl())
                        .child(
                            Label::new(t!("settings-remote-code-hint", minutes = minutes))
                                .text_xs()
                                .text_color(cx.theme().muted_foreground),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .children(link.clone().map(|link| {
                            Button::new("remote-copy-link")
                                .outline()
                                .label(t!("settings-remote-copy-link"))
                                .on_click(move |_, _, cx: &mut App| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))
                                })
                        }))
                        .child(
                            Button::new("remote-cancel-pairing")
                                .outline()
                                .label(t!("settings-remote-cancel"))
                                .on_click(|_, _, cx: &mut App| remote::cancel_pairing(cx)),
                        ),
                )
                .into_any_element()
            }
            None => row
                .child(Label::new(t!("settings-remote-pair-device")).text_sm())
                .child(
                    Button::new("remote-start-pairing")
                        .outline()
                        .label(t!("settings-remote-show-code"))
                        .disabled(options.is_disabled())
                        .on_click(|_, _, cx: &mut App| remote::start_pairing(cx)),
                )
                .into_any_element(),
        }
    })
}

fn device_item(device: PairedDevice) -> SettingItem {
    SettingItem::render(move |_, _, cx| {
        let id = device.id.clone();

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    .child(Label::new(device.name.clone()).text_sm())
                    .child(
                        Label::new(format!("{}    {}", device.platform, device.id))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "remote-remove-{}",
                    device.id.as_str()
                )))
                .outline()
                .label(t!("settings-remote-remove"))
                .on_click(move |_, _, cx: &mut App| remote::remove_device(&id, cx)),
            )
            .into_any_element()
    })
}

fn computers_group(state: &Remote) -> SettingGroup {
    let mut group = SettingGroup::new()
        .title(t!("settings-remote-computers"))
        .item(
            SettingItem::new(
                t!("settings-remote-connect-address"),
                SettingField::input(
                    |cx| cx.global::<Remote>().address.clone(),
                    |value, cx| cx.global_mut::<Remote>().address = value,
                ),
            )
            .description(t!("settings-remote-connect-address-description").into_owned()),
        )
        .item(
            SettingItem::new(
                t!("settings-remote-connect-code"),
                SettingField::input(
                    |cx| cx.global::<Remote>().code.clone(),
                    |value, cx| cx.global_mut::<Remote>().code = value,
                ),
            )
            .description(t!("settings-remote-connect-code-description").into_owned()),
        )
        .item(SettingItem::render(|options, _, cx| {
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_3()
                .child(
                    Label::new(cx.global::<Remote>().status().cloned().unwrap_or_default())
                        .text_xs()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    Button::new("remote-pair")
                        .outline()
                        .label(t!("settings-remote-pair"))
                        .disabled(options.is_disabled() || cx.global::<Remote>().busy())
                        .on_click(|_, _, cx: &mut App| remote::pair_with_host(cx)),
                )
                .into_any_element()
        }));

    for host in state.hosts() {
        group = group.item(host_item(host.clone()));
    }

    group
}

/// Forgetting drops the pairing keys, and getting them back takes a new
/// code from the host, so a stray click must not do it on its own.
fn confirm_forget(host: &PairedHost, window: &mut Window, cx: &mut App) {
    let id = host.id.clone();
    let title = t!("settings-remote-forget-title", name = host.name.as_str());

    window.open_dialog(cx, move |dialog, _, _| {
        let forget_id = id.clone();

        dialog
            .title(title.clone())
            .child(t!("settings-remote-forget-message"))
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new().child(
                            Button::new("remote-forget-cancel")
                                .min_w(DIALOG_BUTTON_MIN_WIDTH)
                                .label(t!("settings-common-cancel")),
                        ),
                    )
                    .child(
                        Button::new("remote-forget-confirm")
                            .min_w(DIALOG_BUTTON_MIN_WIDTH)
                            .danger()
                            .label(t!("settings-remote-forget"))
                            .on_click(move |_, window, cx: &mut App| {
                                window.close_dialog(cx);

                                remote::forget_host(&forget_id, cx);
                            }),
                    ),
            )
    });
}

fn host_item(host: PairedHost) -> SettingItem {
    SettingItem::render(move |_, _, cx| {
        let forget_host = host.clone();

        let status = match cx.global::<Remote>().host_status(&host.id) {
            Status::Idle => t!("settings-remote-status-idle"),
            Status::Connecting => t!("settings-remote-status-connecting"),
            Status::Connected => t!("settings-remote-status-connected"),
            Status::Reconnecting => t!("settings-remote-status-reconnecting"),
            Status::Refused => t!("settings-remote-status-refused"),
        };

        // A connected host's sessions are listed in the workspace sidebar,
        // where they open; the settings page only manages the pairing.
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    .child(Label::new(host.name.clone()).text_sm())
                    .child(
                        Label::new(format!("{status}    {}", host.lan_hints.join(", ")))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "remote-forget-{}",
                    host.id.as_str()
                )))
                .outline()
                .label(t!("settings-remote-forget"))
                .on_click(move |_, window, cx: &mut App| confirm_forget(&forget_host, window, cx)),
            )
            .into_any_element()
    })
}
