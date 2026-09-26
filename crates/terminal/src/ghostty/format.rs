use std::{ptr, slice};

use libghostty_vt_sys::{
    Formatter as VtFormatter, FormatterFormat as VtFormatterFormat,
    FormatterScreenExtra as VtFormatterScreenExtra,
    FormatterTerminalExtra as VtFormatterTerminalExtra,
    FormatterTerminalOptions as VtFormatterTerminalOptions, Selection as VtSelection,
    Terminal as VtTerminal, ghostty_formatter_format_alloc, ghostty_formatter_free,
    ghostty_formatter_terminal_new, ghostty_free, sized as vt_sized,
};

use crate::ghostty::{Error, Result};

/// Every piece of terminal state the formatter can re-emit. Replaying VT text
/// without it leaves a fresh engine on default modes, cursor, palette, and
/// tabstops, so input encoding and cursor-relative output that follow would
/// diverge from the engine the bytes came from.
pub(super) fn full_state_extra() -> VtFormatterTerminalExtra {
    let mut screen = vt_sized!(VtFormatterScreenExtra);

    screen.cursor = true;
    screen.style = true;
    screen.hyperlink = true;
    screen.protection = true;
    screen.kitty_keyboard = true;
    screen.charsets = true;

    let mut extra = vt_sized!(VtFormatterTerminalExtra);

    extra.palette = true;
    extra.modes = true;
    extra.scrolling_region = true;
    extra.tabstops = true;
    extra.pwd = true;
    extra.keyboard = true;
    extra.screen = screen;

    extra
}

/// Run the engine formatter over one terminal and take ownership of the
/// bytes it allocates. The formatter and its output buffer are separate FFI
/// allocations that have to be released whether or not the format succeeded,
/// which is why they never escape this function.
pub(super) fn format_terminal(
    terminal: VtTerminal,
    emit: VtFormatterFormat::Type,
    extra: VtFormatterTerminalExtra,
    selection: Option<&VtSelection>,
    unwrap: bool,
    trim: bool,
) -> Result<Vec<u8>> {
    let mut opts = vt_sized!(VtFormatterTerminalOptions);

    opts.emit = emit;
    opts.unwrap = unwrap;
    opts.trim = trim;
    opts.extra = extra;

    opts.selection = selection
        .map(|s| s as *const VtSelection)
        .unwrap_or(ptr::null());

    let mut formatter: VtFormatter = ptr::null_mut();

    Error::from_code(unsafe {
        ghostty_formatter_terminal_new(ptr::null(), &mut formatter, terminal, opts)
    })?;

    let mut out_ptr: *mut u8 = ptr::null_mut();
    let mut out_len: usize = 0;

    let res = Error::from_code(unsafe {
        ghostty_formatter_format_alloc(formatter, ptr::null(), &mut out_ptr, &mut out_len)
    });

    let bytes = res.map(|_| {
        if out_ptr.is_null() || out_len == 0 {
            Vec::new()
        } else {
            let bytes = unsafe { slice::from_raw_parts(out_ptr, out_len) };

            bytes.to_vec()
        }
    });

    if !out_ptr.is_null() {
        unsafe { ghostty_free(ptr::null(), out_ptr, out_len) };
    }

    unsafe { ghostty_formatter_free(formatter) };

    bytes
}
