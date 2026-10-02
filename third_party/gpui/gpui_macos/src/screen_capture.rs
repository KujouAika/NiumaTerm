use crate::{id, nil, ns_string};
use anyhow::{Result, anyhow};
use block2::RcBlock;
use collections::HashMap;
use core_foundation::base::TCFType;
use core_graphics::display::{
    CGDirectDisplayID, CGDisplayCopyDisplayMode, CGDisplayModeGetPixelHeight,
    CGDisplayModeGetPixelWidth, CGDisplayModeRelease,
};
use ctor::ctor;
use futures::channel::oneshot;
use gpui::{
    DevicePixels, ForegroundExecutor, ScreenCaptureFrame, ScreenCaptureSource, ScreenCaptureStream,
    SharedString, SourceMetadata, size,
};
use gpui_media::core_media::{CMSampleBuffer, CMSampleBufferRef};
use objc2::{
    class, msg_send,
    runtime::{AnyClass, AnyObject, ClassBuilder, Sel},
    sel,
};
use std::{
    cell::RefCell,
    ffi::{CStr, c_void},
    mem, ptr,
    rc::Rc,
};

use crate::NSStringExt;

#[derive(Clone)]
pub struct MacScreenCaptureSource {
    sc_display: id,
    meta: Option<ScreenMeta>,
}

pub struct MacScreenCaptureStream {
    sc_stream: id,
    sc_stream_output: id,
    meta: SourceMetadata,
}

static mut DELEGATE_CLASS: *const AnyClass = ptr::null();
static mut OUTPUT_CLASS: *const AnyClass = ptr::null();
const FRAME_CALLBACK_IVAR: &CStr = c"frame_callback";

#[allow(non_upper_case_globals)]
const SCStreamOutputTypeScreen: isize = 0;

impl ScreenCaptureSource for MacScreenCaptureSource {
    fn metadata(&self) -> Result<SourceMetadata> {
        let (display_id, size) = unsafe {
            let display_id: CGDirectDisplayID = msg_send![self.sc_display, displayID];
            let display_mode_ref = CGDisplayCopyDisplayMode(display_id);
            let width = CGDisplayModeGetPixelWidth(display_mode_ref);
            let height = CGDisplayModeGetPixelHeight(display_mode_ref);
            CGDisplayModeRelease(display_mode_ref);

            (
                display_id,
                size(DevicePixels(width as i32), DevicePixels(height as i32)),
            )
        };
        let (label, is_main) = self
            .meta
            .clone()
            .map(|meta| (meta.label, meta.is_main))
            .unzip();

        Ok(SourceMetadata {
            id: display_id as u64,
            label,
            is_main,
            resolution: size,
        })
    }

    fn stream(
        &self,
        _foreground_executor: &ForegroundExecutor,
        frame_callback: Box<dyn Fn(ScreenCaptureFrame) + Send>,
    ) -> oneshot::Receiver<Result<Box<dyn ScreenCaptureStream>>> {
        unsafe {
            let stream: id = msg_send![class!(SCStream), alloc];
            let filter: id = msg_send![class!(SCContentFilter), alloc];
            let configuration: id = msg_send![class!(SCStreamConfiguration), alloc];
            let delegate: id = msg_send![&*DELEGATE_CLASS, alloc];
            let output: id = msg_send![&*OUTPUT_CLASS, alloc];

            let excluded_windows: id = msg_send![class!(NSArray), array];
            let filter: id = msg_send![filter, initWithDisplay: self.sc_display, excludingWindows: excluded_windows];
            let configuration: id = msg_send![configuration, init];
            let _: () = msg_send![configuration, setScalesToFit: true];
            let _: () = msg_send![configuration, setPixelFormat: 0x42475241u32];
            // let _: id = msg_send![configuration, setShowsCursor: false];
            // let _: id = msg_send![configuration, setCaptureResolution: 3];
            let delegate: id = msg_send![delegate, init];
            let output: id = msg_send![output, init];

            let output_object = output.as_mut().unwrap();
            let frame_callback_ivar = output_object
                .class()
                .instance_variable(FRAME_CALLBACK_IVAR)
                .unwrap();
            *frame_callback_ivar.load_mut::<*mut c_void>(output_object) =
                Box::into_raw(Box::new(frame_callback)) as *mut c_void;

            let meta = self.metadata().unwrap();
            let _: () = msg_send![configuration, setWidth: meta.resolution.width.0 as usize];
            let _: () = msg_send![configuration, setHeight: meta.resolution.height.0 as usize];
            let stream: id = msg_send![stream, initWithFilter: filter, configuration: configuration, delegate: delegate];

            // Stream contains filter, configuration, and delegate internally so we release them here
            // to prevent a memory leak when steam is dropped
            let _: () = msg_send![filter, release];
            let _: () = msg_send![configuration, release];
            let _: () = msg_send![delegate, release];

            let (tx, rx) = oneshot::channel();

            let mut error: id = nil;
            let _: bool = msg_send![stream, addStreamOutput: output, type: SCStreamOutputTypeScreen, sampleHandlerQueue: nil, error: &mut error as *mut id];
            if error != nil {
                let message: id = msg_send![error, localizedDescription];
                let _: () = msg_send![stream, release];
                let _: () = msg_send![output, release];
                tx.send(Err(anyhow!("failed to add stream output {message:?}")))
                    .ok();
                return rx;
            }

            let tx = Rc::new(RefCell::new(Some(tx)));
            let handler = RcBlock::new({
                move |error: id| {
                    let result = if error == nil {
                        let stream = MacScreenCaptureStream {
                            meta: meta.clone(),
                            sc_stream: stream,
                            sc_stream_output: output,
                        };
                        Ok(Box::new(stream) as Box<dyn ScreenCaptureStream>)
                    } else {
                        let _: () = msg_send![stream, release];
                        let _: () = msg_send![output, release];
                        let message: id = msg_send![error, localizedDescription];
                        Err(anyhow!("failed to start screen capture stream {message:?}"))
                    };
                    if let Some(tx) = tx.borrow_mut().take() {
                        tx.send(result).ok();
                    }
                }
            });
            let _: () = msg_send![stream, startCaptureWithCompletionHandler: &*handler];
            rx
        }
    }
}

impl Drop for MacScreenCaptureSource {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![self.sc_display, release];
        }
    }
}

impl ScreenCaptureStream for MacScreenCaptureStream {
    fn metadata(&self) -> Result<SourceMetadata> {
        Ok(self.meta.clone())
    }
}

impl Drop for MacScreenCaptureStream {
    fn drop(&mut self) {
        unsafe {
            let mut error: id = nil;
            let _: bool = msg_send![self.sc_stream, removeStreamOutput: self.sc_stream_output, type: SCStreamOutputTypeScreen, error: &mut error as *mut id];
            if error != nil {
                let message: id = msg_send![error, localizedDescription];
                log::error!("failed to add stream  output {message:?}");
            }

            let handler = RcBlock::new(move |error: id| {
                if error != nil {
                    let message: id = msg_send![error, localizedDescription];
                    log::error!("failed to stop screen capture stream {message:?}");
                }
            });
            let _: () = msg_send![self.sc_stream, stopCaptureWithCompletionHandler: &*handler];
            let _: () = msg_send![self.sc_stream, release];
            let _: () = msg_send![self.sc_stream_output, release];
        }
    }
}

#[derive(Clone)]
struct ScreenMeta {
    label: SharedString,
    // Is this the screen with menu bar?
    is_main: bool,
}

unsafe fn screen_id_to_human_label() -> HashMap<CGDirectDisplayID, ScreenMeta> {
    let screens: id = msg_send![class!(NSScreen), screens];
    let count: usize = msg_send![screens, count];
    let mut map = HashMap::default();
    let screen_number_key = unsafe { ns_string("NSScreenNumber") };
    for i in 0..count {
        let screen: id = msg_send![screens, objectAtIndex: i];
        let device_desc: id = msg_send![screen, deviceDescription];
        if device_desc == nil {
            continue;
        }

        let nsnumber: id = msg_send![device_desc, objectForKey: screen_number_key];
        if nsnumber == nil {
            continue;
        }

        let screen_id: u32 = msg_send![nsnumber, unsignedIntValue];

        let name: id = msg_send![screen, localizedName];
        if name != nil {
            let cstr: *const std::os::raw::c_char = msg_send![name, UTF8String];
            let rust_str = unsafe {
                std::ffi::CStr::from_ptr(cstr)
                    .to_string_lossy()
                    .into_owned()
            };
            map.insert(
                screen_id,
                ScreenMeta {
                    label: rust_str.into(),
                    is_main: i == 0,
                },
            );
        }
    }
    map
}

pub(crate) fn get_sources() -> oneshot::Receiver<Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
    unsafe {
        let (tx, rx) = oneshot::channel();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let screen_id_to_label = screen_id_to_human_label();
        let block = RcBlock::new(move |shareable_content: id, error: id| {
            let Some(tx) = tx.borrow_mut().take() else {
                return;
            };

            let result = if error == nil {
                // A nil content object without an error yields no displays
                // rather than messaging nil.
                let displays: id = if shareable_content == nil {
                    nil
                } else {
                    msg_send![shareable_content, displays]
                };
                let count: usize = if displays == nil {
                    0
                } else {
                    msg_send![displays, count]
                };
                let mut result = Vec::new();
                for i in 0..count {
                    let display: id = msg_send![displays, objectAtIndex: i];
                    let id: CGDirectDisplayID = msg_send![display, displayID];
                    let meta = screen_id_to_label.get(&id).cloned();
                    let source = MacScreenCaptureSource {
                        sc_display: msg_send![display, retain],
                        meta,
                    };
                    result.push(Rc::new(source) as Rc<dyn ScreenCaptureSource>);
                }
                Ok(result)
            } else {
                let msg: id = msg_send![error, localizedDescription];
                Err(anyhow!(
                    "Screen share failed: {:?}",
                    NSStringExt::to_str(&msg)
                ))
            };
            tx.send(result).ok();
        });
        let _: () = msg_send![
            class!(SCShareableContent),
            getShareableContentExcludingDesktopWindows: true,
            onScreenWindowsOnly: true,
            completionHandler: &*block
        ];
        rx
    }
}

#[ctor(unsafe)]
unsafe fn build_classes() {
    let mut decl = ClassBuilder::new(c"GPUIStreamDelegate", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(
            sel!(outputVideoEffectDidStartForStream:),
            output_video_effect_did_start_for_stream as extern "C" fn(_, Sel, id),
        );
        decl.add_method(
            sel!(outputVideoEffectDidStopForStream:),
            output_video_effect_did_stop_for_stream as extern "C" fn(_, Sel, id),
        );
        decl.add_method(
            sel!(stream:didStopWithError:),
            stream_did_stop_with_error as extern "C" fn(_, Sel, id, id),
        );
        DELEGATE_CLASS = decl.register();

        let mut decl = ClassBuilder::new(c"GPUIStreamOutput", class!(NSObject)).unwrap();
        decl.add_method(
            sel!(stream:didOutputSampleBuffer:ofType:),
            stream_did_output_sample_buffer_of_type as extern "C" fn(_, Sel, id, id, isize),
        );
        decl.add_ivar::<*mut c_void>(FRAME_CALLBACK_IVAR);

        OUTPUT_CLASS = decl.register();
    }
}

extern "C" fn output_video_effect_did_start_for_stream(_this: &AnyObject, _: Sel, _stream: id) {}

extern "C" fn output_video_effect_did_stop_for_stream(_this: &AnyObject, _: Sel, _stream: id) {}

extern "C" fn stream_did_stop_with_error(_this: &AnyObject, _: Sel, _stream: id, _error: id) {}

extern "C" fn stream_did_output_sample_buffer_of_type(
    this: &AnyObject,
    _: Sel,
    _stream: id,
    sample_buffer: id,
    buffer_type: isize,
) {
    if buffer_type != SCStreamOutputTypeScreen {
        return;
    }

    unsafe {
        let sample_buffer = sample_buffer as CMSampleBufferRef;
        let sample_buffer = CMSampleBuffer::wrap_under_get_rule(sample_buffer);
        if let Some(buffer) = sample_buffer.image_buffer() {
            let frame_callback = *this
                .class()
                .instance_variable(FRAME_CALLBACK_IVAR)
                .unwrap()
                .load::<*mut c_void>(this);
            let callback: Box<Box<dyn Fn(ScreenCaptureFrame)>> =
                Box::from_raw(frame_callback as *mut _);
            callback(ScreenCaptureFrame(buffer));
            mem::forget(callback);
        }
    }
}
