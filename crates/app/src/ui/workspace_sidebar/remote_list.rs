//! The workspaces and sessions of connected paired hosts, listed under the
//! local workspaces: each host heads the workspaces it offers, and each of
//! those heads the sessions its tabs show.

use app::agent_tab::AgentKind;
use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, SharedString, div, px, transparent_black};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::{ActiveTheme, IconName, Selectable, h_flex, v_flex};
use nmt_remote_core::rpc::{SessionInfo, SessionKind, WorkspaceInfo};
use rust_i18n::t;

use crate::ui::composition::{
    HoverActionLayout, HoverActionVisibility, hover_action, sidebar_selection, toolbar_button,
};
use crate::ui::remote::{self, RemoteWorkspace};
use crate::ui::tab_bar::menu::tab_icon;
use crate::ui::workspace_sidebar::list::{
    WORKSPACE_NAME_TEXT, WORKSPACE_PATH_TEXT, tail_preserving_path, workspace_row_button,
};
use crate::ui::workspace_sidebar::{SIDEBAR_ROW_GUTTER, WORKSPACE_NAME_INSET};
use crate::ui::{AppWindow, modern_dropdown};
use crate::workspace::workspace_display_label;

/// A session row's text, set like a vertical tab row so the two lists read
/// alike.
const SESSION_TEXT: f32 = 12.5;

/// The heading of the remote section and each host's blocks under it, in the
/// list's own rhythm: every block is spaced by the list gap, like the local
/// workspaces above.
pub(super) fn remote_workspace_blocks(
    heading: AnyElement,
    remote: &[RemoteWorkspace],
    width: f32,
    cx: &mut Context<AppWindow>,
) -> Vec<AnyElement> {
    let mut blocks = vec![heading];

    for (index, host) in remote.iter().enumerate() {
        blocks.push(host_row(index, host, cx));

        let workspaces: &[WorkspaceInfo] = host
            .offers
            .as_ref()
            .map_or(&[], |offers| offers.workspaces.as_slice());

        // Older hosts offer the raw name; sessions carry the display label.
        let labels: Vec<String> = workspaces
            .iter()
            .map(|workspace| workspace_display_label(&workspace.name, &workspace.path))
            .collect();

        // A host that sends workspace ids is matched by them, so workspaces
        // sharing a name keep their own sessions. Older hosts send none,
        // and their sessions fall back to the workspace's name.
        let in_workspace = |session: &SessionInfo, workspace: &WorkspaceInfo, label: &str| {
            session
                .workspace
                .as_ref()
                .is_some_and(|held| match &workspace.id {
                    Some(id) => held.id == *id,
                    None => held.name == label,
                })
        };

        for (slot, (workspace, label)) in workspaces.iter().zip(&labels).enumerate() {
            let rows = host
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, session)| in_workspace(session, workspace, label))
                .map(|(row, session)| session_row(index, row, host, session, cx))
                .collect::<Vec<_>>();

            blocks.push(
                v_flex()
                    .w_full()
                    .child(workspace_row(
                        index, slot, host, workspace, label, width, cx,
                    ))
                    .children(rows)
                    .into_any_element(),
            );
        }

        // Sessions no offered workspace holds: tabs of a host without
        // workspaces, or of a workspace the host no longer offers.
        let loose = host
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| {
                !workspaces
                    .iter()
                    .zip(&labels)
                    .any(|(workspace, label)| in_workspace(session, workspace, label))
            })
            .map(|(row, session)| session_row(index, row, host, session, cx))
            .collect::<Vec<_>>();

        if !loose.is_empty() {
            blocks.push(v_flex().w_full().children(loose).into_any_element());
        }
    }

    blocks
}

/// A host heading its workspaces, with the control that starts a terminal
/// there. A remote terminal opens in the host's default directory, so it
/// needs no workspace to start from.
fn host_row(index: usize, host: &RemoteWorkspace, cx: &mut Context<AppWindow>) -> AnyElement {
    let terminal_host = host.id.clone();

    let new_terminal = hover_action(
        ("remote-new-terminal", index),
        t!("settings-remote-new-terminal"),
        HoverActionLayout::Bare,
        HoverActionVisibility::OnGroupHover("remote-host".into()),
        toolbar_button(("remote-new-terminal-button", index))
            .icon(IconName::Plus)
            .accessibility_label(t!("settings-remote-new-terminal"))
            .on_click(move |_, window, cx| remote::open_terminal(&terminal_host, window, cx)),
    );

    let name: SharedString = host.name.clone().into();

    h_flex()
        .id(("remote-host", index))
        .group("remote-host")
        .w_full()
        .min_h(px(24.))
        .pl(px(WORKSPACE_NAME_INSET))
        .pr(px(SIDEBAR_ROW_GUTTER))
        .gap_1p5()
        .items_center()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(WORKSPACE_NAME_TEXT))
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().sidebar_foreground.opacity(0.7))
                .child(name),
        )
        .child(new_terminal)
        .into_any_element()
}

/// A workspace the host offers, set like a local workspace row: its name
/// with the path trailing it, and a hover control that starts a terminal or
/// one of the host's agents there.
fn workspace_row(
    index: usize,
    slot: usize,
    host: &RemoteWorkspace,
    workspace: &WorkspaceInfo,
    label: &str,
    width: f32,
    cx: &mut Context<AppWindow>,
) -> AnyElement {
    let id = index * 1000 + slot;
    let menu_host = host.id.clone();
    let path = workspace.path.clone();

    let agents = host
        .offers
        .as_ref()
        .map(|offers| offers.agents.clone())
        .unwrap_or_default();

    let new_tab = hover_action(
        ("remote-workspace-new-tab", id),
        t!("sidebar-tab-new"),
        HoverActionLayout::Bare,
        HoverActionVisibility::OnGroupHover("remote-ws-item".into()),
        modern_dropdown(
            toolbar_button(("remote-workspace-new-tab-button", id))
                .icon(IconName::Plus)
                .accessibility_label(t!("sidebar-tab-new")),
            move |mut menu, _, _| {
                let terminal_host = menu_host.clone();

                menu = menu
                    .item(t!("settings-remote-new-terminal"), move |window, cx| {
                        remote::open_terminal(&terminal_host, window, cx)
                    })
                    .icon(tab_icon(None, false));

                if !agents.is_empty() {
                    menu = menu.separator();
                }

                for agent in &agents {
                    let agent_host = menu_host.clone();
                    let chosen = agent.clone();
                    let path = path.clone();

                    menu = menu
                        .item(agent.name.clone(), move |window, cx| {
                            remote::open_agent(&agent_host, &chosen, path.clone(), window, cx)
                        })
                        .icon(tab_icon(AgentKind::from_id(&agent.harness), false));
                }

                menu
            },
        ),
    );

    // The same budget a local row gives its path, so a host's paths keep
    // their leaf directory in view the way local ones do.
    let path_budget = (width - 80.0 - 8.0 * label.chars().count() as f32) / 7.0;

    let display_path: SharedString = tail_preserving_path(
        &workspace.path,
        (path_budget.floor().max(0.0) as usize).clamp(8, 64),
    )
    .into();

    let label: SharedString = label.to_owned().into();

    let name = h_flex()
        .w_full()
        .gap_1p5()
        .items_baseline()
        .child(
            div()
                .min_w_0()
                .text_left()
                .text_size(px(WORKSPACE_NAME_TEXT))
                .truncate()
                .child(label.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_left()
                .text_size(px(WORKSPACE_PATH_TEXT))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(cx.theme().sidebar_foreground.opacity(0.4))
                .child(display_path),
        );

    workspace_row_button(("remote-workspace", id), cx)
        .accessibility_label(label)
        .group("remote-ws-item")
        .child(
            h_flex()
                .w_full()
                .gap_1p5()
                .items_center()
                .child(div().flex_1().min_w_0().overflow_hidden().child(name))
                .child(new_tab),
        )
        .into_any_element()
}

/// A session on a host: opening it follows it in a tab here, or shows the
/// tab already following it. The session whose tab is on screen is marked
/// the way the vertical tab list marks its tab, since that tab sits in no
/// list of its own.
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
    let selected = host.selected.as_deref() == Some(session.session.as_str());
    let selection = sidebar_selection(cx);

    let button: Button = workspace_row_button(("remote-session", index * 1000 + row), cx)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(transparent_black())
                .hover(cx.theme().sidebar_foreground.opacity(0.085))
                .active(cx.theme().sidebar_foreground.opacity(0.12)),
        )
        .selected(selected)
        // Button resolves selected colors after element styles, so the
        // sidebar-accent pair must be the selected custom variant itself.
        .when(selected, |this| {
            this.custom(
                ButtonCustomVariant::new(cx)
                    .foreground(selection.active_foreground)
                    .active(selection.active_background),
            )
        })
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
