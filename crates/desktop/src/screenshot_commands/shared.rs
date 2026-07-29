//! screenshot_commands 跨子模块共享平台 helper。
//!
//! 2026-07-30 从原 screenshot_commands.rs 拆出（Task 1.1）。
//! 仅放被 area / scroll / 外部（record_area_picker 等）共用的平台 helper：
//! `format_file_size` + macOS Cocoa / CGDisplay 查询 helper。

/// 字节数 → 人类可读大小：<1M 显示 K（整数）、≥1M 显示 M（1 位小数）。
pub(crate) fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{}K", (bytes + 511) / 1024)
    } else {
        format!("{:.1}M", bytes as f64 / 1024.0 / 1024.0)
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn get_primary_screen_height() -> f64 {
    use objc2::{class, msg_send, runtime::AnyObject};
    unsafe {
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        let primary: *mut AnyObject = msg_send![screens, objectAtIndex: 0];
        let frame: objc2_foundation::NSRect = msg_send![primary, frame];
        frame.size.height as f64
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn get_window_cocoa_frame(win: &tauri::WebviewWindow) -> Option<(f64, f64, f64, f64)> {
    use objc2::{msg_send, runtime::AnyObject};
    let ptr = win.ns_window().ok()?;
    if ptr.is_null() { return None; }
    
    let rect: objc2_foundation::NSRect = unsafe { msg_send![ptr as *mut AnyObject, frame] };
    Some((rect.origin.x as f64, rect.origin.y as f64, rect.size.width as f64, rect.size.height as f64))
}

/// 查包含全局逻辑坐标 (cx, cy) 的 CGDirectDisplayID（区域录屏选区命中检测用）。
///
/// 抽自 start_scroll_recording（L1014-1024），让 record_area_picker 复用。
/// 无命中返回 0（CGMainDisplayID 是 1，0 表示无效）。
#[cfg(target_os = "macos")]
#[allow(dead_code)] // Task 2 record_area_picker 将使用
pub(crate) fn active_display_for_point(cx: f64, cy: f64) -> u32 {
    use core_graphics::display::CGDisplay;
    let displays = match CGDisplay::active_displays() {
        Ok(d) => d,
        Err(_) => {
            log::error!("[active_display_for_point] CGGetActiveDisplayList failed");
            return 0;
        }
    };
    displays.iter().find(|&&id| {
        let bounds = CGDisplay::new(id).bounds();
        cx >= bounds.origin.x && cx < bounds.origin.x + bounds.size.width
            && cy >= bounds.origin.y && cy < bounds.origin.y + bounds.size.height
    }).copied().unwrap_or(0)
}
