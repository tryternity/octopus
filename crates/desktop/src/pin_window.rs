/// 跨平台贴图窗口抽象。
pub trait PinWindow {
    fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64);
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::Mutex;
    use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
    use objc2_app_kit::{NSColor, NSImage, NSImageView, NSView, NSWindow};
    use objc2_foundation::{NSData, NSPoint, NSRect, NSSize};
    use std::ops::Deref;

    struct SendWindow(Retained<NSWindow>);
    unsafe impl Send for SendWindow {}
    unsafe impl Sync for SendWindow {}

    static PIN_WINDOWS: Mutex<Vec<SendWindow>> = Mutex::new(Vec::new());

    pub struct MacPinWindow;

    impl super::PinWindow for MacPinWindow {
        fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) {
            unsafe {
                // 1. NSData from PNG bytes
                let ns_data = NSData::with_bytes(png_data);
                let ns_data_ptr: *mut NSData = ns_data.deref() as *const NSData as *mut NSData;

                // 2. NSImage from data
                let img_ptr: *mut AnyObject = msg_send![class!(NSImage), alloc];
                let image: *mut NSImage = msg_send![img_ptr, initWithData: ns_data_ptr];
                if image.is_null() {
                    log::error!("Failed to create NSImage from PNG data");
                    return;
                }

                // 3. NSImageView
                let iv_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
                let iv_ptr: *mut AnyObject = msg_send![class!(NSImageView), alloc];
                let image_view: *mut NSImageView = msg_send![iv_ptr, initWithFrame: iv_frame];
                let _: () = msg_send![image_view, setImage: image];

                // 4. NSWindow (borderless + floating)
                let win_frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
                let win_ptr: *mut AnyObject = msg_send![class!(NSWindow), alloc];
                let window: *mut NSWindow = msg_send![
                    win_ptr,
                    initWithContentRect: win_frame,
                    styleMask: 0u64,  // NSWindowStyleMaskBorderless = 0
                    backing: 2u64,     // NSBackingStoreBuffered
                    defer: false
                ];

                // Set properties
                let _: () = msg_send![window, setLevel: 3i64]; // NSFloatingWindowLevel
                let _: () = msg_send![window, setHasShadow: true];
                let _: () = msg_send![window, setOpaque: false];
                let clear: *mut NSColor = msg_send![class!(NSColor), clearColor];
                let _: () = msg_send![window, setBackgroundColor: clear];

                // 5. Add image view to content view
                let content: *mut NSView = msg_send![window, contentView];
                let _: () = msg_send![content, addSubview: image_view];

                // 6. Show
                let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];

                // 7. Retain
                let retained = Retained::retain(window).expect("retain NSWindow");
                PIN_WINDOWS.lock().unwrap().push(SendWindow(retained));

                log::info!("Pin window created at ({}, {}) {}x{}", x, y, width, height);
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::MacPinWindow;
