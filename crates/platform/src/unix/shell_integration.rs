use anyhow::{Result, bail};

/// Register the file-manager entry that opens a directory in this terminal.
///
/// There is no counterpart here. The Windows entry is a shell extension DLL
/// registered against Explorer; a file manager on this platform is reached
/// through its own mechanism, and none has been implemented.
pub fn register_shell_integration() -> Result<()> {
    bail!("a file-manager entry is not available on this platform")
}

pub fn unregister_shell_integration() -> Result<()> {
    bail!("a file-manager entry is not available on this platform")
}

pub fn is_shell_integration_registered() -> bool {
    false
}

pub fn shell_integration_dll_mismatched() -> bool {
    false
}

/// Whether the operating system will deliver this application's notifications.
///
/// The answer is the user's, not ours: macOS keeps the permission in System
/// Settings and applies it when a notification is posted. Reporting `true`
/// says only that the application will ask; a denial is honoured by the
/// notification centre itself.
pub fn system_notification_enabled() -> bool {
    true
}

pub fn set_system_notification_enabled(_enabled: bool) -> Result<()> {
    bail!("notification delivery is set in System Settings, not by this application")
}
