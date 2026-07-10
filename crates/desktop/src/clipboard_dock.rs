//! 剪贴板浮窗 dock（吸附收缩）NSWindow 操作。
//!
//! 收缩态：set_ignore_cursor_events(true) 让透明区域鼠标穿透到下层 app。
//! 展开态：set_ignore_cursor_events(false) 恢复正常。
//! 细条通过 CSS pointer-events: auto 可接收 hover/click——macOS 的
//! setIgnoreCursorEvents 优先级低于 WKWebView 内部 DOM 的事件投递，
//! 因此 pointer-events: auto 的元素仍可交互。

/// 收缩态：透明区域鼠标穿透。
pub fn apply_dock_collapsed(window: &tauri::WebviewWindow) {
    let _ = window.set_ignore_cursor_events(true);
    log::debug!("clipboard_dock: set_ignore_cursor_events(true)");
}

/// 展开态：恢复正常接收鼠标事件。
pub fn apply_dock_expanded(window: &tauri::WebviewWindow) {
    let _ = window.set_ignore_cursor_events(false);
    log::debug!("clipboard_dock: set_ignore_cursor_events(false)");
}
