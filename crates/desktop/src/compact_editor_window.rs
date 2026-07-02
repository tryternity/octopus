//! 精简编辑器窗口：独立 Tauri 窗口，720×560 可调大小，居中。
//!
//! macOS：无 webview 原生窗(NSWindow+NSTextView，见 compact_editor_native)。
//! 非 macOS：回退 webview。macOS 开窗切 Regular（Dock 显图标），关窗切回 Accessory。

#[cfg(not(target_os = "macos"))]
use tauri::{WebviewUrl, WebviewWindowBuilder};

pub const WIDTH: f64 = 720.0;
pub const HEIGHT: f64 = 560.0;
pub const MIN_WIDTH: f64 = 480.0;
pub const MIN_HEIGHT: f64 = 360.0;
pub const WINDOW_LABEL: &str = "compact_editor_window";

/// 创建精简编辑器窗口（调用方已确保当前不存在同名窗口）。
pub fn create_compact_editor_window(app_handle: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        // macOS：编辑窗口切 Regular 让 Dock 显示图标（与 settings/notepad 一致）。
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
        crate::settings_window::set_dock_icon();
        // 原生窗(NSWindow+NSTextView，无 webview)
        crate::compact_editor_native::create_native_window(app_handle);
    }
    #[cfg(not(target_os = "macos"))]
    {
        // 非 macOS：回退 webview（现状实现）
        let _ = WebviewWindowBuilder::new(app_handle, WINDOW_LABEL, WebviewUrl::default())
            .title("编辑")
            .inner_size(WIDTH, HEIGHT)
            .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
            .decorations(true)
            .resizable(true)
            .center()
            .visible(true)
            .build();
    }
}

/// macOS: 精简编辑器窗口关闭时切回 Accessory（仅托盘）。
/// 与 notepad_window::on_notepad_closed 对称。
#[cfg(target_os = "macos")]
pub fn on_compact_editor_closed(app_handle: &tauri::AppHandle) {
    let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
}
