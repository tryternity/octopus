use std::cell::Cell;
use std::sync::Mutex;
use objc2::rc::Retained;
use objc2::{define_class, msg_send, class, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSColor, NSEvent, NSView, NSWindow, NSApplication, NSBezierPath,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// Wrapper to store Retained<NSScrollOverlayWindow> in static (it's !Send).
struct SendWindow(#[allow(dead_code)] Retained<NSScrollOverlayWindow>);
unsafe impl Send for SendWindow {}
unsafe impl Sync for SendWindow {}

/// 全局状态：选区确定后通知录制线程。
struct GlobalState {
    /// 选区确定后的回调
    on_complete: Option<Box<dyn FnOnce(Vec<u8>) + Send + 'static>>,
    /// 所有覆盖窗口的引用（防止 ARC 释放）
    windows: Vec<SendWindow>,
    /// 主屏选区所在的窗口的 windowNumber
    main_window_number: u32,
}

static STATE: Mutex<Option<GlobalState>> = Mutex::new(None);

/// 启动覆盖窗口（在主线程调用）。
pub fn start_overlay(on_complete: Box<dyn FnOnce(Vec<u8>) + Send + 'static>) {
    unsafe { start_overlay_inner(on_complete); }
}

unsafe fn start_overlay_inner(on_complete: Box<dyn FnOnce(Vec<u8>) + Send + 'static>) {
    // 必须在主线程执行
    let mtm = MainThreadMarker::new().expect("must be on main thread");

    // 获取所有屏幕
    let monitors = get_monitors(&mtm);

    // 为每个屏幕创建覆盖窗口
    let mut windows = Vec::new();
    let mut main_window_number = 0u32;

    for (i, mon) in monitors.iter().enumerate() {
        let frame = NSRect::new(
            NSPoint::new(mon.x, mon.y),
            NSSize::new(mon.width, mon.height),
        );

        let window: Retained<NSScrollOverlayWindow> = msg_send![
            NSScrollOverlayWindow::alloc(mtm),
            initWithContentRect: frame,
            styleMask: 0u64, // borderless
            backing: 2u64,   // buffered
            defer: false,
        ];

        let _: () = msg_send![&window, setLevel: 3i64]; // floating
        let _: () = msg_send![&window, setOpaque: false];
        let _: () = msg_send![&window, setReleasedWhenClosed: false];
        let clear: Retained<NSColor> = msg_send![class!(NSColor), clearColor];
        let clear_ptr = (&*clear) as *const NSColor as *mut NSColor;
        let _: () = msg_send![&window, setBackgroundColor: clear_ptr];
        let _: () = msg_send![&window, setHasShadow: false];

        // 获取 windowNumber
        let win_num: isize = msg_send![&window, windowNumber];
        if i == 0 {
            main_window_number = win_num as u32;
        }

        // 创建 OverlayView 作为 contentView
        let view_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(mon.width, mon.height));
        let view: Retained<NSScrollOverlayView> = msg_send![
            NSScrollOverlayView::alloc(mtm),
            initWithFrame: view_frame,
        ];
        let view_ptr = Retained::as_ptr(&view) as *mut NSView;
        let _: () = msg_send![&window, setContentView: view_ptr];

        let _: () = msg_send![&window, makeKeyAndOrderFront: std::ptr::null::<objc2::runtime::AnyObject>()];

        windows.push(SendWindow(window));
    }

    // 激活 app 让覆盖窗口获得焦点
    let app: Retained<NSApplication> = NSApplication::sharedApplication(mtm);
    let _: () = msg_send![&app, activateIgnoringOtherApps: true];

    *STATE.lock().unwrap() = Some(GlobalState {
        on_complete: Some(on_complete),
        windows,
        main_window_number,
    });
}

unsafe fn get_monitors(_mtm: &MainThreadMarker) -> Vec<MonitorInfo> {
    use objc2::{class, msg_send, runtime::AnyObject};
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        let count: usize = msg_send![screens, count];
        let primary_h = {
            let primary: *mut AnyObject = msg_send![screens, objectAtIndex: 0];
            let frame: objc2_foundation::NSRect = msg_send![primary, frame];
            frame.size.height as f64
        };
        let mut result = Vec::new();
        for i in 0..count {
            let screen: *mut AnyObject = msg_send![screens, objectAtIndex: i];
            let frame: objc2_foundation::NSRect = msg_send![screen, frame];
            result.push(MonitorInfo {
                x: frame.origin.x,
                y: frame.origin.y,
                width: frame.size.width,
                height: frame.size.height,
                scale_factor: 2.0, // TODO: get actual scale
                _primary_h: primary_h,
            });
        }
    result
}

struct MonitorInfo {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale_factor: f64,
    _primary_h: f64,
}

// ── NSWindow 子类 ──

#[derive(Default)]
struct WindowIvars;

define_class!(
    #[unsafe(super(NSWindow))]
    struct NSScrollOverlayWindow;

    impl NSScrollOverlayWindow {
        #[unsafe(method(canBecomeKey))]
        fn can_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }
    }
);

// ── NSView 子类（选区拉框 + 录制状态绘制）──

#[derive(Default)]
struct ViewIvars {
    state: Cell<u8>,       // 0=idle, 1=selecting, 2=recording
    start_x: Cell<f64>,
    start_y: Cell<f64>,
    sel_x: Cell<f64>,
    sel_y: Cell<f64>,
    sel_w: Cell<f64>,
    sel_h: Cell<f64>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[ivars = ViewIvars]
    struct NSScrollOverlayView;

    impl NSScrollOverlayView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let loc: NSPoint = unsafe { msg_send![event, locationInWindow] };
            self.ivars().state.set(1);
            self.ivars().start_x.set(loc.x);
            self.ivars().start_y.set(loc.y);
            self.ivars().sel_x.set(loc.x);
            self.ivars().sel_y.set(loc.y);
            self.ivars().sel_w.set(0.0);
            self.ivars().sel_h.set(0.0);
            unsafe { let _: () = msg_send![self, setNeedsDisplay: true]; }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if self.ivars().state.get() != 1 { return; }
            let loc: NSPoint = unsafe { msg_send![event, locationInWindow] };
            let sx = self.ivars().start_x.get();
            let sy = self.ivars().start_y.get();
            let x = loc.x.min(sx);
            let y = loc.y.min(sy);
            let w = (loc.x - sx).abs();
            let h = (loc.y - sy).abs();
            self.ivars().sel_x.set(x);
            self.ivars().sel_y.set(y);
            self.ivars().sel_w.set(w);
            self.ivars().sel_h.set(h);
            unsafe { let _: () = msg_send![self, setNeedsDisplay: true]; }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if self.ivars().state.get() != 1 { return; }
            let w = self.ivars().sel_w.get();
            let h = self.ivars().sel_h.get();
            if w < 10.0 || h < 10.0 {
                // 选区太小，取消
                self.ivars().state.set(0);
                unsafe { let _: () = msg_send![self, setNeedsDisplay: true]; }
                return;
            }
            // 选区确定 → 开始录制
            self.ivars().state.set(2);
            unsafe { let _: () = msg_send![self, setNeedsDisplay: true]; }

            // 在后台线程启动录制
            let sel_x = self.ivars().sel_x.get();
            let sel_y = self.ivars().sel_y.get();
            let sel_w = self.ivars().sel_w.get();
            let sel_h = self.ivars().sel_h.get();

            // 获取窗口 frame 用于坐标转换
            let win_frame: NSRect = unsafe { msg_send![self, frame] };
            let win: Retained<NSWindow> = unsafe { msg_send![self, window] };
            let win_frame_global: NSRect = unsafe { msg_send![&win, frame] };

            // 选区全局 Quartz 坐标（Cocoa 左下 → Quartz 左上）
            let primary_h = super::helpers::get_primary_screen_height();
            let global_x = win_frame_global.origin.x + sel_x;
            // Cocoa Y 是从窗口底部算的，sel_y 是从 NSView 底部算的
            // 窗口 frame.origin.y 是窗口底部全局坐标
            // NSView 填满窗口，所以 sel_y 直接加到 window origin
            let global_cocoa_y = win_frame_global.origin.y + sel_y;
            let global_y = primary_h - global_cocoa_y - sel_h;

            // 获取 windowNumber
            let win_num: isize = unsafe { msg_send![&win, windowNumber] };

            start_recording_thread(global_x, global_y, sel_w, sel_h, win_num as u32);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let key_code: u16 = unsafe { msg_send![event, keyCode] };
            if key_code == 53 {
                // ESC
                crate::stop();
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty_rect: NSRect) {
            unsafe {
                let bounds: NSRect = msg_send![self, bounds];
                let state = self.ivars().state.get();

                if state == 2 {
                    // recording: 选区外遮罩 + 选区内透明 + 绿色边框
                    draw_overlay(self, &bounds);
                    return;
                }

                // idle / selecting: 画暗遮罩 + 选区透明 + 绿色边框
                draw_overlay(self, &bounds);
            }
        }
    }
);

unsafe fn draw_green_border(view: &NSScrollOverlayView, _bounds: &NSRect) {
    let sel_x = view.ivars().sel_x.get();
    let sel_y = view.ivars().sel_y.get();
    let sel_w = view.ivars().sel_w.get();
    let sel_h = view.ivars().sel_h.get();
    if sel_w < 1.0 || sel_h < 1.0 { return; }

    let rect = NSRect::new(
        NSPoint::new(sel_x, sel_y),
        NSSize::new(sel_w, sel_h),
    );
    let green: Retained<NSColor> = msg_send![class!(NSColor), colorWithRed: 0.13f64 green: 0.77f64 blue: 0.37f64 alpha: 1.0f64];
    let green_ptr = (&*green) as *const NSColor as *mut NSColor;
    let _: () = msg_send![green_ptr, set];
    let _: () = msg_send![class!(NSBezierPath), strokeRect: rect];
}

unsafe fn draw_overlay(view: &NSScrollOverlayView, bounds: &NSRect) {
    // 全屏暗遮罩
    let dark: Retained<NSColor> = msg_send![class!(NSColor), colorWithCalibratedRed: 0.0f64 green: 0.0f64 blue: 0.0f64 alpha: 0.5f64];
    let dark_ptr = (&*dark) as *const NSColor as *mut NSColor;
    let _: () = msg_send![dark_ptr, set];
    let _: () = msg_send![class!(NSBezierPath), fillRect: *bounds];

    // 选区内透明（清除遮罩）
    let sel_x = view.ivars().sel_x.get();
    let sel_y = view.ivars().sel_y.get();
    let sel_w = view.ivars().sel_w.get();
    let sel_h = view.ivars().sel_h.get();

    if sel_w > 1.0 && sel_h > 1.0 {
        let sel_rect = NSRect::new(
            NSPoint::new(sel_x, sel_y),
            NSSize::new(sel_w, sel_h),
        );
        // 选区内透明：用 NSCompositeClear 清除
        let ctx: *mut objc2::runtime::AnyObject = msg_send![class!(NSGraphicsContext), currentContext];
        let _: () = msg_send![ctx, setCompositingOperation: 0u64]; // NSCompositingOperationClear = 0
        let _: () = msg_send![class!(NSBezierPath), fillRect: sel_rect];
        let _: () = msg_send![ctx, setCompositingOperation: 2u64]; // NSCompositingOperationSourceOver = 2
        // 绿色边框
        let green: Retained<NSColor> = msg_send![class!(NSColor), colorWithRed: 0.13f64 green: 0.77f64 blue: 0.37f64 alpha: 1.0f64];
        let green_ptr = (&*green) as *const NSColor as *mut NSColor;
        let _: () = msg_send![green_ptr, set];
        let _: () = msg_send![class!(NSBezierPath), strokeRect: sel_rect];
    }
}

/// 在后台线程启动录制循环。
fn start_recording_thread(global_x: f64, global_y: f64, sel_w: f64, sel_h: f64, exclude_wid: u32) {
    crate::set_recording(true);

    // 创建预览窗口（选区右下角右侧）
    let preview_x = global_x + sel_w + 12.0;
    let preview_y = global_y;
    super::preview_window::create_preview(preview_x, preview_y);

    // 设置按钮回调（1=save, 2=copy, 3=cancel）
    super::preview_window::set_button_callback(Box::new(|action| {
        match action {
            1 | 2 => {
                // save or copy → stop recording (backend handles on_complete)
                crate::stop();
            }
            3 => {
                // cancel → stop without saving
                // 设标志让 on_complete 不入库
                crate::set_cancelled(true);
                crate::stop();
            }
            _ => {}
        }
    }));

    // 设置所有覆盖窗口为滚轮穿透
    {
        let state = STATE.lock().unwrap();
        if let Some(ref s) = *state {
            for win in &s.windows {
                let w = &win.0;
                let _: () = unsafe { msg_send![w, setIgnoresMouseEvents: true] };
            }
        }
    }

    // 激活选区下方的应用并获取目标窗口 ID
    let center_x = global_x + sel_w / 2.0;
    let center_y = global_y + sel_h / 2.0;
    let mut target_wid = None;
    if let Some(info) = super::helpers::get_target_window_at_point(center_x, center_y) {
        super::helpers::activate_app_by_pid(info.pid);
        target_wid = Some(info.window_id);
    }

    let on_complete = {
        let mut state = STATE.lock().unwrap();
        state.as_mut().and_then(|s| s.on_complete.take())
    };

    let on_complete = match on_complete {
        Some(cb) => cb,
        None => return,
    };

    std::thread::spawn(move || {
        // 等待应用激活
        std::thread::sleep(std::time::Duration::from_millis(200));

        // 首帧
        let first = if let Some(wid) = target_wid {
            match super::capture::capture_region_window(wid, global_x, global_y, sel_w, sel_h) {
                Ok(img) => img,
                Err(e) => {
                    log::warn!("[scroll-capture] first frame target capture failed: {}, falling back...", e);
                    match super::capture::capture_region_excluding(exclude_wid, global_x, global_y, sel_w, sel_h) {
                        Ok(img) => img,
                        Err(err) => {
                            log::error!("[scroll-capture] fallback first frame failed: {}", err);
                            cleanup();
                            return;
                        }
                    }
                }
            }
        } else {
            match super::capture::capture_region_excluding(
                exclude_wid, global_x, global_y, sel_w, sel_h,
            ) {
                Ok(img) => img,
                Err(e) => {
                    log::error!("[scroll-capture] first frame failed: {}", e);
                    cleanup();
                    return;
                }
            }
        };

        let mut config = crate::stitch::StitchConfig::default();
        config.min_confidence = 0.25;
        let mut stitcher = crate::stitch::Stitcher::new(first, config);

        let frame_interval = std::time::Duration::from_millis(30); // 目标帧率：30ms (约 33fps)
        let mut last_frame = None;

        loop {
            let start_time = std::time::Instant::now();
            if !crate::is_recording() {
                break;
            }
            let frame_res = if let Some(wid) = target_wid {
                super::capture::capture_region_window(wid, global_x, global_y, sel_w, sel_h)
            } else {
                super::capture::capture_region_excluding(exclude_wid, global_x, global_y, sel_w, sel_h)
            };

            match frame_res {
                Ok(frame) => {
                    let (fw, fh) = (frame.width(), frame.height());
                    let p = frame.get_pixel(fw / 2, fh / 2);
                    log::info!("[scroll-capture] captured frame {}x{} center pixel=({},{},{})", fw, fh, p[0], p[1], p[2]);
                    
                    // 保存前 30 帧用于排查捕获画面是否正确
                    static FRAME_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                    let count = FRAME_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if count < 30 {
                        let dir = "/Users/wudarui/workspace/agent/octopus/debug_frames";
                        let _ = std::fs::create_dir_all(dir);
                        let path = format!("{}/frame_{}.png", dir, count);
                        if let Err(e) = frame.save(&path) {
                            log::warn!("[scroll-capture] failed to save debug frame: {}", e);
                        } else {
                            log::info!("[scroll-capture] saved debug frame to {}", path);
                        }
                    }
                    
                    match stitcher.process_frame(&frame) {
                        Ok(true) => {
                            log::info!("[scroll-capture] stitched! canvas_h={}", stitcher.height());
                            // 更新预览
                            let canvas = stitcher.canvas();
                            let preview_w = 184u32;
                            let preview_h = (preview_w * canvas.height() / canvas.width()).min(200);
                            let preview_img = image::imageops::resize(canvas, preview_w, preview_h, image::imageops::FilterType::CatmullRom);
                            let mut preview_png = Vec::new();
                            use image::ImageEncoder;
                            let enc = image::codecs::png::PngEncoder::new(&mut preview_png);
                            let _ = enc.write_image(preview_img.as_raw(), preview_w, preview_h, image::ExtendedColorType::Rgba8);
                            super::preview_window::update_preview(preview_png, stitcher.height());
                        }
                        Ok(false) => {
                            log::info!("[scroll-capture] Frame skipped (not stitched)");
                        }
                        Err(e) => {
                            log::error!("[scroll-capture] Stitcher error: {}", e);
                        }
                    }
                    last_frame = Some(frame);
                }
                Err(e) => {
                    log::warn!("[scroll-capture] capture failed: {}", e);
                }
            }

            let elapsed = start_time.elapsed();
            if elapsed < frame_interval {
                std::thread::sleep(frame_interval - elapsed);
            }
        }

        // Finalize
        if let Some(ref lf) = last_frame {
            let _ = stitcher.finalize(lf);
        }

        // 关闭预览窗口
        super::preview_window::close_preview();

        if crate::is_cancelled() {
            log::info!("[scroll-capture] cancelled, skipping on_complete");
            cleanup();
            return;
        }

        let canvas = stitcher.canvas().clone();
        let mut png_bytes = Vec::new();
        use image::ImageEncoder;
        let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
        let _ = encoder.write_image(
            canvas.as_raw(),
            canvas.width(),
            canvas.height(),
            image::ExtendedColorType::Rgba8,
        );

        log::info!("[scroll-capture] recording complete, {} bytes PNG", png_bytes.len());
        on_complete(png_bytes);
        cleanup();
    });
}

extern "C" {
    static _dispatch_main_q: std::ffi::c_void;
    fn dispatch_async_f(
        queue: *const std::ffi::c_void,
        context: *mut std::ffi::c_void,
        work: extern "C" fn(*mut std::ffi::c_void),
    );
}

extern "C" fn cleanup_on_main_thread(ctx: *mut std::ffi::c_void) {
    let state_ptr = ctx as *mut GlobalState;
    unsafe {
        let state = Box::from_raw(state_ptr);
        for win in &state.windows {
            let w = &win.0;
            // 隐藏窗口
            let _: () = msg_send![w, orderOut: std::ptr::null::<objc2::runtime::AnyObject>()];
            // 关闭窗口 (因为设置了 setReleasedWhenClosed: false, Rust 的 Retained 在这里 drop 时会安全地释放其内存)
            let _: () = msg_send![w, close];
        }
    }
}

/// 清理：在主线程销毁所有覆盖窗口。
fn cleanup() {
    let mut state = STATE.lock().unwrap();
    if let Some(s) = state.take() {
        let boxed = Box::new(s);
        let raw_ptr = Box::into_raw(boxed);
        unsafe {
            let main_q = &_dispatch_main_q as *const std::ffi::c_void;
            dispatch_async_f(main_q, raw_ptr as *mut std::ffi::c_void, cleanup_on_main_thread);
        }
    }
}
