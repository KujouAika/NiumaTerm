//! Shared geometry for application chrome and conversation surfaces.

use gpui::{Pixels, px};

pub const CONTROL_RADIUS: Pixels = px(6.0);
pub const SURFACE_RADIUS: Pixels = px(8.0);
pub const CARD_RADIUS: Pixels = px(12.0);
pub const TAB_HEIGHT: Pixels = px(32.0);

/// The settings tab ends on the navigation pane's content divider.
pub const SETTINGS_NAV_WIDTH: Pixels = px(240.0);

/// Adjacent file and review panes share a footer baseline at every window size.
pub const REVIEW_FOOTER_HEIGHT: Pixels = px(32.0);
