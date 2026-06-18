// src/result_window.rs

use log::debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

const RESULT_WIDTH: f64 = 520.0;
const RESULT_HEIGHT: f64 = 100.0;
const WINDOW_LABEL: &str = "result_window";

static WINDOW_READY: AtomicBool = AtomicBool::new(false);
static PENDING_TEXT: Mutex<Option<String>> = Mutex::new(None);
static SESSION_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    .shadow(false)
    // macOS：非激活悬浮窗（focused(false)）默认吞掉首次点击——仅用于激活窗口、
    // 不派发给 webview，导致工具栏按钮（✏️ 进入编辑等）首次点击无响应。accept_first_mouse
    // 让首次点击也正常派发，按钮点击可靠（双击进入已弃用，改用 edit_shortcut，见 spec §3.1）。
    .accept_first_mouse(true);

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

/// 前端页面就绪命令：初始化 ready 状态，并冲刷可能积压的初始文本
#[tauri::command]
pub fn result_window_ready(app_handle: tauri::AppHandle) {
    WINDOW_READY.store(true, Ordering::Relaxed);
    let pending = PENDING_TEXT.lock().unwrap().take();
    if let Some(text) = pending {
        show_result(&app_handle, &text);
    }
}

/// 显示结果窗口并展示识别文本。
pub fn show_result(app: &tauri::AppHandle, text: &str) {
    let _ = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    // 「判 ready + 写 pending」收进同一把 PENDING_TEXT 锁，与 result_window_ready 的
    // store(true)+take 互斥——消除「load(false) 后、写 pending 前 ready 已 take 走 None」
    // 导致该文本滞留（应用启动首帧文本丢失 / 不弹窗）的 TOCTOU 竞态。
    let need_emit = {
        let mut guard = PENDING_TEXT.lock().unwrap();
        if WINDOW_READY.load(Ordering::Relaxed) {
            true
        } else {
            *guard = Some(text.to_string());
            false
        }
    };
    if need_emit {
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = window.emit("show-result", text);
            let _ = window.show();
        }
    }
}

/// 更新结果窗口文本（流式更新时使用）。
pub fn update_result(app: &tauri::AppHandle, text: &str) {
    // 同 show_result：判 ready + 写 pending 进同一锁，消除与 result_window_ready 的竞态。
    let need_emit = {
        let mut guard = PENDING_TEXT.lock().unwrap();
        if WINDOW_READY.load(Ordering::Relaxed) {
            true
        } else {
            *guard = Some(text.to_string());
            false
        }
    };
    if need_emit {
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = window.emit("update-result", text);
        }
    }
}

/// 清空结果窗口内容并隐藏（粘贴完成后调用）。
pub fn clear_result(app: &tauri::AppHandle) {
    *PENDING_TEXT.lock().unwrap() = None;
    if WINDOW_READY.load(Ordering::Relaxed) {
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = window.emit("clear-result", ());
            let window_clone = window.clone();
            let current_session = SESSION_COUNTER.load(Ordering::Relaxed);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                if SESSION_COUNTER.load(Ordering::Relaxed) == current_session {
                    let _ = window_clone.hide();
                }
            });
        }
    }
}

/// 隐藏结果窗口（不清空内容，不归档）。
pub fn hide_result(app: &tauri::AppHandle) {
    if WINDOW_READY.load(Ordering::Relaxed) {
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = window.emit("hide-result", ());
            let window_clone = window.clone();
            let current_session = SESSION_COUNTER.load(Ordering::Relaxed);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                if SESSION_COUNTER.load(Ordering::Relaxed) == current_session {
                    let _ = window_clone.hide();
                }
            });
        }
    }
}


