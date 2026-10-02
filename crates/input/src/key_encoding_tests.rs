//! Characterization (golden) tests for the encoder. The function had **zero** tests
//! before it was moved here; these lock its current byte output so the rio-input
//! extraction and later refactors are verifiably non-regressing.

use crate::event::ElementState;
use crate::keyboard::{Key, KeyLocation, ModifiersState, NamedKey};
use crate::{KeyEncodeFlags, KeyInput, bracket_paste, encode_terminal_input};

fn named(key: NamedKey) -> KeyInput {
    KeyInput {
        logical_key: Key::Named(key),
        key_without_modifiers: Key::Named(key),
        text_with_all_modifiers: None,
        location: KeyLocation::Standard,
        state: ElementState::Pressed,
        repeat: false,
    }
}

fn character(c: &str) -> KeyInput {
    let k = Key::Character(c.into());

    KeyInput {
        logical_key: k.clone(),
        key_without_modifiers: k,
        text_with_all_modifiers: None,
        location: KeyLocation::Standard,
        state: ElementState::Pressed,
        repeat: false,
    }
}

fn released(key: NamedKey) -> KeyInput {
    let mut k = named(key);

    k.state = ElementState::Released;

    k
}

// --- encode_terminal_input: frontend-facing key event policy. ---

#[test]
fn terminal_input_suppresses_release_without_event_type_reporting() {
    let got = encode_terminal_input(
        &released(NamedKey::Escape),
        ModifiersState::empty(),
        KeyEncodeFlags::DISAMBIGUATE_ESC_CODES,
        None,
    );

    assert_eq!(got, None);
}

#[test]
fn terminal_input_reports_release_when_event_type_reporting_is_enabled() {
    let got = encode_terminal_input(
        &released(NamedKey::Escape),
        ModifiersState::empty(),
        KeyEncodeFlags::DISAMBIGUATE_ESC_CODES | KeyEncodeFlags::REPORT_EVENT_TYPES,
        None,
    );

    assert_eq!(got.as_deref(), Some(&b"\x1b[27;1:3u"[..]));
}

#[test]
fn event_reporting_preserves_text_keys_and_encodes_cursor_repeats() {
    let flags = KeyEncodeFlags::DISAMBIGUATE_ESC_CODES | KeyEncodeFlags::REPORT_EVENT_TYPES;

    let mut letter = character("a");

    letter.state = ElementState::Released;

    for input in [
        letter,
        released(NamedKey::Enter),
        released(NamedKey::Tab),
        released(NamedKey::Backspace),
    ] {
        assert!(encode_terminal_input(&input, ModifiersState::empty(), flags, None).is_none());
        assert!(
            encode_terminal_input(
                &input,
                ModifiersState::empty(),
                flags | KeyEncodeFlags::REPORT_ALL_KEYS_AS_ESC,
                None,
            )
            .is_some()
        );
    }

    let mut arrow = named(NamedKey::ArrowUp);

    arrow.repeat = true;

    assert_eq!(
        encode_terminal_input(
            &arrow,
            ModifiersState::empty(),
            flags | KeyEncodeFlags::APP_CURSOR,
            None,
        )
        .as_deref(),
        Some(b"\x1b[1;1:2A".as_slice()),
    );
}

#[test]
fn terminal_input_uses_text_fallback_for_printable_press() {
    let got = encode_terminal_input(
        &character("a"),
        ModifiersState::empty(),
        KeyEncodeFlags::empty(),
        Some("a"),
    );

    assert_eq!(got.as_deref(), Some(&b"a"[..]));
}

// --- terminal protocol helpers used by frontend input adapters. ---

#[test]
fn bracket_paste_wraps_only_when_active() {
    assert_eq!(bracket_paste(b"ls -la", false), b"ls -la".to_vec());
    assert_eq!(
        bracket_paste(b"ls -la", true),
        b"\x1b[200~ls -la\x1b[201~".to_vec()
    );
    assert_eq!(bracket_paste(b"", true), b"\x1b[200~\x1b[201~".to_vec());
}
