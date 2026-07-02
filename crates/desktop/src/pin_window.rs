/// 跨平台贴图窗口抽象。
pub trait PinWindow {
    fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64);
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::Mutex;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, sel, AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSColor, NSEvent, NSImage, NSImageView, NSMenu, NSMenuItem, NSWindow,
    };
    use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString};

    struct SendWindow(#[allow(dead_code)] Retained<PinNSWindow>);
    unsafe impl Send for SendWindow {}
    unsafe impl Sync for SendWindow {}

    static PIN_WINDOWS: Mutex<Vec<SendWindow>> = Mutex::new(Vec::new());

    define_class!(
        #[unsafe(super(NSWindow))]
        struct PinNSWindow;

        impl PinNSWindow {
            #[unsafe(method(scrollWheel:))]
            fn scroll_wheel(&self, event: &NSEvent) {
                let delta_y = event.scrollingDeltaY();
                if delta_y == 0.0 { return; }
                let frame = self.frame();
                let sc = 1.0 + delta_y * 0.01;
                let new_w = (frame.size.width * sc).max(20.0).min(10000.0);
                let new_h = (frame.size.height * sc).max(20.0).min(10000.0);
                let mouse_in_win = event.locationInWindow();
                let ratio_x = if frame.size.width > 0.0 { mouse_in_win.x / frame.size.width } else { 0.5 };
                let ratio_y = if frame.size.height > 0.0 { mouse_in_win.y / frame.size.height } else { 0.5 };
                let new_x = frame.origin.x + mouse_in_win.x - ratio_x * new_w;
                let new_y = frame.origin.y + mouse_in_win.y - ratio_y * new_h;
                let new_frame = NSRect::new(NSPoint::new(new_x, new_y), NSSize::new(new_w, new_h));
                self.setFrame_display(new_frame, true);
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, event: &NSEvent) {
                let mtm = MainThreadMarker::new().expect("must be on main thread");
                unsafe {
                    let menu: Retained<NSMenu> = msg_send![NSMenu::alloc(mtm), init];
                    let title = NSString::from_str("关闭");
                    let empty = NSString::new();
                    let item: Retained<NSMenuItem> = msg_send![
                        NSMenuItem::alloc(mtm),
                        initWithTitle: &*title,
                        action: Some(sel!(close)),
                        keyEquivalent: &*empty
                    ];
                    item.setTarget(Some(self));
                    menu.addItem(&item);
                    if let Some(content) = self.contentView() {
                        NSMenu::popUpContextMenu_withEvent_forView(&menu, event, &content);
                    }
                    // 右键菜单关闭后清理已关闭的窗口引用（防泄漏）
                    // 延迟 0.1s 执行，等 NSWindow.close() 完成
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        super::macos::cleanup_closed_pin_windows();
                    });
                }
            }
        }
    );

    define_class!(
        #[unsafe(super(NSImageView))]
        struct PinNSImageView;

        impl PinNSImageView {
            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, event: &NSEvent) {
                if let Some(window) = self.window() {
                    window.performWindowDragWithEvent(event);
                }
            }
        }
    );

    pub struct MacPinWindow;

    impl super::PinWindow for MacPinWindow {
        fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) {
            unsafe {
                let ns_data = NSData::with_bytes(png_data);
                let ns_data_ptr = &*ns_data as *const NSData as *mut NSData;
                let image: Option<Retained<NSImage>> = msg_send![
                    NSImage::alloc(),
                    initWithData: ns_data_ptr
                ];
                let image = match image {
                    Some(img) => img,
                    None => {
                        log::error!("Pin window: failed to init NSImage (invalid PNG data {} bytes)", png_data.len());
                        return;
                    }
                };

                let mtm = MainThreadMarker::new().expect("must be main thread");
                let iv_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
                let image_view: Retained<PinNSImageView> = msg_send![
                    PinNSImageView::alloc(mtm),
                    initWithFrame: iv_frame
                ];
                image_view.setImage(Some(&image));

                let win_frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
                let window: Retained<PinNSWindow> = msg_send![
                    PinNSWindow::alloc(mtm),
                    initWithContentRect: win_frame,
                    styleMask: 0u64,
                    backing: 2u64,
                    defer: false
                ];

                window.setLevel(3);
                window.setHasShadow(true);
                window.setOpaque(false);
                let clear = NSColor::clearColor();
                window.setBackgroundColor(Some(&clear));

                let content = window.contentView().expect("window must have content view");
                content.addSubview(&image_view);
                image_view.setAutoresizingMask(NSAutoresizingMaskOptions(18));

                window.makeKeyAndOrderFront(None);

                PIN_WINDOWS.lock().unwrap().push(SendWindow(window));
                log::info!("Pin window created at ({},{}) {}x{}", x, y, width, height);
            }
        }
    }

    /// 关闭所有贴图窗口并清理引用（防 NSWindow 泄漏）。
    pub fn close_all_pin_windows() {
        let mut windows = PIN_WINDOWS.lock().unwrap();
        if let Some(mtm) = MainThreadMarker::new() {
            for w in windows.drain(..) {
                unsafe {
                    let _: () = msg_send![&w.0, close];
                }
            }
        } else {
            windows.clear();
        }
    }

    /// 清理已关闭的贴图窗口引用（右键关闭后调用）。
    pub fn cleanup_closed_pin_windows() {
        let mut windows = PIN_WINDOWS.lock().unwrap();
        windows.retain(|w| {
            let is_closed: bool = unsafe { msg_send![&w.0, isVisible] };
            is_closed
        });
    }
}

#[cfg(target_os = "macos")]
pub use macos::MacPinWindow;
