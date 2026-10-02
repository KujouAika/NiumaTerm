use crate::terminal_tab::scrollbar::geometry::{
    scrollbar_offset_for_thumb, scrollbar_thumb_geometry,
};

#[test]
fn scrollbar_thumb_stays_inside_track_with_long_history() {
    let (top, height) = scrollbar_thumb_geometry(10_000.0, 9_975.0, 25.0).unwrap();

    assert!(top + height <= 1.0, "thumb bottom was {}", top + height);
    assert_eq!(
        scrollbar_offset_for_thumb(10_000.0, 25.0, top),
        Some(9_975.0)
    );
}
