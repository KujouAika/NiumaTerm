//! The workspaces and sessions of connected paired hosts, listed under the
//! local workspaces: each host heads the workspaces it offers, and each of
//! those heads the sessions its tabs show.

use std::collections::HashMap;

use app::agent_tab::AgentKind;
use gpui::prelude::*;
use gpui::{AnyElement, Context, ElementId, FontWeight, SharedString, Window, div, px};
use gpui_component::button::{ButtonCustomVariant, ButtonVariants as _};
use gpui_component::modern_menu::ModernMenuExt as _;
use gpui_component::{ActiveTheme, IconName, IconNamed, Selectable as _, h_flex, v_flex};
use nmt_config::local_state::TabFold;
use nmt_remote_core::rpc::{SessionInfo, SessionKind, WorkspaceInfo};
use rust_i18n::t;

use crate::ui::composition::{
    HoverActionLayout, HoverActionVisibility, hover_action, sidebar_selection, toolbar_button,
};
use crate::ui::platform_style::{Host, PlatformStyle as _};
use crate::ui::remote::{self, RemoteWorkspace};
use crate::ui::shell::{InlineRename, InlineRenameSession, InlineRenameStyle, pending_tab_icon};
use crate::ui::tab_bar::menu::tab_icon;
use crate::ui::tab_bar::{fold_row, tab_row, tab_row_icon};
use crate::ui::workspace_sidebar::list::{
    Disclosure, WORKSPACE_NAME_TEXT, WORKSPACE_PATH_TEXT, selection_bar, tail_preserving_path,
    workspace_row_button,
};
use crate::ui::workspace_sidebar::{SIDEBAR_ROW_GUTTER, WORKSPACE_NAME_INSET};
use crate::ui::{AppWindow, modern_dropdown};
use crate::workspace::workspace_display_label;

/// The heading of the remote section and each host's blocks under it, in the
/// list's own rhythm: every block is spaced by the list gap, like the local
/// workspaces above.
pub(super) fn remote_workspace_blocks(
    heading: AnyElement,
    remote: &[RemoteWorkspace],
    folds: &HashMap<(String, String), TabFold>,
    renames: &InlineRenameSession,
    width: f32,
    window: &mut Window,
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

        // Sessions no offered workspace holds: tabs of a host without
        // workspaces, of a workspace the host no longer offers, or not yet
        // placed by the host. They sit right under the host row, since after
        // the last workspace they would read as that workspace's sessions.
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
            .map(|(row, session)| session_row(index, row, host, session, renames, cx))
            .collect::<Vec<_>>();

        if !loose.is_empty() {
            blocks.push(v_flex().w_full().children(loose).into_any_element());
        }

        for (slot, (workspace, label)) in workspaces.iter().zip(&labels).enumerate() {
            let fold_key = (
                host.id.as_str().to_owned(),
                workspace.id.clone().unwrap_or_else(|| label.clone()),
            );

            let fold = folds.get(&fold_key).copied().unwrap_or_default();

            let is_selected =
                |session: &SessionInfo| host.selected.as_deref() == Some(session.session.as_str());

            let rows = host
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, session)| in_workspace(session, workspace, label))
                .filter_map(|(row, session)| {
                    let shown = match fold {
                        TabFold::All => true,
                        TabFold::Awake => !session.pending,
                        TabFold::Collapsed => false,
                    };

                    fold_row(
                        ElementId::Name(
                            format!(
                                "remote-session-fold:{}:{}",
                                host.id.as_str(),
                                session.session
                            )
                            .into(),
                        ),
                        shown,
                        window,
                        cx,
                        |cx| session_row(index, row, host, session, renames, cx),
                    )
                })
                .collect::<Vec<_>>();

            // With the session on screen folded away, its workspace row takes
            // over the selection it would show.
            let highlight = fold == TabFold::Collapsed
                && host
                    .sessions
                    .iter()
                    .any(|session| in_workspace(session, workspace, label) && is_selected(session));

            let row = WorkspaceRow {
                index,
                slot,
                fold,
                fold_key,
                highlight,
            };

            blocks.push(
                v_flex()
                    .w_full()
                    .child(workspace_row(
                        row, host, workspace, label, width, window, cx,
                    ))
                    .children(rows)
                    .into_any_element(),
            );
        }
    }

    blocks
}

/// A host heading its workspaces. New tabs start from a workspace row, so
/// what they open is tied to a place on the host.
fn host_row(index: usize, host: &RemoteWorkspace, cx: &mut Context<AppWindow>) -> AnyElement {
    let name: SharedString = host.name.clone().into();

    h_flex()
        .id(("remote-host", index))
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
        .into_any_element()
}

/// A workspace the host offers, set like a local workspace row: its name
/// with the path trailing it, and a hover control that starts a terminal or
/// one of the host's agents there.
fn workspace_row(
    row: WorkspaceRow,
    host: &RemoteWorkspace,
    workspace: &WorkspaceInfo,
    label: &str,
    width: f32,
    window: &mut Window,
    cx: &mut Context<AppWindow>,
) -> AnyElement {
    let WorkspaceRow {
        index,
        slot,
        fold,
        fold_key,
        highlight,
    } = row;

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
                .font_weight(FontWeight::NORMAL)
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

    let selection = sidebar_selection(cx);

    // Keyed by host and workspace rather than list position, so a host
    // connecting above this one does not hand this row another's reveal.
    let disclosure = Disclosure::new(
        ElementId::Name(format!("remote-disclosure:{}:{}", fold_key.0, fold_key.1).into()),
        fold,
        window,
        cx,
    );

    let item = workspace_row_button(("remote-workspace", id), cx)
        .accessibility_label(label)
        .selected(highlight)
        // Button resolves selected colors after element styles, so the
        // sidebar-accent pair must be the selected custom variant itself.
        .when(highlight, |this| {
            this.custom(
                ButtonCustomVariant::new(cx)
                    .foreground(selection.active_foreground)
                    .active(selection.active_background),
            )
        })
        .group("remote-ws-item")
        // The name starts on the session rows' icon column until the
        // disclosure mark makes it slide over.
        .pl(px(SIDEBAR_ROW_GUTTER))
        .on_hover(disclosure.on_hover())
        .child(
            h_flex()
                .relative()
                .w_full()
                .gap_1p5()
                .items_center()
                .child(disclosure.mark(cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .ml(disclosure.name_offset())
                        .child(name),
                )
                .child(new_tab),
        )
        // Opening a session is done from its own row, so the workspace row
        // is left to fold them.
        .on_click(cx.listener(move |this, _, _, cx| {
            let (host, workspace) = fold_key.clone();

            this.sidebar.cycle_remote_fold(host, workspace);

            cx.notify();
        }));

    div()
        .w_full()
        .relative()
        .child(item)
        // After the row itself, because the row's selected fill would
        // otherwise paint over the bar's lane.
        .children((highlight && Host::SIDEBAR_SELECTION_MARK).then(|| selection_bar(cx)))
        .into_any_element()
}

/// Where a host workspace's row sits in the list and how its sessions fold.
struct WorkspaceRow {
    /// The host's position among the connected hosts.
    index: usize,

    /// The workspace's position among those its host offers.
    slot: usize,

    fold: TabFold,

    /// The host and workspace the sidebar keeps the fold under.
    fold_key: (String, String),

    /// The session on screen is in this workspace and folded away.
    highlight: bool,
}

/// A session on a host, drawn as a local tab row: opening it follows it in a
/// tab here, or shows the tab already following it. The session whose tab is
/// on screen is marked the way the local list marks its tab, since that tab
/// sits in no list of its own.
///
/// A session this computer follows acts like a local tab: it is renamed and
/// closed from its menu, closed from its hover control, and can also be
/// disconnected from, which closes only what follows it here. A session it
/// does not follow offers only connecting to it: renaming or ending something
/// on another computer is left to whoever has it open.
fn session_row(
    index: usize,
    row: usize,
    host: &RemoteWorkspace,
    session: &SessionInfo,
    renames: &InlineRenameSession,
    cx: &mut Context<AppWindow>,
) -> AnyElement {
    let icon = match session.kind {
        SessionKind::Agent => tab_icon(
            session.harness.as_deref().and_then(AgentKind::from_id),
            false,
        ),
        SessionKind::Terminal | SessionKind::Unknown => tab_icon(None, false),
    };

    let key = index * 1000 + row;
    let host_id = host.id.clone();
    let opened = session.clone();
    let title: SharedString = session.title.clone().into();
    let selected = host.selected.as_deref() == Some(session.session.as_str());
    let followed = host.followed.contains(&session.session);

    let label: AnyElement = match renames.remote_input(&host.id, &session.session).cloned() {
        Some(input) => {
            let rename_shell = cx.entity();

            InlineRename::new(
                ("remote-session-rename", key),
                title.clone(),
                input,
                InlineRenameStyle::SidebarTab,
                move |window, cx| {
                    rename_shell
                        .update(cx, |this, cx| this.finish_remote_rename(false, window, cx));
                },
            )
            .into_any_element()
        }
        None => div()
            .flex_1()
            .overflow_hidden()
            .truncate()
            .child(title.clone())
            .into_any_element(),
    };

    let close = {
        let host_id = host.id.clone();
        let host_name = host.name.clone();
        let closed = session.clone();

        hover_action(
            ("remote-session-close", key),
            t!("tabbar-menu-close"),
            HoverActionLayout::Inline,
            HoverActionVisibility::OnGroupHover("remote-session".into()),
            "\u{00d7}",
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();

            this.request_close_remote_session(
                host_id.clone(),
                host_name.clone(),
                closed.clone(),
                window,
                cx,
            );
        }))
    };

    // A session still asleep on its host wears the faded row and moon a
    // local tab waiting to start does, so both lists tell running tabs from
    // sleeping ones the same way.
    let glyph = match session.pending {
        true => pending_tab_icon(("remote-session-pending", key)).into_any_element(),
        false => icon.into_any_element(),
    };

    let row = tab_row(("remote-session", key), title, selected, cx)
        .group("remote-session")
        .when(session.pending, |this| this.opacity(0.6))
        .child(tab_row_icon(glyph))
        .child(label)
        .when(followed, |row| row.child(close))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.open_remote_session(&host_id, &opened, window, cx)
        }));

    let menu_shell = cx.entity();
    let menu_host = host.id.clone();
    let menu_host_name = host.name.clone();
    let menu_session = session.clone();

    div()
        .id(("remote-session-menu", key))
        .w_full()
        .modern_context_menu(move |menu, _, _| {
            if !followed {
                let connect_shell = menu_shell.clone();
                let connected = (menu_host.clone(), menu_session.clone());

                return menu
                    .item(t!("remote-session-connect"), move |window, cx| {
                        let (host, session) = connected.clone();

                        connect_shell.update(cx, |this, cx| {
                            this.open_remote_session(&host, &session, window, cx)
                        });
                    })
                    .icon(PlugIcon);
            }

            let rename_shell = menu_shell.clone();
            let disconnect_shell = menu_shell.clone();
            let close_shell = menu_shell.clone();

            let renamed = (menu_host.clone(), menu_session.clone());

            let disconnected = (
                menu_host.clone(),
                menu_host_name.clone(),
                menu_session.session.clone(),
            );

            let closed = (
                menu_host.clone(),
                menu_host_name.clone(),
                menu_session.clone(),
            );

            menu.item(t!("tabbar-menu-rename"), move |window, cx| {
                let (host, session) = renamed.clone();

                rename_shell.update(cx, |this, cx| {
                    this.start_remote_rename(host, session, window, cx)
                });
            })
            .icon(IconName::PenLine)
            .item(t!("remote-session-disconnect"), move |window, cx| {
                let (host, name, session) = disconnected.clone();

                disconnect_shell.update(cx, |this, cx| {
                    this.request_disconnect_session(host, name, session, window, cx)
                });
            })
            .icon(UnplugIcon)
            .item(t!("tabbar-menu-close"), move |window, cx| {
                let (host, name, session) = closed.clone();

                close_shell.update(cx, |this, cx| {
                    this.request_close_remote_session(host, name, session, window, cx)
                });
            })
            .icon(IconName::Close)
        })
        .child(row)
        .into_any_element()
}

/// Plug pulled from its socket (`assets/icons/unplug.svg`), for leaving a
/// session that keeps running on its host.
struct UnplugIcon;

impl IconNamed for UnplugIcon {
    fn path(self) -> SharedString {
        "icons/unplug.svg".into()
    }
}

/// Plug ready to go in (`assets/icons/plug.svg`), for opening a session that
/// runs on its host.
struct PlugIcon;

impl IconNamed for PlugIcon {
    fn path(self) -> SharedString {
        "icons/plug.svg".into()
    }
}
