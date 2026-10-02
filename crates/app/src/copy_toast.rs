use std::time::Duration;

use gpui::{App, Styled as _, Window};
use gpui_component::WindowExt as _;
use gpui_component::notification::Notification;
use rust_i18n::t;

struct TextCopiedNotification;

pub(crate) fn show_text_copied(window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::new()
            .message(t!("terminal-text-copied"))
            .id::<TextCopiedNotification>()
            .autohide_after(Duration::from_millis(1500))
            .show_close(false)
            .w_auto()
            .px_3()
            .py_2(),
        cx,
    );
}
