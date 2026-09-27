use gpui::{App, IntoElement as _, ParentElement as _, SharedString, Styled as _};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::label::Label;
use gpui_component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_component::{ActiveTheme as _, Disableable as _, h_flex, v_flex};
use nmt_remote::connection::Status;
use nmt_remote::store::{PairedDevice, PairedHost, now_ms};
use nmt_remote_core::identity::DeviceId;
use nmt_remote_core::rpc::{Origin, SessionInfo};
use rust_i18n::t;

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
        );

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

/// A terminal a paired device started here. It runs with nobody at the host
/// watching, so the host user can end it.
fn remote_created_item(session: SessionInfo) -> SettingItem {
    SettingItem::render(move |_, _, cx| {
        let id = session.session.clone();

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
                Button::new(SharedString::from(format!("remote-close-{id}")))
                    .outline()
                    .label(t!("settings-remote-close-session"))
                    .on_click(move |_, _, cx: &mut App| remote::close_remote_created(&id, cx)),
            )
            .into_any_element()
    })
}

fn pairing_item(state: &Remote) -> SettingItem {
    let pairing = state.pairing();

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
                    Button::new("remote-cancel-pairing")
                        .outline()
                        .label(t!("settings-remote-cancel"))
                        .on_click(|_, _, cx: &mut App| remote::cancel_pairing(cx)),
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

        for session in state.host_sessions(&host.id).unwrap_or_default() {
            group = group.item(host_session_item(host.id.clone(), session.clone()));
        }
    }

    group
}

/// A session running on a paired host, which this computer can view.
fn host_session_item(host: DeviceId, session: SessionInfo) -> SettingItem {
    SettingItem::render(move |_, _, cx| {
        let id = session.session.clone();
        let host = host.clone();

        let origin = match session.origin {
            Origin::Tab => t!("settings-remote-origin-tab"),
            _ => t!("settings-remote-started-remotely"),
        };

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_3()
            .pl_4()
            .child(
                v_flex()
                    .flex_1()
                    .child(Label::new(session.title.clone()).text_sm())
                    .child(
                        Label::new(format!("{origin}    {}x{}", session.cols, session.rows))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "remote-view-{}-{id}",
                    host.as_str()
                )))
                .outline()
                .label(t!("settings-remote-open-session"))
                .on_click(move |_, window, cx: &mut App| {
                    remote::open_session(&host, &id, window, cx)
                }),
            )
            .into_any_element()
    })
}

fn host_item(host: PairedHost) -> SettingItem {
    SettingItem::render(move |_, _, cx| {
        let open_id = host.id.clone();
        let forget_id = host.id.clone();
        let list_id = host.id.clone();

        let status = match cx.global::<Remote>().host_status(&host.id) {
            Status::Idle => t!("settings-remote-status-idle"),
            Status::Connecting => t!("settings-remote-status-connecting"),
            Status::Connected => t!("settings-remote-status-connected"),
            Status::Reconnecting => t!("settings-remote-status-reconnecting"),
            Status::Refused => t!("settings-remote-status-refused"),
        };

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
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(format!(
                            "remote-list-{}",
                            host.id.as_str()
                        )))
                        .outline()
                        .label(t!("settings-remote-sessions"))
                        .on_click(move |_, _, cx: &mut App| remote::refresh_sessions(&list_id, cx)),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "remote-forget-{}",
                            host.id.as_str()
                        )))
                        .outline()
                        .label(t!("settings-remote-forget"))
                        .on_click(move |_, _, cx: &mut App| remote::forget_host(&forget_id, cx)),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "remote-open-{}",
                            host.id.as_str()
                        )))
                        .primary()
                        .label(t!("settings-remote-new-terminal"))
                        .disabled(cx.global::<Remote>().busy())
                        .on_click(move |_, window, cx: &mut App| {
                            remote::open_terminal(&open_id, window, cx)
                        }),
                    ),
            )
            .into_any_element()
    })
}
