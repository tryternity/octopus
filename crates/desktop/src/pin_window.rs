/// 跨平台贴图窗口抽象。
pub trait PinWindow {
    fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64);
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::Cell;
    use std::sync::Mutex;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, msg_send_id, class, sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSColor, NSEvent, NSImage, NSImageView, NSMenu, NSMenuItem, NSView, NSWindow,
    };
    use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString};
    use std::ops::Deref;

    struct SendWindow(#[allow(dead_code)] Retained<PinNSWindow>);
    unsafe impl Send for SendWindow {}
    unsafe impl Sync for SendWindow {}

    static PIN_WINDOWS: Mutex<Vec<SendWindow>> = Mutex::new(Vec::new());

    #[derive(Default)]
    struct PinIvars {
        drag_mouse_x: Cell<f64>,
        drag_mouse_y: Cell<f64>,
        drag_origin_x: Cell<f64>,
        drag_origin_y: Cell<f64>,
    }

    define_class!(
        #[unsafe(super(NSWindow))]
        #[ivars = PinIvars]
        struct PinNSWindow;

        impl PinNSWindow {
            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, event: &NSEvent) {
                let loc: NSPoint = unsafe { msg_send![event, locationInWindow] };
                let frame: NSRect = unsafe { msg_send![self, frame] };
                self.ivars().drag_mouse_x.set(frame.origin.x + loc.x);
                self.ivars().drag_mouse_y.set(frame.origin.y + loc.y);
                self.ivars().drag_origin_x.set(frame.origin.x);
                self.ivars().drag_origin_y.set(frame.origin.y);
            }

            #[unsafe(method(mouseDragged:))]
            fn mouse_dragged(&self, _event: &NSEvent) {
                let mouse: NSPoint = unsafe { msg_send![self, mouseLocationOutsideOfEventStream] };
                let dx = mouse.x - self.ivars().drag_mouse_x.get();
                let dy = mouse.y - self.ivars().drag_mouse_y.get();
                let new_origin = NSPoint::new(
                    self.ivars().drag_origin_x.get() + dx,
                    self.ivars().drag_origin_y.get() + dy,
                );
                unsafe { let _: () = msg_send![self, setFrameOrigin: new_origin]; }
            }

            #[unsafe(method(scrollWheel:))]
            fn scroll_wheel(&self, event: &NSEvent) {
                let delta_y: f64 = unsafe { msg_send![event, scrollingDeltaY] };
                if delta_y == 0.0 { return; }
                let frame: NSRect = unsafe { msg_send![self, frame] };
                let sc = 1.0 + delta_y * 0.01;
                let new_w = (frame.size.width * sc).max(20.0).min(10000.0);
                let new_h = (frame.size.height * sc).max(20.0).min(10000.0);
                let mouse_in_win: NSPoint = unsafe { msg_send![event, locationInWindow] };
                let ratio_x = if frame.size.width > 0.0 { mouse_in_win.x / frame.size.width } else { 0.5 };
                let ratio_y = if frame.size.height > 0.0 { mouse_in_win.y / frame.size.height } else { 0.5 };
                let new_x = frame.origin.x + mouse_in_win.x - ratio_x * new_w;
                let new_y = frame.origin.y + mouse_in_win.y - ratio_y * new_h;
                let new_frame = NSRect::new(NSPoint::new(new_x, new_y), NSSize::new(new_w, new_h));
                unsafe { let _: () = msg_send![self, setFrame: new_frame display: true]; }
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, event: &NSEvent) {
                unsafe {
                    let menu: Retained<NSMenu> = msg_send_id![msg_send_id![class!(NSMenu), alloc], init];
                    let title = NSString::from_str("关闭");
                    let empty = NSString::new();
                    let item: Retained<NSMenuItem> = msg_send_id![
                        msg_send_id![class!(NSMenuItem), alloc],
                        initWithTitle: &*title,
                        action: Some(sel!(close)),
                        keyEquivalent: &*empty
                    ];
                    let _: () = msg_send![&item, setTarget: self];
                    menu.addItem(&item);
                    let content: Retained<NSView> = msg_send_id![self, contentView];
                    let menu_ptr = (&*menu) as *const NSMenu as *mut NSMenu;
                    let content_ptr = (&*content) as *const NSView as *mut NSView;
                    let _: () = msg_send![self, popUpContextMenu: menu_ptr withEvent: event forView: content_ptr];
                }
            }
        }
    );

    pub struct MacPinWindow;

    impl super::PinWindow for MacPinWindow {
        fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) {
            unsafe {
                let ns_data = NSData::with_bytes(png_data);
                let ns_data_ptr = ns_data.deref() as *const NSData as *mut NSData;
                let image: Retained<NSImage> = msg_send_id![
                    msg_send_id![class!(NSImage), alloc],
                    initWithData: ns_data_ptr
                ];

                let iv_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
                    let image_view: Retained<NSImageView> = msg_send_id![
                        msg_send_id![class!(NSImageView), alloc],
                        initWithFrame: iv_frame
                    ];
                let image_ptr = (&*image) as *const NSImage as *mut NSImage;
                let _: () = msg_send![&image_view, setImage: image_ptr];

                let mtm = MainThreadMarker::new().expect("must be main thread");
                let win_frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
                let window: Retained<PinNSWindow> = msg_send_id![
                    PinNSWindow::alloc(mtm),
                    initWithContentRect: win_frame,
                    styleMask: 0u64,
                    backing: 2u64,
                    defer: false
                ];

                let _: () = msg_send![&window, setLevel: 3i64];
                let _: () = msg_send![&window, setHasShadow: true];
                let _: () = msg_send![&window, setOpaque: false];
                let clear: Retained<NSColor> = msg_send_id![class!(NSColor), clearColor];
                let clear_ptr = (&*clear) as *const NSColor as *mut NSColor;
                let _: () = msg_send![&window, setBackgroundColor: clear_ptr];

                let content: Retained<NSView> = msg_send_id![&window, contentView];
                let iv_ptr = (&*image_view) as *const NSImageView as *mut NSImageView;
                let _: () = msg_send![&content, addSubview: iv_ptr];
                let _: () = msg_send![&window, makeKeyAndOrderFront: std::ptr::null::<objc2::runtime::AnyObject>()];

                PIN_WINDOWS.lock().unwrap().push(SendWindow(window));
                log::info!("Pin window created at ({},{}) {}x{}", x, y, width, height);
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::MacPinWindow;
