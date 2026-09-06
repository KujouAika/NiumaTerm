use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertStyle};
use objc2_foundation::NSString;
use tracing::error;

/// Report a failure the application cannot start past.
///
/// This is reached before there is a window, and sometimes before there is an
/// event loop, so the message is also logged: an alert needs the main thread,
/// and a caller that is already off it would otherwise lose the reason
/// entirely.
pub fn show_error_dialog(title: &str, message: &str) {
    error!("{title}: {message}");

    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };

    // SAFETY: every argument is an owned `NSString`, and the alert is run on
    // the main thread the marker proves this is.
    unsafe {
        let alert = NSAlert::new(main_thread);
        alert.setAlertStyle(NSAlertStyle::Critical);
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(message));
        alert.runModal();
    }
}
