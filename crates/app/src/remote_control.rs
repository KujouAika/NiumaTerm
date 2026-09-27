//! One side controls a shared session at a time. While a paired device has
//! a host tab, the host covers it with a frosted sheet naming the device;
//! while the host has taken it back, the device's tab says so. Both sheets
//! offer what the person can do next.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{AnyElement, App, ClickEvent, FocusHandle, SharedString, Window, div, px};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};
use tokio::sync::watch;

/// A host tab's side of sharing: who controls it from another computer,
/// and taking it back.
pub struct HostControl {
    /// Names of the devices controlling the session now.
    pub controllers: Arc<dyn Fn() -> Vec<String> + Send + Sync>,

    /// Bumped when the controllers change.
    pub changes: watch::Receiver<u64>,

    /// End every device's view, keeping the session running here.
    pub take_back: Arc<dyn Fn() + Send + Sync>,
}

/// Closes the tab showing a pane, as the close shortcut would. The shell
/// owns tabs, so it hands each pane this for the sheets' close buttons.
pub type CloseTab = Arc<dyn Fn(&mut Window, &mut App)>;

type Listener = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A frosted sheet over a pane, with a message and its buttons. It takes
/// the pointer and, through `focus`, the keyboard, so nothing reaches the
/// session underneath.
pub struct ControlSheet {
    message: SharedString,
    focus: FocusHandle,
    buttons: Vec<(SharedString, bool, Listener)>,
}

impl ControlSheet {
    pub fn new(message: impl Into<SharedString>, focus: FocusHandle) -> Self {
        Self {
            message: message.into(),
            focus,
            buttons: Vec::new(),
        }
    }

    /// A button; the primary one carries the recommended choice.
    pub fn button(
        mut self,
        label: impl Into<SharedString>,
        primary: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.buttons
            .push((label.into(), primary, Box::new(on_click)));

        self
    }

    pub fn render(self, cx: &App) -> AnyElement {
        let buttons =
            self.buttons
                .into_iter()
                .enumerate()
                .map(|(index, (label, primary, on_click))| {
                    let button = Button::new(("control-sheet-button", index))
                        .label(label)
                        .on_click(on_click);

                    match primary {
                        true => button.primary(),
                        false => button.outline(),
                    }
                });

        div()
            .id("control-sheet")
            .track_focus(&self.focus)
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            // The pane underneath may set a terminal or agent font; the sheet
            // is application chrome and reads in the interface's.
            .font_family(cx.theme().font_family.clone())
            .text_size(cx.theme().font_size)
            .backdrop_blur(px(24.))
            .bg(cx.theme().background.opacity(0.45))
            .child(
                v_flex()
                    .max_w(px(480.))
                    .gap_4()
                    .items_center()
                    .child(
                        div()
                            .text_center()
                            .text_color(cx.theme().foreground)
                            .child(self.message),
                    )
                    .child(h_flex().gap_2().children(buttons)),
            )
            .into_any_element()
    }
}
