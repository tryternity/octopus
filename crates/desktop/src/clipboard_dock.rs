//! 剪贴板浮窗 dock（吸附收缩）NSWindow 操作。

/// 收缩态：透明区域鼠标穿透 + 刷新 hit-test。
pub fn apply_dock_collapsed(window: &tauri::WebviewWindow) {
    let _ = window.set_ignore_cursor_events(true);
    // macOS hit-test 缓存：set_ignore_cursor_events(true) 后首次点击
    // 仍命中本窗口。用 NSWindow invalidateCursorRects 强制刷新。
    #[cfg(target_os = "macos")]
    {
        let win = window.clone();
        let _ = window.run_on_main_thread(move || {
            if let Ok(ptr) = win.ns_window() {
                if !ptr.is_null() {
                    unsafe {
                        let _: () = objc2::msg_send![
                            ptr as *mut objc2::runtime::AnyObject,
                            invalidateCursorRects
                        ];
                    }
                }
            }
        });
    }
    log::debug!("clipboard_dock: collapsed (ignore_cursor + invalidate)");
}

/// 展开态：恢复正常接收鼠标事件。
pub fn apply_dock_expanded(window: &tauri::WebviewWindow) {
    let _ = window.set_ignore_cursor_events(false);
    log::debug!("clipboard_dock: expanded (cursor events on)");
}
