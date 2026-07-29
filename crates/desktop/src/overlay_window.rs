//! Run And Paste silent 模式的 overlay 浮窗——显示进度/toast，不获取键盘焦点。
//!
//! 三种模式（由 emit payload 决定）：
//! - loading：spinner + "正在执行 {action}... · 按 Esc 取消"
//! - toast warn：黄色图标 + {message}，{duration} ms 后自动消失
//! - toast error：红色图标 + {message}，{duration} ms 后自动消失

use tauri::{AppHandle, Emitter, Manager};

use crate::window_factory::{build_float_window, FloatWindowSpec};

pub const WINDOW_LABEL: &str = "overlay_window";

/// 创建 overlay 窗口（应用启动时调用，visible=false）。
pub fn create_overlay_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: "overlay.html",
        title: "",
        inner_size: (320.0, 48.0),
        visible: false,
        resizable: false,
        position: None,
        focused: None,
        accept_first_mouse: None,
    });
}

/// overlay payload（序列化传给前端）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayPayload {
    mode: String,       // "loading" | "toast"
    message: String,
    toast_type: String, // "warn" | "error"（toast 模式）
    duration: u64,      // toast 自动关闭 ms
}

/// 在鼠标附近显示 overlay 窗口（不调 set_focus，不抢焦点）。
fn show_at_mouse(app: &AppHandle, payload: &OverlayPayload) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        // P2-3：get_mouse_position 失败时 fallback 中心显示（不再用 100,100 假坐标）。
        // overlay 宽 320 高 ~80，居中即取主屏中心位置。
        let (win_x, win_y) = match crate::action_bar_commands::get_mouse_position(app) {
            Some((mx, my)) => (mx - 160.0, my - 60.0), // 鼠标上方居中
            None => {
                // fallback：主屏中心（overlay 用于 ASR 录音指示，位置精度让位给可用性）
                let (cx, cy) = app.primary_monitor()
                    .ok()
                    .flatten()
                    .and_then(|m| {
                        let scale = m.scale_factor();
                        let pos = m.position();
                        let sz = m.size();
                        Some(((pos.x as f64 + sz.width as f64 / scale / 2.0) - 160.0,
                              (pos.y as f64 + sz.height as f64 / scale / 2.0) - 40.0))
                    })
                    .unwrap_or((400.0, 300.0));
                (cx, cy)
            }
        };

        let _ = win.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(win_x, win_y),
        ));
        let _ = win.show();
        // 不调 set_focus——overlay 不获取键盘焦点
        let _ = app.emit("overlay://show", payload);
    }
}

/// 隐藏 overlay 窗口。
#[allow(dead_code)]
pub fn hide_overlay_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.hide();
    }
}

/// 显示 loading 状态（执行中）。当前 quick_execute 不使用 overlay——保留供未来 silent 模式复用。
#[allow(dead_code)]
pub fn show_overlay_loading(app: &AppHandle, action_name: &str) {
    show_at_mouse(app, &OverlayPayload {
        mode: "loading".into(),
        message: format!("正在执行 {}...", action_name),
        toast_type: String::new(),
        duration: 0,
    });
}

/// 显示 toast（warn/error），duration ms 后自动隐藏。当前未使用——保留供未来 silent 模式复用。
#[allow(dead_code)]
pub fn show_overlay_toast(app: &AppHandle, message: &str, toast_type: &str, duration_ms: u64) {
    show_at_mouse(app, &OverlayPayload {
        mode: "toast".into(),
        message: message.into(),
        toast_type: toast_type.into(),
        duration: duration_ms,
    });

    // duration 后自动隐藏
    let app_clone = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(duration_ms));
        hide_overlay_window(&app_clone);
    });
}
