use crate::{NSStringExt, id, nil};
use gpui::WindowAppearance;
use objc2::msg_send;
use objc2_app_kit::{NSAppearanceNameVibrantDark, NSAppearanceNameVibrantLight};
use objc2_foundation::NSString;

pub(crate) unsafe fn window_appearance_from_native(appearance: id) -> WindowAppearance {
    // A nil appearance has no name; checking first keeps the lookup from
    // messaging nil, which debug builds of objc2 reject.
    let name: id = if appearance.is_null() {
        nil
    } else {
        unsafe { msg_send![appearance, name] }
    };
    unsafe {
        if name == NSAppearanceNameVibrantLight as *const NSString as id {
            WindowAppearance::VibrantLight
        } else if name == NSAppearanceNameVibrantDark as *const NSString as id {
            WindowAppearance::VibrantDark
        } else if name == NSAppearanceNameAqua {
            WindowAppearance::Light
        } else if name == NSAppearanceNameDarkAqua {
            WindowAppearance::Dark
        } else {
            println!("unknown appearance: {:?}", name.to_str());
            WindowAppearance::Light
        }
    }
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    pub static NSAppearanceNameAqua: id;
    pub static NSAppearanceNameDarkAqua: id;
}
