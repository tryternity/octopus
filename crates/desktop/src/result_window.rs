// src/result_window.rs

use log::debug;
use tauri::{Emitter, Manager};

const RESULT_WIDTH: f64 = 520.0;
const RESULT_HEIGHT: f64 = 100.0;
const WINDOW_LABEL: &str = "result_window";

// ── 窗口管理 ──

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
            // debug 构建（cargo run / cargo build 不带 --release）自动打开 devtools，
            // 便于排查前端渲染/事件。release 构建自动剔除，无副作用。
            #[cfg(debug_assertions)]
            window.open_devtools();

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
pub fn show_result(app: &tauri::AppHandle, text: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("show-result", text);
        let _ = window.show();
    }
}

/// 更新结果窗口文本（流式更新时使用）。
pub fn update_result(app: &tauri::AppHandle, text: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("update-result", text);
    }
}

/// 清空结果窗口内容并隐藏（粘贴完成后调用）。
pub fn clear_result(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("clear-result", ());
        let window_clone = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = window_clone.hide();
        });
    }
}

/// 隐藏结果窗口（不清空内容，不归档）。
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
