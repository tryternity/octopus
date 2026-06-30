//! 记事本窗口：独立 Tauri 窗口，原生标题栏，1000×680 可调大小。
//!
//! 单例管理：已打开则 set_focus，不重复创建。
//! macOS：打开记事本窗口时切换到 Regular 激活策略（Dock 显示图标），
//! 与 settings_window 一致。

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const WIDTH: f64 = 1000.0;
const HEIGHT: f64 = 680.0;
const MIN_WIDTH: f64 = 720.0;
const MIN_HEIGHT: f64 = 480.0;
const WINDOW_LABEL: &str = "notepad_window";

/// 打开记事本窗口（单例：已存在则 set_focus，不重复创建）。
#[tauri::command]
pub fn open_notepad(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_focus();
        let _ = window.show();
        return;
    }
    // macOS: 记事本是内容编辑窗口，切到 Regular 让 Dock 显示图标（与 settings 一致）。
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("记事本")
    .inner_size(WIDTH, HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .visible(true)
    .build();
}
