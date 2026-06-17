//! 设置窗口：独立 Tauri 窗口，原生标题栏，800×600 可调大小。
//!
//! 单例管理：已打开则 set_focus，不重复创建。

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const SETTINGS_WIDTH: f64 = 800.0;
const SETTINGS_HEIGHT: f64 = 600.0;
const MIN_WIDTH: f64 = 640.0;
const MIN_HEIGHT: f64 = 480.0;
const WINDOW_LABEL: &str = "settings_window";

/// 打开设置窗口（单例：已存在则 set_focus）。
#[tauri::command]
pub fn open_settings(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        WebviewUrl::App("settings/index.html".into()),
    )
    .title("Octopus 设置")
    .inner_size(SETTINGS_WIDTH, SETTINGS_HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .visible(true)
    .build();
}
