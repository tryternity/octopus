// src/result_window.rs

use log::debug;
use tauri::{Emitter, Manager};

const RESULT_WIDTH: f64 = 520.0;
const RESULT_HEIGHT: f64 = 100.0;
const WINDOW_LABEL: &str = "result_window";

/// 创建结果展示窗口（默认隐藏）。
pub fn create_result_window(app: &tauri::AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }

    let builder = tauri::WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        tauri::WebviewUrl::App("result/index.html".into()),
    )
    .title("Result")
    .inner_size(RESULT_WIDTH, RESULT_HEIGHT)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .focused(false)
    .visible(false)
    .shadow(false);

    match builder.build() {
        Ok(window) => {
            // 首次创建时定位到屏幕顶部居中
            if let Ok(monitor) = window.primary_monitor() {
                if let Some(m) = monitor {
                    let x = (m.size().width as f64 / m.scale_factor() - RESULT_WIDTH) / 2.0;
                    let y = 80.0;
                    let _ = window.set_position(tauri::Position::Logical(
                        tauri::LogicalPosition::new(x, y),
                    ));
                }
            }
            debug!("Result window created");
        }
        Err(e) => debug!("Failed to create result window: {}", e),
    }
}

/// 显示结果窗口并展示识别文本。
/// 仅首次显示时设置位置，之后保留用户拖拽后的位置。
pub fn show_result(app: &tauri::AppHandle, text: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("show-result", text);
        let _ = window.show();
    }
}

/// 隐藏结果窗口。
pub fn hide_result(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("hide-result", ());
        let window_clone = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = window_clone.hide();
        });
    }
}
