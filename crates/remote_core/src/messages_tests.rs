use crate::messages::{MAX_DEVICE_NAME_CHARS, device_name};

#[test]
fn device_names_are_trimmed_and_limited_by_characters_not_bytes() {
    assert_eq!(
        device_name("  Studio Mac \n").as_deref(),
        Some("Studio Mac")
    );

    // Three bytes per character in UTF-8, yet within the character limit.
    let wide = "\u{5de5}".repeat(MAX_DEVICE_NAME_CHARS);

    assert_eq!(device_name(&wide), Some(wide.clone()));
    assert_eq!(device_name(&format!("{wide}x")), None);
}

#[test]
fn blank_names_and_control_characters_are_rejected() {
    assert_eq!(device_name(""), None);
    assert_eq!(device_name(" \t "), None);
    assert_eq!(device_name("Desk\u{7}top"), None);
    assert_eq!(device_name("two\nlines"), None);
}
