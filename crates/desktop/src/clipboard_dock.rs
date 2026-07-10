//! 剪贴板浮窗 dock（吸附收缩）NSWindow 操作。

/// 收缩态：透明区域鼠标穿透 + 刷新 hit-test。
pub fn apply_dock_collapsed(window: &tauri::WebviewWindow) {
    let win = window.clone();
    let _ = window.run_on_main_thread(move || {
        if let Ok(ptr) = win.ns_window() {
            if !ptr.is_null() {
                let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                ns_win.setIgnoresMouseEvents(true);
                log::debug!("clipboard_dock: setIgnoresMouseEvents(true)");
            }
        }
    });
    // Tauri 的 set_ignore_cursor_events 作为补充——与 screenshot 同模式
    let _ = window.set_ignore_cursor_events(true);
}

/// 展开态：恢复正常接收鼠标事件。
pub fn apply_dock_expanded(window: &tauri::WebviewWindow) {
    let win = window.clone();
    let _ = window.run_on_main_thread(move || {
        if let Ok(ptr) = win.ns_window() {
            if !ptr.is_null() {
                let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                ns_win.setIgnoresMouseEvents(false);
                log::debug!("clipboard_dock: setIgnoresMouseEvents(false)");
            }
        }
    });
    let _ = window.set_ignore_cursor_events(false);
}
