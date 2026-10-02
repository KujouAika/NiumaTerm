use nmt_platform::environment::DEFAULT_EDITOR;

use crate::{CursorShape, Shell};

#[inline]
pub(crate) fn default_bool_true() -> bool {
    true
}

#[inline]
pub fn default_shell() -> Shell {
    let (program, args) = nmt_platform::configured_shell_defaults();

    Shell { program, args }
}

#[inline]
pub(crate) fn default_working_dir() -> Option<String> {
    None
}

#[inline]
pub(crate) fn default_cursor() -> CursorShape {
    CursorShape::default()
}

#[inline]
pub fn default_theme() -> String {
    "modern_dark".into()
}

#[inline]
pub(crate) fn default_editor() -> Shell {
    Shell {
        program: DEFAULT_EDITOR.into(),
        args: vec![],
    }
}
