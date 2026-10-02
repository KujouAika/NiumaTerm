//! Cell color resolution against a theme palette.
//!
//! Every frontend that draws the viewport grid (the desktop renderer and the
//! mobile core) resolves interned `Style` colors the same way: the engine's
//! OSC override layer over the theme palette, bold brightening of the base
//! colors, and the dim remap. Keeping the rules here means a program's
//! colors look the same on every device that shows its terminal.

use nmt_config::colors::term::{DIM_FACTOR, List, TermColors};
use nmt_config::colors::{AnsiColor, ColorArray, ColorRgb, NamedColor};

use crate::grid::StyleFlags;

/// Resolve `color` for a cell with `flags`. `foreground` selects the text
/// rules: bold brightens and dim darkens only the text color.
pub fn resolve_color(
    palette: &List,
    overrides: &TermColors,
    color: &AnsiColor,
    flags: StyleFlags,
    foreground: bool,
) -> ColorRgb {
    let dim = foreground && flags.contains(StyleFlags::DIM);
    let bold = foreground && flags.contains(StyleFlags::BOLD);

    match color {
        AnsiColor::Named(named) => {
            let named = if foreground && bold && !dim {
                named.to_light()
            } else if dim {
                named.to_dim()
            } else {
                *named
            };

            indexed_color(palette, overrides, named as usize)
        }
        AnsiColor::Spec(rgb) => {
            if dim {
                dim_color(*rgb)
            } else {
                *rgb
            }
        }
        AnsiColor::Indexed(index) => {
            let index = match (foreground, dim, bold, *index) {
                (true, true, _, 8..=15) => *index as usize - 8,
                (true, true, _, 0..=7) => NamedColor::DimBlack as usize + *index as usize,
                (false, false, true, 0..=7) => *index as usize + 8,
                (false, true, false, 8..=15) => *index as usize - 8,
                (false, true, false, 0..=7) => NamedColor::DimBlack as usize + *index as usize,
                _ => *index as usize,
            };

            indexed_color(palette, overrides, index)
        }
    }
}

/// Palette slot `index`, taking a program's OSC override when it set one.
pub fn indexed_color(palette: &List, overrides: &TermColors, index: usize) -> ColorRgb {
    overrides[index].unwrap_or(palette[index]).into()
}

/// Faint text: the color scaled by the same factor the palette uses for its
/// derived dim colors.
pub fn dim_color(color: ColorRgb) -> ColorRgb {
    let color: ColorArray = (color * DIM_FACTOR).into();

    color.into()
}
