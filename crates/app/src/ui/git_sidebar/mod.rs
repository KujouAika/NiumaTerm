mod diff_view;
mod tree;
mod view;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::Path;

use gpui::prelude::*;
use gpui::{
    App, Context, Entity, FocusHandle, Point, ScrollStrategy, UniformListScrollHandle, Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::ui::git_sidebar::diff_view::DiffView;
use crate::ui::git_sidebar::tree::TreeRow;
use crate::ui::git_status::{GitStatusModel, fetch_file_diff};

enum ChangeDirection {
    Previous,
    Next,
}

/// One workspace's review state survives switches to conversations and other workspaces.
pub(crate) struct GitSidebar {
    model: Entity<GitStatusModel>,
    cwd: String,
    selected: Option<String>,
    diff: DiffView,
    diff_seq: u64,
    seen_snapshot_seq: u64,
    files_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    focus: FocusHandle,
    filter: Entity<InputState>,
    filter_open: bool,
    collapsed: HashSet<String>,
    rows: Vec<TreeRow>,
    selected_line: Option<usize>,
    loading: bool,
    visible: bool,
    can_quote: bool,
    files_width: f32,
    files_open: bool,
    wrap: bool,
    diff_width: f32,
}

impl GitSidebar {
    pub(crate) fn new(cwd: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = cx.new(GitStatusModel::for_tab);

        model.update(cx, |model, cx| model.set_target_cwd(Some(cwd.clone()), cx));

        let filter = cx.new(|cx| InputState::new(window, cx));

        cx.subscribe(&filter, |this: &mut Self, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.update_tree(cx);

                cx.notify();
            }
        })
        .detach();

        cx.observe(&model, |this: &mut Self, model, cx| {
            let seq = model.read(cx).snapshot_seq;

            if seq != this.seen_snapshot_seq {
                this.seen_snapshot_seq = seq;
                this.on_snapshot_changed(cx);
            }

            cx.notify();
        })
        .detach();

        Self {
            model,
            cwd,
            selected: None,
            diff: DiffView::default(),
            diff_seq: 0,
            seen_snapshot_seq: 0,
            files_scroll: UniformListScrollHandle::default(),
            diff_scroll: UniformListScrollHandle::default(),
            focus: cx.focus_handle(),
            filter,
            filter_open: false,
            collapsed: HashSet::new(),
            rows: Vec::new(),
            selected_line: None,
            loading: false,
            visible: false,
            can_quote: false,
            files_width: 226.0,
            files_open: true,
            wrap: false,
            diff_width: 700.0,
        }
    }

    pub(crate) fn cwd(&self) -> &str {
        &self.cwd
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub(crate) fn set_quote_available(&mut self, available: bool) {
        self.can_quote = available;
    }

    pub(crate) fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }

        self.visible = visible;

        self.model.update(cx, |model, cx| {
            model.sidebar_open = visible;

            if visible {
                model.refresh(cx);
            }
        });
    }

    fn update_tree(&mut self, cx: &Context<Self>) {
        self.rows = self
            .model
            .read(cx)
            .snapshot
            .as_ref()
            .map_or_else(Vec::new, |snapshot| {
                tree::rows(
                    &snapshot.files,
                    &self.collapsed,
                    self.filter.read(cx).value().as_ref(),
                )
            });
    }

    fn on_snapshot_changed(&mut self, cx: &mut Context<Self>) {
        self.update_tree(cx);

        let files = self
            .model
            .read(cx)
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.files);

        let next = self
            .selected
            .as_ref()
            .filter(|path| files.is_some_and(|files| files.iter().any(|file| &file.path == *path)))
            .cloned()
            .or_else(|| {
                files
                    .and_then(|files| files.first())
                    .map(|file| file.path.clone())
            });

        if next != self.selected {
            self.selected = next;
            self.diff = DiffView::default();
            self.selected_line = None;
            self.diff_seq += 1;
            self.reset_diff_scroll();
        }

        self.fetch_diff(cx);
    }

    fn reset_diff_scroll(&self) {
        self.diff_scroll
            .0
            .borrow()
            .base_handle
            .set_offset(Point::default());

        self.diff_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn select_adjacent_file(&mut self, direction: ChangeDirection, cx: &mut Context<Self>) {
        let files: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.file.is_some())
            .collect();

        if files.is_empty() {
            return;
        }

        let current = files
            .iter()
            .position(|(_, row)| Some(&row.path) == self.selected.as_ref());

        let next = match (current, direction) {
            (Some(index), ChangeDirection::Next) => (index + 1).min(files.len() - 1),
            (Some(index), ChangeDirection::Previous) => index.saturating_sub(1),
            (None, _) => 0,
        };

        let (row, file) = files[next];
        let path = file.path.clone();

        self.files_scroll
            .scroll_to_item(row, ScrollStrategy::Center);

        self.select(path, cx);
    }

    fn select(&mut self, path: String, cx: &mut Context<Self>) {
        self.files_open = true;

        if self.selected.as_deref() == Some(&path) {
            return;
        }

        self.selected = Some(path);
        self.diff = DiffView::default();
        self.selected_line = None;
        self.reset_diff_scroll();
        self.fetch_diff(cx);

        cx.notify();
    }

    fn fetch_diff(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(snapshot)) =
            (self.selected.clone(), self.model.read(cx).snapshot.as_ref())
        else {
            self.loading = false;

            return;
        };

        let root = snapshot.repo_root.clone();

        let untracked = snapshot
            .files
            .iter()
            .any(|file| file.path == path && file.status == "??");

        self.diff_seq += 1;

        let seq = self.diff_seq;

        self.loading = self.diff.is_empty();

        let fetch = cx.background_executor().spawn(async move {
            let lines = fetch_file_diff(&root, &path, untracked);

            DiffView::prepare(lines, &path)
        });

        cx.spawn(async move |this, cx| {
            let prepared = fetch.await;

            this.update(cx, |this, cx| {
                if this.diff_seq == seq {
                    this.diff.update(prepared);

                    this.selected_line =
                        this.selected_line.filter(|index| *index < this.diff.len());

                    this.loading = false;

                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn selected_reference(&self, cx: &App) -> Option<String> {
        let root = &self.model.read(cx).snapshot.as_ref()?.repo_root;
        let path = self.selected.as_ref()?;
        let line = self.diff.line(self.selected_line?)?;
        let number = line.new_line.or(line.old_line)?;

        let side = if line.new_line.is_some() {
            "new"
        } else {
            "old"
        };

        Some(format!(
            "{}:{number} ({side})\n    {}",
            Path::new(root).join(path).display(),
            line.text
        ))
    }

    fn jump_change(&mut self, direction: ChangeDirection, cx: &mut Context<Self>) {
        let current = self.diff.current_change(&self.diff_scroll);

        let index = match direction {
            ChangeDirection::Next => (current + 1).min(self.diff.change_count().saturating_sub(1)),
            ChangeDirection::Previous => current.saturating_sub(1),
        };

        self.diff.jump_change(index, &self.diff_scroll);

        cx.notify();
    }
}
