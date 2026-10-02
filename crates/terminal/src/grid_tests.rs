use std::mem;

use nmt_config::colors::{AnsiColor, NamedColor};

use crate::grid::*;

#[test]
fn square_is_eight_bytes() {
    // The whole point of this rewrite.
    assert_eq!(mem::size_of::<Square>(), 8);
}

#[test]
fn fields_are_independent() {
    let mut s = Square(0);

    s.set_c('Z');

    s.set_style_id(0x1234);

    s.set_extras_id(Some(0x5678));

    s.set_wide(Wide::Wide);

    assert_eq!(s.c(), 'Z');
    assert_eq!(s.style_id(), 0x1234);
    assert_eq!(s.extras_id(), Some(0x5678));
    assert_eq!(s.wide(), Wide::Wide);
}

#[test]
fn intern_returns_existing_id() {
    let mut set = StyleSet::new();

    let s = Style {
        fg: AnsiColor::Named(NamedColor::Red),
        ..Style::default()
    };

    let id1 = set.intern(s);
    let id2 = set.intern(s);

    assert_eq!(id1, id2);
    assert_ne!(id1, DEFAULT_STYLE_ID);
    assert_eq!(set.len(), 2);
}

#[test]
fn distinct_styles_get_distinct_ids() {
    let mut set = StyleSet::new();

    let red = Style {
        fg: AnsiColor::Named(NamedColor::Red),
        ..Style::default()
    };

    let blue = Style {
        fg: AnsiColor::Named(NamedColor::Blue),
        ..Style::default()
    };

    assert_ne!(set.intern(red), set.intern(blue));
    assert_eq!(set.len(), 3);
}
