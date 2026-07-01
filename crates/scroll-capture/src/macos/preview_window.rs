use std::cell::Cell;
use std::sync::Mutex;
use objc2::rc::Retained;
use objc2::{define_class, msg_send, class, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSColor, NSEvent, NSView, NSWindow, NSImage, NSBezierPath,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSData};

struct SendPreviewWindow(#[allow(dead_code)] Retained<NSPreviewWindow>);
unsafe impl Send for SendPreviewWindow {}
unsafe impl Sync for SendPreviewWindow {}

static PREVIEW_WINDOW: Mutex<Option<SendPreviewWindow>> = Mutex::new(None);
static PENDING_PREVIEW: Mutex<Option<(Vec<u8>, u32)>> = Mutex::new(None);
static CURRENT_PNG: Mutex<Option<Vec<u8>>> = Mutex::new(None);
static CURRENT_HEIGHT: Mutex<Option<u32>> = Mutex::new(None);
static BUTTON_CALLBACK: Mutex<Option<Box<dyn Fn(u8) + Send>>> = Mutex::new(None);

pub fn set_button_callback(cb: Box<dyn Fn(u8) + Send>) {
    *BUTTON_CALLBACK.lock().unwrap() = Some(cb);
}

pub fn create_preview(x: f64, y: f64) {
    unsafe {
        let mtm = MainThreadMarker::new().expect("main thread");
        let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(200.0, 300.0));
        let window: Retained<NSPreviewWindow> = msg_send![
            NSPreviewWindow::alloc(mtm),
            initWithContentRect: frame,
            styleMask: 0u64,
            backing: 2u64,
            defer: false,
        ];
        let _: () = msg_send![&window, setLevel: 3i64];
        let _: () = msg_send![&window, setHasShadow: true];
        let _: () = msg_send![&window, setOpaque: false];
        let clear: Retained<NSColor> = msg_send![class!(NSColor), clearColor];
        let clear_ptr = (&*clear) as *const NSColor as *mut NSColor;
        let _: () = msg_send![&window, setBackgroundColor: clear_ptr];

        let view_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 300.0));
        let view: Retained<NSPreviewView> = msg_send![
            NSPreviewView::alloc(mtm),
            initWithFrame: view_frame,
        ];
        let view_ptr = Retained::as_ptr(&view) as *mut NSView;
        let _: () = msg_send![&window, setContentView: view_ptr];
        let _: () = msg_send![&window, makeKeyAndOrderFront: std::ptr::null::<objc2::runtime::AnyObject>()];

        *PREVIEW_WINDOW.lock().unwrap() = Some(SendPreviewWindow(window));
    }
}

pub fn update_preview(png_data: Vec<u8>, height: u32) {
    *PENDING_PREVIEW.lock().unwrap() = Some((png_data, height));
    let window_opt = PREVIEW_WINDOW.lock().unwrap();
    if let Some(ref sw) = *window_opt {
        let win = &sw.0;
        unsafe {
            let content: Retained<NSView> = msg_send![win, contentView];
            let _: () = msg_send![&content, setNeedsDisplay: true];
        }
    }
}

pub fn close_preview() {
    let win_opt = PREVIEW_WINDOW.lock().unwrap().take();
    if let Some(sw) = win_opt {
        let win = &sw.0;
        unsafe {
            let _: () = msg_send![win, close];
        }
    }
}

fn take_pending_preview() -> Option<(Vec<u8>, u32)> {
    PENDING_PREVIEW.lock().unwrap().take()
}

define_class!(
    #[unsafe(super(NSWindow))]
    struct NSPreviewWindow;

    impl NSPreviewWindow {
        #[unsafe(method(canBecomeKey))]
        fn can_become_key(&self) -> bool { true }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { false }
    }
);

#[derive(Default)]
struct PreviewIvars;

define_class!(
    #[unsafe(super(NSView))]
    struct NSPreviewView;

    impl NSPreviewView {
        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool { false }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            unsafe { draw_content(self); }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let loc: NSPoint = unsafe { msg_send![event, locationInWindow] };
            let bounds: NSRect = unsafe { msg_send![self, bounds] };
            let btn_y = 4.0;
            let btn_h = 24.0;
            let gap = 4.0;
            let total_w = bounds.size.width - 16.0;
            let btn_w = (total_w - gap * 2.0) / 3.0;
            if loc.y >= btn_y && loc.y <= btn_y + btn_h {
                let x0 = 8.0;
                let action = if loc.x >= x0 && loc.x < x0 + btn_w { 1 }
                    else if loc.x >= x0 + btn_w + gap && loc.x < x0 + btn_w * 2.0 + gap { 2 }
                    else if loc.x >= x0 + (btn_w + gap) * 2.0 && loc.x < x0 + total_w + 8.0 { 3 }
                    else { 0 };
                if action > 0 {
                    if let Some(ref cb) = *BUTTON_CALLBACK.lock().unwrap() { cb(action); }
                }
            }
        }
    }
);

unsafe fn draw_content(view: &NSPreviewView) {
    let bounds: NSRect = msg_send![view, bounds];

    // 清除
    extern "C" { fn CGContextClearRect(ctx: core_graphics::sys::CGContextRef, rect: core_graphics::geometry::CGRect); }
    let ctx: *mut objc2::runtime::AnyObject = msg_send![class!(NSGraphicsContext), currentContext];
    if !ctx.is_null() {
        let cg_ctx: *mut objc2::runtime::AnyObject = msg_send![ctx, CGContext];
        if !cg_ctx.is_null() {
            use core_graphics::geometry::{CGRect, CGPoint, CGSize};
            CGContextClearRect(cg_ctx as core_graphics::sys::CGContextRef, CGRect {
                origin: CGPoint { x: bounds.origin.x, y: bounds.origin.y },
                size: CGSize { width: bounds.size.width, height: bounds.size.height },
            });
        }
    }

    // 背景
    let bg: Retained<NSColor> = msg_send![class!(NSColor), colorWithCalibratedRed: 0.06f64 green: 0.06f64 blue: 0.07f64 alpha: 0.92f64];
    let bg_ptr = (&*bg) as *const NSColor as *mut NSColor;
    let _: () = msg_send![bg_ptr, set];
    let _: () = msg_send![class!(NSBezierPath), fillRect: bounds];

    // 取 pending preview
    if let Some((png, h)) = take_pending_preview() {
        *CURRENT_PNG.lock().unwrap() = Some(png);
        *CURRENT_HEIGHT.lock().unwrap() = Some(h);
    }

    // 状态条
    let h = CURRENT_HEIGHT.lock().unwrap().unwrap_or(0);
    let amber: Retained<NSColor> = msg_send![class!(NSColor), colorWithRed: 0.96f64 green: 0.62f64 blue: 0.04f64 alpha: 1.0f64];
    let amber_ptr = (&*amber) as *const NSColor as *mut NSColor;
    let status_rect = NSRect::new(NSPoint::new(8.0, bounds.size.height - 20.0), NSSize::new(bounds.size.width - 16.0, 12.0));
    let _ = amber_ptr;

    // 预览图
    if let Some(ref png) = *CURRENT_PNG.lock().unwrap() {
        let ns_data = NSData::with_bytes(png);
        let ns_data_ptr = Retained::as_ptr(&ns_data) as *mut NSData;
        let image: Option<Retained<NSImage>> = msg_send![msg_send![class!(NSImage), alloc], initWithData: ns_data_ptr];
        if let Some(ref img) = image {
            let preview_area = NSRect::new(
                NSPoint::new(8.0, 36.0),
                NSSize::new(bounds.size.width - 16.0, bounds.size.height - 72.0),
            );
            let from_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
            let _: () = msg_send![img, drawInRect: preview_area, fromRect: from_rect, operation: 2i64, fraction: 1.0f64];
        }
    }

    // 按钮
    let btn_y = 4.0;
    let btn_h = 24.0;
    let gap = 4.0;
    let total_w = bounds.size.width - 16.0;
    let btn_w = (total_w - gap * 2.0) / 3.0;
    fill_btn(8.0, btn_y, btn_w, btn_h, 0.23, 0.51, 0.96);
    fill_btn(8.0 + btn_w + gap, btn_y, btn_w, btn_h, 0.13, 0.77, 0.37);
    stroke_btn(8.0 + (btn_w + gap) * 2.0, btn_y, btn_w, btn_h);
}

unsafe fn fill_btn(x: f64, y: f64, w: f64, h: f64, r: f64, g: f64, b: f64) {
    let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
    let color: Retained<NSColor> = msg_send![class!(NSColor), colorWithRed: r green: g blue: b alpha: 1.0f64];
    let color_ptr = (&*color) as *const NSColor as *mut NSColor;
    let _: () = msg_send![color_ptr, set];
    let _: () = msg_send![class!(NSBezierPath), fillRect: rect];
    // 圆角 6
    let _: () = msg_send![color_ptr, set];
}

unsafe fn stroke_btn(x: f64, y: f64, w: f64, h: f64) {
    let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
    let border: Retained<NSColor> = msg_send![class!(NSColor), colorWithCalibratedWhite: 1.0f64 alpha: 0.15f64];
    let border_ptr = (&*border) as *const NSColor as *mut NSColor;
    let _: () = msg_send![border_ptr, set];
    let _: () = msg_send![class!(NSBezierPath), strokeRect: rect];
}
