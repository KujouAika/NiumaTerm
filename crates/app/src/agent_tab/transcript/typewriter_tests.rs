use std::time::Instant;

use crate::agent_tab::transcript::typewriter::{ReplyTyping, shown_prefix};

/// The cut lands between characters, whatever their width in bytes, and a
/// count past the end lets the whole text through.
#[test]
fn a_prefix_ends_on_a_character_boundary() {
    assert_eq!(shown_prefix("你好世界", 2), "你好");
    assert_eq!(shown_prefix("ab", 5), "ab");
    assert_eq!(shown_prefix("ab", 0), "");
}

/// A completed payload merged in the same batch as a delta can leave the
/// delta's byte offset past the end of the text, or inside a character; the
/// reply still starts typing instead of panicking on the UI thread.
#[test]
fn a_stale_offset_starts_typing_at_the_nearest_character() {
    let mut typing = ReplyTyping::new();

    typing.begin(0, "hi", 12, Instant::now());

    assert_eq!(typing.shown_reply(0, "hi"), "hi");

    let mut typing = ReplyTyping::new();

    // Byte 1 falls inside the two-byte "é".
    typing.begin(0, "é and more", 1, Instant::now());

    assert_eq!(typing.shown_reply(0, "é and more"), "");
}
