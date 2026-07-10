//! 剪贴板浮窗 dock（吸附收缩）的 NSWindow 操作。
//!
//! 收缩态：setIgnoresMouseEvents(true) 全窗口穿透。
//! 展开态：setIgnoresMouseEvents(false) 恢复正常。

#[cfg(target_os = "macos")]
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
}

#[cfg(target_os = "macos")]
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
}

#[cfg(not(target_os = "macos"))]
pub fn apply_dock_collapsed(_window: &tauri::WebviewWindow) {}
#[cfg(not(target_os = "macos"))]
pub fn apply_dock_expanded(_window: &tauri::WebviewWindow) {}
