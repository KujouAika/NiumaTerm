//! A message waiting for the harness its submission launched.

use gpui::prelude::*;
use gpui::{AnyElement, App, div, px, relative};
use gpui_component::spinner::Spinner;
use gpui_component::{ActiveTheme as _, IconName, Sizable as _, h_flex, text, v_flex};
use rust_i18n::t;

use crate::agent_tab::transcript::disclosure_row::{
    USER_BUBBLE_PADDING_X, USER_BUBBLE_PADDING_Y, USER_BUBBLE_RADIUS, USER_BUBBLE_TAIL_RADIUS,
    USER_BUBBLE_WIDTH_FRACTION,
};

/// The message `text`, if one is held, in the bubble it will be sent as,
/// over a line saying that `agent` is starting. The message has already left
/// the composer, so showing it where it will land keeps the send from
/// reading as dropped while the harness comes up; the line is all a held
/// slash command gets, because the command is still in the composer.
pub(crate) fn held_prompt(text: Option<&str>, agent: &str, cx: &App) -> AnyElement {
    let bubble = text.map(|text| {
        h_flex().w_full().justify_end().child(
            div()
                .max_w(relative(USER_BUBBLE_WIDTH_FRACTION))
                .min_w_0()
                .px(px(USER_BUBBLE_PADDING_X))
                .py(px(USER_BUBBLE_PADDING_Y))
                .rounded_tl(px(USER_BUBBLE_RADIUS))
                .rounded_tr(px(USER_BUBBLE_RADIUS))
                .rounded_bl(px(USER_BUBBLE_RADIUS))
                .rounded_br(px(USER_BUBBLE_TAIL_RADIUS))
                .bg(cx.theme().muted)
                .child(text::TextView::plain("held-prompt-text", text.to_owned())),
        )
    });

    let status = h_flex()
        .gap_2()
        .items_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(
            Spinner::new()
                .icon(IconName::LoaderCircle)
                .small()
                .color(cx.theme().muted_foreground),
        )
        .child(t!("agent-start-launching", name = agent).to_string());

    v_flex()
        .w_full()
        .gap_3()
        .pb_4()
        .children(bubble)
        .child(status)
        .into_any_element()
}
