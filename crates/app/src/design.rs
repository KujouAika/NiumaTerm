//! Shared geometry for application chrome and conversation surfaces.

use gpui::{Pixels, Window, px};

pub const CONTROL_RADIUS: Pixels = px(6.0);
pub const SURFACE_RADIUS: Pixels = px(8.0);
pub const CARD_RADIUS: Pixels = px(12.0);
pub const TAB_HEIGHT: Pixels = px(32.0);

/// The settings tab ends on the navigation pane's content divider.
pub const SETTINGS_NAV_WIDTH: Pixels = px(SETTINGS_NAV_WIDTH_PX);

/// `SETTINGS_NAV_WIDTH` as a plain number, for window geometry computed in
/// constant expressions, which cannot read a `Pixels` value.
pub const SETTINGS_NAV_WIDTH_PX: f32 = 240.0;

/// Adjacent file and review panes share a footer baseline at every window size.
pub const REVIEW_FOOTER_HEIGHT: Pixels = px(32.0);

/// Repeated layout spacing follows a four-pixel scale.
pub const SPACE_2: Pixels = px(8.0);

pub const SPACE_3: Pixels = px(12.0);

/// Auxiliary views yield space before the main reading area becomes cramped.
pub const PRIMARY_CONTENT_MIN_WIDTH: Pixels = px(320.0);

pub const AUXILIARY_MIN_WIDTH: Pixels = px(240.0);
pub const AUXILIARY_MAX_WIDTH: Pixels = px(480.0);

/// A starting proportion for an auxiliary view, before content bounds or user resizing.
pub const AUXILIARY_WIDTH_SHARE: f32 = 0.381_966;

/// File navigation leaves most of the review surface available for code.
pub const REVIEW_FILES_WIDTH: Pixels = px(224.0);

pub const REVIEW_FILES_MIN_WIDTH: Pixels = px(160.0);
pub const REVIEW_FILES_MAX_WIDTH: Pixels = px(400.0);

/// Compact previews compare multiple palettes without imposing an aspect ratio.
pub const THEME_CARD_MIN_WIDTH: Pixels = px(200.0);

pub const THEME_CARD_HEIGHT: Pixels = px(120.0);
pub const THEME_PREVIEW_HEIGHT: Pixels = px(72.0);
pub const THEME_GRID_MAX_COLUMNS: u16 = 4;

/// The bar is taller than the Fluent standard strip because it holds
/// controls and a session heading, not a title alone. A host that
/// draws its own window buttons over the bar measures their inset from this
/// height, since it would otherwise center them in a shorter strip.
pub const TITLE_BAR_HEIGHT: f32 = 44.0;

/// How far a row's fill reaches back into the sidebar inset on each side,
/// and how much padding the row then puts back so its content still starts
/// on the column's edge. Without it the highlight stops exactly where the
/// first glyph starts and reads as clipped; the leading half is also the
/// lane the selected-row mark stands in.
pub const SIDEBAR_ROW_GUTTER: f32 = 6.0;

/// How often a looping status animation (a busy dot, a pulse, a spinner)
/// re-renders. Each re-render rebuilds the whole view hosting the mark, and
/// the display link would otherwise drive that at the panel's refresh rate
/// (120 Hz on ProMotion) for as long as an agent works. A slow breath or a
/// small spinner reads as smooth well below that; a window the user is not
/// looking at only needs to show that something is still moving.
pub fn status_animation_fps(window: &Window) -> f32 {
    if window.is_window_active() {
        30.0
    } else {
        10.0
    }
}
