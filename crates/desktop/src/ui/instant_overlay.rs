//! Instant 指示浮窗（talk / PTT 模式专用）。
//!
//! 只读、透明、底部居中、不抢焦点的紧凑指示窗。由 coordinator 在
//! InstantStart 时 show + emit 状态，InstantStop / paste done 后 hide。
//!
//! 与 result_window 的区别：
//! - result_window：可编辑、抢焦点、顶部居中、720×480，适合 toggle 模式精修。
//! - instant_overlay：只读、不抢焦点、底部居中、400×80，适合 talk 模式快速粘贴。
//!
//! 状态机（由 `instant-state` 事件 payload `{ state, text }` 驱动）：
//! - listening：波形动画 + "正在聆听…"
//! - processing：spinner + "识别中…"
//! - polishing：spinner + "润色中…"
//! - done：展示最终文字（短暂停留后由 Rust hide）

use tauri::{AppHandle, Emitter, Manager};

use crate::ui::window_factory::{build_float_window, FloatWindowSpec};

pub const WINDOW_LABEL: &str = "instant_overlay";

const OVERLAY_WIDTH: f64 = 400.0;
const OVERLAY_HEIGHT: f64 = 80.0;
/// 底部留白（逻辑像素）——浮窗底边距屏幕底边的距离。
const BOTTOM_MARGIN: f64 = 80.0;

/// instant-state 事件 payload（序列化传给前端）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InstantStatePayload {
    state: String,
    text: String,
}

/// 创建 instant 浮窗（如不存在），并显示 + 推送状态。
///
/// `state`: "listening" | "processing" | "polishing" | "done"
/// `text`: 实时识别文字或最终文字（listening 期间可空）。
///
/// 首次调用时创建窗口（底部居中、透明、不抢焦点），后续调用复用：
/// 重定位到底部居中（多屏切换兜底）+ show + emit `instant-state`。
pub fn show_instant_overlay(app: &AppHandle, state: &str, text: &str) {
    // 不存在则创建（懒创建——toggle 模式下永不创建，节省资源）。
    let win = match app.get_webview_window(WINDOW_LABEL) {
        Some(w) => w,
        None => match build_float_window(app, FloatWindowSpec {
            label: WINDOW_LABEL,
            url: "instant-overlay.html",
            title: "Instant",
            inner_size: (OVERLAY_WIDTH, OVERLAY_HEIGHT),
            // 首次 build 时直接可见（下面会 set_position + emit）。
            visible: true,
            resizable: false,
            // position 在 build 后用主屏底部居中重算（多屏 / 缩放自适应）。
            position: None,
            // 不抢焦点：talk 模式下用户正在前台 app 工作，浮窗只做指示。
            focused: Some(false),
            accept_first_mouse: None,
        }) {
            Ok(w) => w,
            Err(e) => {
                log::debug!("Failed to create instant overlay: {}", e);
                return;
            }
        },
    };

    // 重定位到主屏底部居中（每次 show 都重算，应对多屏 / 缩放变化）。
    position_bottom_center(app, &win);

    let _ = win.show();
    // 不调 set_focus——instant 浮窗不抢键盘焦点。

    let _ = app.emit_to(
        WINDOW_LABEL,
        "instant-state",
        InstantStatePayload {
            state: state.to_string(),
            text: text.to_string(),
        },
    );
}

/// 隐藏 instant 浮窗（不销毁，下次 InstantStart 复用）。
pub fn hide_instant_overlay(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.hide();
    }
}

/// 把窗口定位到主屏底部居中（逻辑坐标）。
///
/// 多屏场景取 primary_monitor；失败则保持当前位置（不阻断 show）。
fn position_bottom_center(app: &AppHandle, win: &tauri::WebviewWindow) {
    let Some(monitor) = app.primary_monitor().ok().flatten() else {
        return;
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let pos = monitor.position();
    // 逻辑坐标：屏幕宽 / scale - 窗口宽，居中；屏幕高 / scale - 窗口高 - 底部留白。
    let x = (size.width as f64 / scale - OVERLAY_WIDTH) / 2.0;
    // monitor.position() 是物理坐标的原点（多屏时可能为负），换算到逻辑后加偏移。
    let y = pos.y as f64 / scale + (size.height as f64 / scale - OVERLAY_HEIGHT - BOTTOM_MARGIN);
    let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
}
