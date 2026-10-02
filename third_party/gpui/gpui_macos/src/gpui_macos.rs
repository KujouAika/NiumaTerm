#![cfg(target_os = "macos")]
//! macOS platform implementation for GPUI.
//!
//! macOS screens have a y axis that goes up from the bottom of the screen and
//! an origin at the bottom left of the main display.

mod dispatcher;
mod display;
mod display_link;
mod events;
mod keyboard;
mod pasteboard;
mod system_notifications;

#[cfg(feature = "screen-capture")]
mod screen_capture;

use gpui_apple::metal_renderer as renderer;

pub mod metal_renderer {
    pub use gpui_apple::metal_renderer::{PathRasterizationVertex, PathSprite, SurfaceBounds};

    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub use gpui_apple::metal_renderer::MetalHeadlessRenderer;
}

#[cfg(feature = "font-kit")]
mod open_type;

#[cfg(feature = "font-kit")]
mod text_system;

mod platform;
mod window;
mod window_appearance;

use objc2::{
    Encode, Encoding, RefEncode, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool},
};
use objc2_foundation::{NSNotFound, NSString};
use std::{
    ffi::{CStr, c_char},
    ops::Range,
    ptr,
};

pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub(crate) use display_link::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
pub(crate) use window::*;

#[cfg(feature = "font-kit")]
pub(crate) use text_system::*;

pub use platform::MacPlatform;

/// Untyped Objective-C object pointer.
///
/// Most of the AppKit glue talks to objects through dynamically registered
/// subclasses and raw ivars, so a nullable untyped pointer matches how those
/// objects are passed around better than the typed `Retained` wrappers.
#[allow(non_camel_case_types)]
pub(crate) type id = *mut AnyObject;

#[allow(non_upper_case_globals)]
pub(crate) const nil: id = ptr::null_mut();

trait BoolExt {
    fn to_objc(self) -> Bool;
}

impl BoolExt for bool {
    fn to_objc(self) -> Bool {
        Bool::new(self)
    }
}

trait NSStringExt {
    unsafe fn to_str(&self) -> &str;
}

impl NSStringExt for id {
    unsafe fn to_str(&self) -> &str {
        unsafe {
            let cstr: *const c_char = msg_send![*self, UTF8String];
            if cstr.is_null() {
                ""
            } else {
                CStr::from_ptr(cstr).to_str().unwrap()
            }
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
struct NSRange {
    pub location: usize,
    pub length: usize,
}

impl NSRange {
    fn invalid() -> Self {
        Self {
            location: NSNotFound as usize,
            length: 0,
        }
    }

    fn is_valid(&self) -> bool {
        self.location != NSNotFound as usize
    }

    fn to_range(self) -> Option<Range<usize>> {
        if self.is_valid() {
            let start = self.location;
            let end = start + self.length;
            Some(start..end)
        } else {
            None
        }
    }
}

impl From<Range<usize>> for NSRange {
    fn from(range: Range<usize>) -> Self {
        NSRange {
            location: range.start,
            length: range.len(),
        }
    }
}

// Matches the `{_NSRange=QQ}` encoding AppKit reports for `NSRange`
// parameters, so debug-build message verification accepts this local type.
unsafe impl Encode for NSRange {
    const ENCODING: Encoding = Encoding::Struct("_NSRange", &[usize::ENCODING, usize::ENCODING]);
}

unsafe impl RefEncode for NSRange {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

/// Returns an autoreleased `NSString`; callers must be inside an autorelease
/// pool, which AppKit callbacks and the run loop always provide.
unsafe fn ns_string(string: &str) -> id {
    Retained::autorelease_ptr(NSString::from_str(string)).cast()
}
