//! The sessions of connected paired hosts, listed under the local
//! workspaces: one block per host, heading the sessions running there.

use app::agent_tab::AgentKind;
use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, SharedString, div, px, transparent_black};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::{ActiveTheme, IconName, h_flex, v_flex};
use nmt_remote_core::rpc::{SessionInfo, SessionKind};
use rust_i18n::t;

use crate::ui::composition::{
    HoverActionLayout, HoverActionVisibility, hover_action, toolbar_button,
};
use crate::ui::remote::{self, RemoteWorkspace};
use crate::ui::tab_bar::menu::tab_icon;
use crate::ui::workspace_sidebar::list::workspace_row_button;
use crate::ui::workspace_sidebar::{SIDEBAR_ROW_GUTTER, WORKSPACE_NAME_INSET};
use crate::ui::{AppWindow, UI_RADIUS, modern_dropdown};

/// A session row's text, set like a vertical tab row so the two lists read
/// alike.
const SESSION_TEXT: f32 = 12.5;

/// The heading of the remote section and each host's block under it, in the
/// list's own rhythm: blocks are spaced by the list gap.
pub(super) fn remote_workspace_blocks(
    heading: AnyElement,
    remote: &[RemoteWorkspace],
    cx: &mut Context<AppWindow>,
) -> Vec<AnyElement> {
    let mut blocks = vec![heading];

    blocks.extend(remote.iter().enumerate().map(|(index, host)| {
        v_flex()
            .w_full()
            .child(host_row(index, host, cx))
            .children(
                host.sessions
                    .iter()
                    .enumerate()
                    .map(|(row, session)| session_row(index, row, host, session, cx)),
            )
            .into_any_element()
    }));

    blocks
}

/// A host heading its sessions, with the control that starts a terminal or
/// an agent there.
fn host_row(index: usize, host: &RemoteWorkspace, cx: &mut Context<AppWindow>) -> AnyElement {
    let menu_host = host.id.clone();
    let offers = host.offers.clone();

    let new_session = hover_action(
        ("remote-new-session", index),
        t!("sidebar-tab-new"),
        HoverActionLayout::Bare,
        HoverActionVisibility::OnGroupHover("remote-host".into()),
        modern_dropdown(
            toolbar_button(("remote-new-session-button", index))
                .icon(IconName::Plus)
                .accessibility_label(t!("sidebar-tab-new")),
            move |mut menu, _, _| {
                let terminal_host = menu_host.clone();

                menu = menu
                    .item(t!("settings-remote-new-terminal"), move |window, cx| {
                        remote::open_terminal(&terminal_host, window, cx)
                    })
                    .icon(tab_icon(None, false));

                for workspace in offers.iter().flat_map(|offers| &offers.workspaces) {
                    for agent in offers.iter().flat_map(|offers| &offers.agents) {
                        let agent_host = menu_host.clone();
                        let chosen = agent.clone();
                        let path = workspace.path.clone();

                        menu = menu
                            .item(
                                format!("{} · {}", agent.name, workspace.name),
                                move |window, cx| {
                                    remote::open_agent(
                                        &agent_host,
                                        &chosen,
                                        path.clone(),
                                        window,
                                        cx,
                                    )
                                },
                            )
                            .icon(tab_icon(AgentKind::from_id(&agent.harness), false));
                    }
                }

                menu
            },
        ),
    );

    let name: SharedString = host.name.clone().into();

    div()
        .id(("remote-host", index))
        .w_full()
        .group("remote-host")
        .child(
            h_flex()
                .w_full()
                .min_h(px(28.))
                .pl(px(WORKSPACE_NAME_INSET))
                .pr(px(SIDEBAR_ROW_GUTTER))
                .gap_1p5()
                .items_center()
                .rounded(UI_RADIUS)
                .bg(cx.theme().sidebar_foreground.opacity(0.045))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(name),
                )
                .child(new_session),
        )
        .into_any_element()
}

/// A session on a host: opening it follows it in a tab here, or shows the
/// tab already following it.
fn session_row(
    index: usize,
    row: usize,
    host: &RemoteWorkspace,
    session: &SessionInfo,
    cx: &mut Context<AppWindow>,
) -> AnyElement {
    let icon = match session.kind {
        SessionKind::Agent => tab_icon(
            session.harness.as_deref().and_then(AgentKind::from_id),
            false,
        ),
        SessionKind::Terminal | SessionKind::Unknown => tab_icon(None, false),
    };

    let host_id = host.id.clone();
    let opened = session.clone();
    let title: SharedString = session.title.clone().into();

    let button: Button = workspace_row_button(("remote-session", index * 1000 + row), cx)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(transparent_black())
                .hover(cx.theme().sidebar_foreground.opacity(0.085))
                .active(cx.theme().sidebar_foreground.opacity(0.12)),
        )
        .accessibility_label(title.clone())
        .child(
            h_flex().w_full().gap_2().items_center().child(icon).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_left()
                    .text_size(px(SESSION_TEXT))
                    .child(title),
            ),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.open_remote_session(&host_id, &opened, window, cx)
        }));

    button.into_any_element()
}
