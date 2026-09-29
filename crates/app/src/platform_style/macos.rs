use gpui::prelude::*;
use gpui::{AnyElement, Div, Edges, Stateful, px};
use gpui_component::TitleBar;
use gpui_component::button::Button;
use gpui_component::input::Input;
use gpui_component::tab::Tab;

use crate::design::{SIDEBAR_ROW_GUTTER, SPACE_2, SPACE_3, TITLE_BAR_HEIGHT};
use crate::platform_style::{PlatformStyle, TabDensity};

/// Height of a close/minimize/zoom button, measured from the frame AppKit
/// gives the standard window buttons. Their vertical inset is applied
/// symmetrically from the top of the window, so half the leftover space
/// centers them in the taller title bar.
const TRAFFIC_LIGHT_HEIGHT: f32 = 14.0;

/// Width of the close, minimize and zoom buttons together: three round
/// 14-point buttons with the 9 points AppKit leaves between neighbors.
const TRAFFIC_LIGHTS_WIDTH: f32 = 3.0 * TRAFFIC_LIGHT_HEIGHT + 2.0 * 9.0;

/// Edge of the icon each title bar control centers in its frame.
const TITLE_BAR_ICON: f32 = 16.0;

/// Visible space between neighboring title bar glyphs, and between the zoom
/// button and the first glyph, so the leading group reads as one evenly
/// spaced row with the window buttons.
const TITLE_BAR_GLYPH_SPACING: f32 = 14.0;

/// Where a glyph's ink starts inside its 16-point icon box. Lucide and the
/// app's own icons keep a margin of about this much on each side, so the
/// visible spacing is the box spacing plus two margins.
const TITLE_BAR_GLYPH_MARGIN: f32 = 1.5;

/// AppKit keeps drawing the window buttons over the transparent title bar,
/// so the chrome makes room for them at the leading edge.
pub struct MacOs;

impl PlatformStyle for MacOs {
    /// Equal top and leading insets center the close button within the
    /// window's rounded corner instead of keeping the narrower inset AppKit
    /// applies for a standard-height title bar.
    const WINDOW_CONTROLS_INSET: Option<f32> =
        Some((TITLE_BAR_HEIGHT - TRAFFIC_LIGHT_HEIGHT) / 2.0);

    /// The three window buttons and their spacing occupy the leading edge.
    /// The zoom button's edge carries no ink margin, so the first frame
    /// starts where its glyph lands the glyph spacing past that edge.
    const TITLE_BAR_LEADING_INSET: f32 = (TITLE_BAR_HEIGHT - TRAFFIC_LIGHT_HEIGHT) / 2.0
        + TRAFFIC_LIGHTS_WIDTH
        + TITLE_BAR_GLYPH_SPACING
        - TITLE_BAR_GLYPH_MARGIN
        - (Self::TITLE_BAR_BUTTON_SIZE - TITLE_BAR_ICON) / 2.0;

    /// Keeps toggled and hovered controls inside the window's curved edge.
    const TITLE_BAR_TRAILING_INSET: f32 = 12.0;

    /// Frames touch, and the frame is sized so its padding alone spaces the
    /// glyphs; the frames only show on hover, one at a time.
    const TITLE_BAR_BUTTON_GAP: f32 = 0.0;

    const TITLE_BAR_BUTTON_SIZE: f32 = TITLE_BAR_ICON + TITLE_BAR_GLYPH_SPACING
        - 2.0 * TITLE_BAR_GLYPH_MARGIN
        - Self::TITLE_BAR_BUTTON_GAP;

    /// The last glyph keeps the glyph spacing to the sidebar's edge, as the
    /// first keeps it to the zoom button; its frame's padding and ink margin
    /// already cover part of it. The last control is always the next-busy
    /// arrow, whose ink runs to within half a point of its box rather than
    /// the usual margin.
    const TITLE_BAR_CONTROLS_TRAILING_GAP: f32 =
        TITLE_BAR_GLYPH_SPACING - 0.5 - (Self::TITLE_BAR_BUTTON_SIZE - TITLE_BAR_ICON) / 2.0;

    /// The icon and the close control each keep a real inset, so a tab needs
    /// that much more room before it gives both up for the glyph slot.
    const COMPACT_TAB_WIDTH: f32 = 82.0;

    const FULL_TAB_WIDTH: f32 = 112.0;

    /// A macOS source list marks its selected row with the fill alone, which
    /// lets the workspace names start on the column under the close button.
    const SIDEBAR_SELECTION_MARK: bool = false;

    /// State the reserved leading edge on the bar itself, so the room kept
    /// clear for the window buttons and the width the leading region is
    /// measured against cannot drift apart.
    fn title_bar(bar: TitleBar) -> TitleBar {
        bar.pl(px(Self::TITLE_BAR_LEADING_INSET))
    }

    /// The title sits close to the icon while the trailing padding keeps the
    /// close control clear of the tab's edge. An icon-only tab has no title
    /// area left to pad.
    fn tab(tab: Tab, density: TabDensity) -> Tab {
        match density {
            TabDensity::IconOnly => tab,
            TabDensity::Full | TabDensity::Compact => tab.content_paddings(Edges {
                left: px(4.0),
                right: SPACE_3,
                ..Default::default()
            }),
        }
    }

    /// The inset takes layout room, so the title laid out after the icon
    /// keeps its distance from it.
    fn tab_prefix(prefix: Div) -> Div {
        prefix.pl(SPACE_3)
    }

    /// Mirrors the leading inset at the trailing edge. An icon-only tab hands
    /// its one slot to the close control and has no trailing group to inset.
    fn tab_suffix(suffix: Div, density: TabDensity) -> Div {
        match density {
            TabDensity::IconOnly => suffix,
            TabDensity::Full | TabDensity::Compact => suffix.pr(SPACE_2),
        }
    }

    /// Titles start right after the icon, so every title in the strip begins
    /// at the same offset. A lone glyph stays centered in its slot.
    fn tab_title(title: Stateful<Div>, density: TabDensity) -> Stateful<Div> {
        match density {
            TabDensity::IconOnly => title.justify_center(),
            TabDensity::Full | TabDensity::Compact => title.justify_start(),
        }
    }

    /// Matches the leading title, so renaming edits the text where it already
    /// stands.
    fn tab_rename_input(input: Input) -> Input {
        input.text_left()
    }

    /// The heading centers in the bar, on the same line as the window buttons
    /// and the leading controls, which AppKit and the bar both center
    /// vertically.
    fn session_heading_slot(slot: Div) -> Div {
        slot.items_center()
    }

    /// The status area already reaches into the gutter, so the row keeps its
    /// place in it.
    fn sidebar_agent_usage(usage: AnyElement) -> AnyElement {
        usage
    }

    /// The status area offsets the row into its gutter, so the row pads its
    /// icon back onto the content column and keeps the text off the trailing
    /// edge.
    fn agent_usage_row(row: Button) -> Button {
        row.pl(px(SIDEBAR_ROW_GUTTER)).pr_1()
    }

    /// The content column starts under the close button's leading edge, so
    /// the heading stays on it.
    fn sidebar_heading(heading: Div) -> Div {
        heading
    }

    /// Both status rows reach back into the gutter the workspace row fills
    /// use, so their hover fills start on the same edge as those fills; each
    /// row pads its icon back onto the content column.
    fn sidebar_status(status: Stateful<Div>) -> Stateful<Div> {
        status.ml(px(-SIDEBAR_ROW_GUTTER))
    }

    /// Matches the quota row under it, so the two icons share the content
    /// column.
    fn token_usage_row(row: Button) -> Button {
        Self::agent_usage_row(row)
    }
}
