/// 跨平台贴图窗口抽象。
pub trait PinWindow {
    fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64);
}

#[cfg(target_os = "macos")]
mod macos {
    use parking_lot::Mutex;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, sel, AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSColor, NSEvent, NSImage, NSImageView,
        NSWindow, NSTrackingArea, NSTrackingAreaOptions,
    };
    use objc2_foundation::{NSData, NSPoint, NSRect, NSSize};

    struct SendWindow(#[allow(dead_code)] Retained<PinNSWindow>);
    unsafe impl Send for SendWindow {}
    unsafe impl Sync for SendWindow {}

    static PIN_WINDOWS: Mutex<Vec<SendWindow>> = Mutex::new(Vec::new());

    const CLOSE_BUTTON_TAG: i64 = 99991;
    const CLOSE_BTN_SIZE: f64 = 20.0;
    const CLOSE_BTN_MARGIN: f64 = 2.0;

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
                let new_w = (frame.size.width * sc).clamp(20.0, 10000.0);
                let new_h = (frame.size.height * sc).clamp(20.0, 10000.0);
                let mouse_in_win = event.locationInWindow();
                let ratio_x = if frame.size.width > 0.0 { mouse_in_win.x / frame.size.width } else { 0.5 };
                let ratio_y = if frame.size.height > 0.0 { mouse_in_win.y / frame.size.height } else { 0.5 };
                let new_x = frame.origin.x + mouse_in_win.x - ratio_x * new_w;
                let new_y = frame.origin.y + mouse_in_win.y - ratio_y * new_h;
                let new_frame = NSRect::new(NSPoint::new(new_x, new_y), NSSize::new(new_w, new_h));
                self.setFrame_display(new_frame, true);
            }
            #[unsafe(method(cleanup))]
            fn cleanup(&self) {
                super::macos::cleanup_closed_pin_windows();
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

            #[unsafe(method(mouseEntered:))]
            fn mouse_entered(&self, _event: &NSEvent) {
                unsafe {
                    let win = match self.window() { Some(w) => w, None => return };
                    if let Some(content) = win.contentView() {
                        let btn: Option<Retained<NSImageView>> = msg_send![&content, viewWithTag: CLOSE_BUTTON_TAG];
                        if let Some(btn) = btn {
                            btn.setHidden(false);
                        }
                    }
                }
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _event: &NSEvent) {
                unsafe {
                    let win = match self.window() { Some(w) => w, None => return };
                    if let Some(content) = win.contentView() {
                        let btn: Option<Retained<NSImageView>> = msg_send![&content, viewWithTag: CLOSE_BUTTON_TAG];
                        if let Some(btn) = btn {
                            btn.setHidden(true);
                        }
                    }
                }
            }
        }
    );

    // 关闭按钮视图——与 PinNSImageView 完全相同的模式（NSImageView 子类 + mouseDown 重写）
    define_class!(
        #[unsafe(super(NSImageView))]
        struct PinCloseBtnView;

        impl PinCloseBtnView {
            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _event: &NSEvent) {
                if let Some(window) = self.window() {
                    window.close();
                    unsafe {
                        let cleanup_sel = sel!(cleanup);
                        let _: () = msg_send![
                            &window,
                            performSelector: cleanup_sel,
                            withObject: None as Option<&objc2::runtime::AnyObject>,
                            afterDelay: 0.1f64
                        ];
                    }
                }
            }
        }
    );

    /// 生成关闭按钮 PNG（40×40，红圆 + 白×，retina 2×）
    fn create_close_button_png() -> Vec<u8> {
        let size = 40u32;
        let mut img = image::RgbaImage::new(size, size);
        let center = (size as f64 - 1.0) / 2.0;
        let radius = center;
        for y in 0..size {
            for x in 0..size {
                let dx = x as f64 - center;
                let dy = y as f64 - center;
                let dist = (dx * dx + dy * dy).sqrt();
                let mut px = [0u8, 0, 0, 0];
                if dist <= radius {
                    px = [0xD8, 0x2E, 0x2E, 0xEB];
                }
                let nx = dx / radius.max(0.1);
                let ny = dy / radius.max(0.1);
                let x_thick = (nx.abs() - ny.abs()).abs() / 0.45;
                let x_pos = (nx.abs() + ny.abs()) / 1.1;
                if x_thick < 1.0 && x_pos < 0.95 {
                    px = [255, 255, 255, 255];
                }
                img.put_pixel(x, y, image::Rgba(px));
            }
        }
        let mut png = Vec::new();
        let _ = img.write_to(
            &mut std::io::Cursor::new(&mut png),
            image::ImageFormat::Png,
        );
        png
    }

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

                let mtm = match MainThreadMarker::new() {
                    Some(m) => m,
                    None => {
                        log::error!("Not on main thread in PinWindow::create");
                        return;
                    }
                };
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

                window.setReleasedWhenClosed(false);
                window.setLevel(3);
                window.setHasShadow(true);
                window.setOpaque(false);
                let clear = NSColor::clearColor();
                window.setBackgroundColor(Some(&clear));
                // NSTrackingArea（下方添加到 contentView）负责 hover 检测，无需 acceptsMouseMovedEvents

                if let Some(content) = window.contentView() {
                    content.addSubview(&image_view);
                    image_view.setAutoresizingMask(NSAutoresizingMaskOptions(18));

                    let tracking_options = NSTrackingAreaOptions::MouseEnteredAndExited
                        | NSTrackingAreaOptions::ActiveAlways
                        | NSTrackingAreaOptions::InVisibleRect;
                    let tracking_area: Retained<NSTrackingArea> = msg_send![
                        NSTrackingArea::alloc(),
                        initWithRect: iv_frame,
                        options: tracking_options,
                        owner: &*image_view,
                        userInfo: None::<&objc2::runtime::AnyObject>
                    ];
                    let _: () = msg_send![&*content, addTrackingArea: &*tracking_area];

                    // 关闭按钮（右上角，初始隐藏，hover 显示）
                    let btn_size = CLOSE_BTN_SIZE;
                    let btn_x = width - btn_size - CLOSE_BTN_MARGIN;
                    let btn_y = height - btn_size - CLOSE_BTN_MARGIN;
                    let btn_frame = NSRect::new(
                        NSPoint::new(btn_x, btn_y),
                        NSSize::new(btn_size, btn_size),
                    );
                    let close_btn: Retained<PinCloseBtnView> = msg_send![
                        PinCloseBtnView::alloc(mtm),
                        initWithFrame: btn_frame
                    ];
                    // 预渲染的关闭按钮图标
                    let close_png = create_close_button_png();
                    let cb_data = NSData::with_bytes(&close_png);
                    let cb_data_ptr = &*cb_data as *const NSData as *mut NSData;
                    let cb_image: Option<Retained<NSImage>> = msg_send![
                        NSImage::alloc(),
                        initWithData: cb_data_ptr
                    ];
                    if let Some(cb_img) = cb_image {
                        close_btn.setImage(Some(&cb_img));
                    }
                    let _: () = msg_send![&close_btn, setTag: CLOSE_BUTTON_TAG];
                    close_btn.setAutoresizingMask(NSAutoresizingMaskOptions(9));
                    close_btn.setHidden(true);
                    content.addSubview(&close_btn);
                } else {
                    log::error!("Window contentView is None");
                }

                window.makeKeyAndOrderFront(None);

                PIN_WINDOWS.lock().push(SendWindow(window));
                log::info!("Pin window created at ({},{}) {}x{}", x, y, width, height);
            }
        }
    }

    /// 清理已关闭的贴图窗口引用（关闭按钮点击后由 cleanup selector 延迟调用）。
    pub fn cleanup_closed_pin_windows() {
        let mut windows = PIN_WINDOWS.lock();
        windows.retain(|w| {
            let is_closed: bool = unsafe { msg_send![&w.0, isVisible] };
            is_closed
        });
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::sync::Once;
    use windows::{
        core::*,
        Win32::Foundation::*,
        Win32::UI::WindowsAndMessaging::*,
        Win32::Graphics::Gdi::*,
        Win32::System::LibraryLoader::GetModuleHandleW,
        Win32::UI::Input::KeyboardAndMouse::*,
    };

    static REGISTER_CLASS_ONCE: Once = Once::new();
    const WINDOW_CLASS_NAME: PCWSTR = w!("OctopusPinWindow");

    struct WindowState {
        original_w: i32,
        original_h: i32,
        current_zoom: f64,
        hbitmap: HBITMAP,
        hovered: bool,
        tracking_mouse: bool,
    }

    pub struct WinPinWindow;

    impl super::PinWindow for WinPinWindow {
        fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) {
            let png_vec = png_data.to_vec();
            std::thread::spawn(move || {
                if let Err(e) = create_window_blocking(&png_vec, x, y, width, height) {
                    log::error!("Failed to create Windows pin window: {}", e);
                }
            });
        }
    }

    fn create_window_blocking(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) -> Result<()> {
        let img = image::load_from_memory(png_data)
            .map_err(|e| Error::new(E_FAIL, hstring!(e.to_string())))?;
        let rgba = img.to_rgba8();
        let img_w = rgba.width() as i32;
        let img_h = rgba.height() as i32;

        let mut bgra = vec![0u8; (img_w * img_h * 4) as usize];
        for (i, pixel) in rgba.pixels().enumerate() {
            let r = pixel[0] as u32;
            let g = pixel[1] as u32;
            let b = pixel[2] as u32;
            let a = pixel[3] as u32;
            let r_p = ((r * a) / 255) as u8;
            let g_p = ((g * a) / 255) as u8;
            let b_p = ((b * a) / 255) as u8;
            let a_p = a as u8;
            bgra[i * 4] = b_p;
            bgra[i * 4 + 1] = g_p;
            bgra[i * 4 + 2] = r_p;
            bgra[i * 4 + 3] = a_p;
        }

        unsafe {
            let hinstance = GetModuleHandleW(None)?;

            REGISTER_CLASS_ONCE.call_once(|| {
                let wnd_class = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: HINSTANCE(hinstance.0),
                    lpszClassName: WINDOW_CLASS_NAME,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or(HCURSOR::default()),
                    ..Default::default()
                };
                RegisterClassW(&wnd_class);
            });

            let screen_hdc = GetDC(HWND::default());
            let dpi_x = GetDeviceCaps(screen_hdc, LOGPIXELSX);
            ReleaseDC(HWND::default(), screen_hdc);
            let scale = dpi_x as f64 / 96.0;

            let px = (x * scale) as i32;
            let py = (y * scale) as i32;
            let pw = (width * scale) as i32;
            let ph = (height * scale) as i32;

            let hdc_mem = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: img_w,
                    biHeight: -img_h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB as u32,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: Default::default(),
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let hbitmap = CreateDIBSection(
                hdc_mem,
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                HANDLE::default(),
                0,
            )?;
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
            DeleteDC(hdc_mem);

            let state = Box::new(WindowState {
                original_w: pw,
                original_h: ph,
                current_zoom: 1.0,
                hbitmap,
                hovered: false,
                tracking_mouse: false,
            });
            let state_ptr = Box::into_raw(state);

            let hwnd_res = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
                WINDOW_CLASS_NAME,
                w!("Octopus Pin Window"),
                WS_POPUP,
                px,
                py,
                pw,
                ph,
                None,
                None,
                HINSTANCE(hinstance.0),
                Some(state_ptr as *const std::ffi::c_void),
            );

            let hwnd = match hwnd_res {
                Ok(hwnd) => hwnd,
                Err(e) => {
                    let _ = Box::from_raw(state_ptr);
                    DeleteObject(HGDIOBJ(hbitmap.0));
                    return Err(e);
                }
            };

            if let Err(e) = update_layered_window_view(hwnd, state_ptr) {
                DestroyWindow(hwnd);
                return Err(e);
            }

            ShowWindow(hwnd, SW_SHOWNOACTIVATE);

            let mut msg = MSG::default();
            loop {
                let status = GetMessageW(&mut msg, None, 0, 0);
                if status.0 == 0 || status.0 == -1 {
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }

    unsafe fn update_layered_window_view(hwnd: HWND, state_ptr: *mut WindowState) -> Result<()> {
        let state = &*state_ptr;
        let zoom_w = (state.original_w as f64 * state.current_zoom) as i32;
        let zoom_h = (state.original_h as f64 * state.current_zoom) as i32;

        let hdc_screen = GetDC(HWND::default());
        let hdc_mem_dest = CreateCompatibleDC(hdc_screen);
        
        let hbm_dest = match CreateCompatibleBitmap(hdc_screen, zoom_w, zoom_h) {
            Ok(h) => h,
            Err(e) => {
                DeleteDC(hdc_mem_dest);
                ReleaseDC(HWND::default(), hdc_screen);
                return Err(e);
            }
        };
        let hold_dest = SelectObject(hdc_mem_dest, HGDIOBJ(hbm_dest.0));

        let hdc_mem_src = CreateCompatibleDC(hdc_screen);
        let hold_src = SelectObject(hdc_mem_src, HGDIOBJ(state.hbitmap.0));

        let mut bitmap: BITMAP = std::mem::zeroed();
        GetObjectW(
            HGDIOBJ(state.hbitmap.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bitmap as *mut BITMAP as *mut std::ffi::c_void),
        );

        SetStretchBltMode(hdc_mem_dest, HALFTONE);

        let run_gdi_calls = || -> Result<()> {
            SetBrushOrgEx(hdc_mem_dest, 0, 0, None)?;
            StretchBlt(
                hdc_mem_dest,
                0,
                0,
                zoom_w,
                zoom_h,
                hdc_mem_src,
                0,
                0,
                bitmap.bmWidth,
                bitmap.bmHeight,
                SRCCOPY,
            )?;

            // Draw close button overlay (red circle + white ×) at top-right when hovered
            if state.hovered {
                let btn_sz = 24i32.min(zoom_w).min(zoom_h);
                let btn_x = zoom_w - btn_sz - 2;
                let btn_y = 2;
                let red_brush = match CreateSolidBrush(COLORREF(0x003030D8)) {
                    Ok(b) => b,
                    Err(_) => { /* skip close button if GDI fails */ }
                };
                let old_brush = SelectObject(hdc_mem_dest, HGDIOBJ(red_brush.0));
                let _ = Ellipse(hdc_mem_dest, btn_x, btn_y, btn_x + btn_sz, btn_y + btn_sz);
                SelectObject(hdc_mem_dest, old_brush);
                DeleteObject(HGDIOBJ(red_brush.0));
                let white_pen = match CreatePen(0i32, 2, COLORREF(0x00FFFFFF)) {
                    Ok(p) => p,
                    Err(_) => { return Ok(()); }
                };
                let old_pen = SelectObject(hdc_mem_dest, HGDIOBJ(white_pen.0));
                let inset = btn_sz / 3;
                let _ = MoveToEx(hdc_mem_dest, btn_x + inset, btn_y + inset, None);
                let _ = LineTo(hdc_mem_dest, btn_x + btn_sz - inset, btn_y + btn_sz - inset);
                let _ = MoveToEx(hdc_mem_dest, btn_x + btn_sz - inset, btn_y + inset, None);
                let _ = LineTo(hdc_mem_dest, btn_x + inset, btn_y + btn_sz - inset);
                SelectObject(hdc_mem_dest, old_pen);
                DeleteObject(HGDIOBJ(white_pen.0));
            }

            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let ppt_src = POINT { x: 0, y: 0 };
            let psize = SIZE { cx: zoom_w, cy: zoom_h };

            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect).unwrap_or(());
            let ppt_dst = POINT { x: rect.left, y: rect.top };

            UpdateLayeredWindow(
                hwnd,
                hdc_screen,
                Some(&ppt_dst),
                Some(&psize),
                hdc_mem_dest,
                Some(&ppt_src),
                COLORREF::default(),
                Some(&blend),
                ULW_ALPHA,
            )?;
            Ok(())
        };

        let result = run_gdi_calls();

        SelectObject(hdc_mem_src, hold_src);
        DeleteDC(hdc_mem_src);

        SelectObject(hdc_mem_dest, hold_dest);
        DeleteDC(hdc_mem_dest);
        DeleteObject(HGDIOBJ(hbm_dest.0));
        
        ReleaseDC(HWND::default(), hdc_screen);

        result
    }

    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_CREATE => {
                let create_struct_ptr = lparam.0 as *const CREATESTRUCTW;
                if !create_struct_ptr.is_null() {
                    let create_struct = &*create_struct_ptr;
                    let state_ptr = create_struct.lpCreateParams as *mut WindowState;
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                // Check if click is in close button area (top-right)
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    if state.hovered {
                        let mut p = POINT::default();
                        if GetCursorPos(&mut p).as_bool() {
                            let mut rect = RECT::default();
                            if GetWindowRect(hwnd, &mut rect).as_bool() {
                                let cur_w = rect.right - rect.left;
                                let btn_sz = 24i32.min(cur_w);
                                let btn_x = rect.right - btn_sz - 2;
                                let btn_y = rect.top + 2;
                                if p.x >= btn_x && p.x <= btn_x + btn_sz
                                    && p.y >= btn_y && p.y <= btn_y + btn_sz {
                                    DestroyWindow(hwnd);
                                    return LRESULT(0);
                                }
                            }
                        }
                    }
                }
                ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, WPARAM(HTCAPTION as usize), LPARAM(0));
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    if !state.tracking_mouse {
                        state.tracking_mouse = true;
                        let tme = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&tme);
                    }
                    if !state.hovered {
                        state.hovered = true;
                        let _ = update_layered_window_view(hwnd, state_ptr);
                    }
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    state.tracking_mouse = false;
                    if state.hovered {
                        state.hovered = false;
                        let _ = update_layered_window_view(hwnd, state_ptr);
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    let delta = (wparam.0 >> 16) as i16;
                    let zoom_factor = 1.0 + (delta as f64 / 120.0) * 0.05;
                    let next_zoom = (state.current_zoom * zoom_factor).clamp(0.1, 50.0);

                    if next_zoom != state.current_zoom {
                        let mut mouse_pos = POINT::default();
                        if GetCursorPos(&mut mouse_pos).as_bool() {
                            let mut rect = RECT::default();
                            if GetWindowRect(hwnd, &mut rect).as_bool() {
                                let cur_w = rect.right - rect.left;
                                let cur_h = rect.bottom - rect.top;

                                let rx = if cur_w > 0 { (mouse_pos.x - rect.left) as f64 / cur_w as f64 } else { 0.5 };
                                let ry = if cur_h > 0 { (mouse_pos.y - rect.top) as f64 / cur_h as f64 } else { 0.5 };

                                state.current_zoom = next_zoom;
                                let new_w = (state.original_w as f64 * state.current_zoom) as i32;
                                let new_h = (state.original_h as f64 * state.current_zoom) as i32;

                                let new_x = mouse_pos.x - (rx * new_w as f64) as i32;
                                let new_y = mouse_pos.y - (ry * new_h as f64) as i32;

                                SetWindowPos(
                                    hwnd,
                                    HWND::default(),
                                    new_x,
                                    new_y,
                                    new_w,
                                    new_h,
                                    SWP_NOZORDER | SWP_NOACTIVATE,
                                ).unwrap_or(());

                                let _ = update_layered_window_view(hwnd, state_ptr);
                            }
                        }
                    }
                }
                LRESULT(0)
            }

            WM_DESTROY => {
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;
                if !state_ptr.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0); // Clear to prevent double-free
                    let state = Box::from_raw(state_ptr);
                    DeleteObject(HGDIOBJ(state.hbitmap.0));
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use gtk::prelude::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    struct LinuxWindowState {
        original_w: f64,
        original_h: f64,
        current_zoom: f64,
        surface: cairo::ImageSurface,
        hovered: Cell<bool>,
    }

    pub struct LinuxPinWindow;

    impl super::PinWindow for LinuxPinWindow {
        fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) {
            let img = match image::load_from_memory(png_data) {
                Ok(img) => img,
                Err(e) => {
                    log::error!("Failed to decode PNG in Linux pin window: {}", e);
                    return;
                }
            };
            let rgba = img.to_rgba8();
            let img_w = rgba.width() as i32;
            let img_h = rgba.height() as i32;

            let mut bgra = vec![0u8; (img_w * img_h * 4) as usize];
            for (i, pixel) in rgba.pixels().enumerate() {
                let r = pixel[0] as u32;
                let g = pixel[1] as u32;
                let b = pixel[2] as u32;
                let a = pixel[3] as u32;
                let r_p = ((r * a) / 255) as u8;
                let g_p = ((g * a) / 255) as u8;
                let b_p = ((b * a) / 255) as u8;
                let a_p = a as u8;
                bgra[i * 4] = b_p;
                bgra[i * 4 + 1] = g_p;
                bgra[i * 4 + 2] = r_p;
                bgra[i * 4 + 3] = a_p;
            }

            let mut surface = match cairo::ImageSurface::create(cairo::Format::ARgb32, img_w, img_h) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Failed to create Cairo image surface: {}", e);
                    return;
                }
            };
            {
                if let Ok(mut data) = surface.data() {
                    data.copy_from_slice(&bgra);
                }
            }

            let state = Rc::new(RefCell::new(LinuxWindowState {
                original_w: width,
                original_h: height,
                current_zoom: 1.0,
                surface,
                hovered: Cell::new(false),
            }));

            let window = gtk::Window::new(gtk::WindowType::Toplevel);
            window.set_title("Octopus Pin Window");
            window.set_decorated(false);
            window.set_keep_above(true);
            window.set_skip_taskbar_hint(true);
            window.set_skip_pager_hint(true);

            if let Some(screen) = gdk::Screen::default() {
                if let Some(visual) = screen.rgba_visual() {
                    window.set_visual(Some(&visual));
                }
            }
            window.set_app_paintable(true);
            window.add_events(gdk::EventMask::POINTER_MOTION_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);

            window.set_default_size(width as i32, height as i32);
            window.move_(x as i32, y as i32);

            let state_draw = state.clone();
            window.connect_draw(move |win, cr| {
                let _ = cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
                cr.set_operator(cairo::Operator::Source);
                let _ = cr.paint();

                cr.set_operator(cairo::Operator::Over);
                let win_w = win.allocated_width() as f64;
                let win_h = win.allocated_height() as f64;

                let s = state_draw.borrow();
                let img_w = s.surface.width() as f64;
                let img_h = s.surface.height() as f64;

                if img_w > 0.0 && img_h > 0.0 {
                    let _ = cr.save();
                    cr.scale(win_w / img_w, win_h / img_h);
                    let _ = cr.set_source_surface(&s.surface, 0.0, 0.0);
                    let _ = cr.paint();
                    let _ = cr.restore();
                }

                // Close button overlay when hovered
                if s.hovered.get() {
                    let btn_sz = 24.0_f64.min(win_w).min(win_h);
                    let btn_x = win_w - btn_sz - 2.0;
                    let btn_y = 2.0;
                    let _ = cr.save();
                    cr.arc(btn_x + btn_sz / 2.0, btn_y + btn_sz / 2.0, btn_sz / 2.0, 0.0, std::f64::consts::TAU);
                    let _ = cr.set_source_rgba(0.85, 0.17, 0.17, 0.92);
                    let _ = cr.fill();
                    let inset = btn_sz * 0.3;
                    let _ = cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
                    cr.set_line_width(1.5);
                    cr.move_to(btn_x + inset, btn_y + inset);
                    cr.line_to(btn_x + btn_sz - inset, btn_y + btn_sz - inset);
                    cr.move_to(btn_x + btn_sz - inset, btn_y + inset);
                    cr.line_to(btn_x + inset, btn_y + btn_sz - inset);
                    let _ = cr.stroke();
                    let _ = cr.restore();
                }

                glib::Propagation::Proceed
            });

            let state_btn = state.clone();
            window.connect_button_press_event(move |win, event| {
                if event.button() == 1 {
                    // Check if click is in close button area
                    let s = state_btn.borrow();
                    if s.hovered.get() {
                        let (mx, my) = event.coords();
                        let win_w = win.allocated_width() as f64;
                        let win_h = win.allocated_height() as f64;
                        let btn_sz = 24.0_f64.min(win_w).min(win_h);
                        let btn_x = win_w - btn_sz - 2.0;
                        let btn_y = 2.0;
                        if mx >= btn_x && mx <= btn_x + btn_sz && my >= btn_y && my <= btn_y + btn_sz {
                            win.close();
                            return glib::Propagation::Stop;
                        }
                    }
                    let (x, y) = event.root_coords();
                    win.begin_drag_move(1, x as i32, y as i32, event.time());
                }
                glib::Propagation::Proceed
            });

            let state_motion = state.clone();
            window.connect_motion_notify_event(move |win, _event| {
                let s = state_motion.borrow();
                if !s.hovered.get() {
                    s.hovered.set(true);
                    win.queue_draw();
                }
                glib::Propagation::Proceed
            });

            let state_leave = state.clone();
            window.connect_leave_notify_event(move |win, _event| {
                let s = state_leave.borrow();
                if s.hovered.get() {
                    s.hovered.set(false);
                    win.queue_draw();
                }
                glib::Propagation::Proceed
            });

            let state_scroll = state.clone();
            window.connect_scroll_event(move |win, event| {
                let direction = event.direction();
                let zoom_factor = match direction {
                    gdk::ScrollDirection::Up => 1.05,
                    gdk::ScrollDirection::Down => 0.95,
                    _ => 1.0,
                };

                if zoom_factor != 1.0 {
                    let mut s = state_scroll.borrow_mut();
                    let next_zoom = (s.current_zoom * zoom_factor).clamp(0.1, 50.0);
                    if next_zoom != s.current_zoom {
                        let (mx, my) = event.coords();
                        let (cur_w, cur_h) = win.size();
                        let (wx, wy) = win.position();

                        let rx = if cur_w > 0 { mx / cur_w as f64 } else { 0.5 };
                        let ry = if cur_h > 0 { my / cur_h as f64 } else { 0.5 };

                        s.current_zoom = next_zoom;
                        let new_w = (s.original_w * s.current_zoom) as i32;
                        let new_h = (s.original_h * s.current_zoom) as i32;

                        let new_wx = wx as f64 + mx - rx * new_w as f64;
                        let new_wy = wy as f64 + my - ry * new_h as f64;

                        win.resize(new_w, new_h);
                        win.move_(new_wx as i32, new_wy as i32);
                        win.queue_draw();
                    }
                }
                glib::Propagation::Proceed
            });


            window.show_all();
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::MacPinWindow as PinWindowImpl;

#[cfg(target_os = "windows")]
pub use windows::WinPinWindow as PinWindowImpl;

#[cfg(target_os = "linux")]
pub use linux::LinuxPinWindow as PinWindowImpl;
