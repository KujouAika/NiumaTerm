use gpui::{Keystroke, Modifiers};
use nmt_input::keyboard::ModifiersState;
use nmt_terminal::input::{KeyPhase, TerminalKey};

pub(super) fn terminal_key(key: &Keystroke) -> TerminalKey<'_> {
    TerminalKey {
        key: &key.key,
        key_char: key.key_char.as_deref(),
        modifiers: modifiers_state(key.modifiers),
        function: key.modifiers.function,
        phase: KeyPhase::Press,
    }
}

/// Alt-F4 (close) and Alt-Space (system menu) only work when the key message
/// reaches `DefWindowProc`. GPUI skips native dispatch for any key the view
/// handles, so encoding these to the PTY would remove the window shortcuts.
pub(super) fn is_window_system_key(key: &Keystroke) -> bool {
    cfg!(windows) && key.modifiers == Modifiers::alt() && matches!(key.key.as_str(), "f4" | "space")
}

pub(super) fn modifiers_state(modifiers: Modifiers) -> ModifiersState {
    let mut state = ModifiersState::empty();

    state.set(ModifiersState::SHIFT, modifiers.shift);

    state.set(ModifiersState::ALT, modifiers.alt);

    state.set(ModifiersState::CONTROL, modifiers.control);

    state.set(ModifiersState::SUPER, modifiers.platform);

    state
}
