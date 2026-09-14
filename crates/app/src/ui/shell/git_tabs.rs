use gpui::{AppContext as _, Context, Window};
use rust_i18n::t;

use crate::tabs::TabId;
use crate::ui::git_sidebar::GitSidebar;
use crate::ui::shell::Shell;
use crate::ui::shell::actions::{QuoteGitLine, ReturnFromGit, ToggleGitSidebar};
use crate::ui::shell::tab_surface::{GitTab, TabSurface};

impl Shell {
    pub(super) fn on_toggle_git_sidebar(
        &mut self,
        _: &ToggleGitSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_settings_workspace();

        let tabs = self.workspaces.active_tabs();
        let return_to = (!tabs.active().is_git()).then_some(tabs.active_id());
        let existing = tabs.tabs().iter().position(|tab| tab.surface().is_git());

        if let Some(index) = existing {
            self.workspaces.active_tabs_mut().activate(index);
            self.ensure_active_tab_live(window, cx);

            if let TabSurface::Git(tab) = self.workspaces.active_tabs_mut().active_mut()
                && return_to.is_some()
            {
                tab.return_to = return_to;
            }
        } else {
            let cwd = self
                .try_active_pane()
                .and_then(|pane| pane.read(cx).tab_state().cwd)
                .or_else(|| {
                    self.active_agent()
                        .and_then(|pane| pane.read(cx).working_directory())
                })
                .or_else(|| {
                    self.workspaces
                        .active_roots()
                        .map(|roots| roots.primary().to_string())
                })
                .unwrap_or_default();

            let view = cx.new(|cx| GitSidebar::new(cwd, window, cx));
            let id = TabId(Self::alloc_id(&mut self.next_id));

            self.workspaces.active_tabs_mut().new_tab(
                TabSurface::Git(GitTab { view, return_to }),
                id,
                t!("git-tab-title").into_owned(),
            );
        }

        self.on_active_tab_changed(window, cx);
        self.focus_active(window, cx);
        self.sync_session_memory(cx);

        cx.notify();
    }

    pub(super) fn sync_git_tab_visibility(&self, cx: &mut Context<Self>) {
        let active = self.workspaces.active_tabs().active_id();

        let can_quote = self
            .workspaces
            .active_tabs()
            .tabs()
            .iter()
            .any(|tab| tab.surface().is_agent());

        let views: Vec<_> = self
            .workspaces
            .all_tabs()
            .flat_map(|tabs| tabs.tabs())
            .filter_map(|tab| {
                tab.surface()
                    .git()
                    .map(|git| (tab.id() == active, git.view.clone()))
            })
            .collect();

        for (visible, view) in views {
            view.update(cx, |view, cx| {
                view.set_quote_available(visible && can_quote);
                view.set_visible(visible, cx);
            });
        }
    }

    pub(super) fn on_return_from_git(
        &mut self,
        _: &ReturnFromGit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(git) = self.workspaces.active_tabs().active().git() else {
            return;
        };

        let preferred = git.return_to;
        let tabs = self.workspaces.active_tabs();

        let index = preferred
            .and_then(|id| tabs.tabs().iter().position(|tab| tab.id() == id))
            .or_else(|| tabs.tabs().iter().position(|tab| !tab.surface().is_git()));

        if let Some(index) = index {
            self.workspaces.active_tabs_mut().activate(index);
            self.on_active_tab_changed(window, cx);
            self.focus_active(window, cx);
            self.sync_session_memory(cx);

            cx.notify();
        }
    }

    pub(super) fn on_quote_git_line(
        &mut self,
        _: &QuoteGitLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(git) = self.workspaces.active_tabs().active().git() else {
            return;
        };

        let Some(reference) = git.view.read(cx).selected_reference(cx) else {
            return;
        };

        let preferred = git.return_to;
        let tabs = self.workspaces.active_tabs();

        let index = tabs
            .tabs()
            .iter()
            .position(|tab| Some(tab.id()) == preferred && tab.surface().is_agent())
            .or_else(|| tabs.tabs().iter().position(|tab| tab.surface().is_agent()));

        let Some(index) = index else { return };

        self.workspaces.active_tabs_mut().activate(index);
        self.on_active_tab_changed(window, cx);

        if let Some(agent) = self.active_agent() {
            agent.update(cx, |pane, cx| {
                pane.append_code_reference(&reference, window, cx)
            });
        }

        self.focus_active(window, cx);
        self.sync_session_memory(cx);

        cx.notify();
    }
}
