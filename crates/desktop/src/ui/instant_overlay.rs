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
/// 底部留白（逻辑像素）——浮窗底边距屏幕底边的距离。尽量贴近底部不干扰工作区。
const BOTTOM_MARGIN: f64 = 8.0;

/// instant-state 事件 payload（序列化传给前端）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InstantStatePayload {
    state: String,
    text: String,
}

/// 启动时预创建 instant 浮窗壳（隐藏）。首次 show 时零延迟（WebView 已加载）。
pub fn precreate(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: "instant-overlay.html",
        title: "Instant",
        inner_size: (OVERLAY_WIDTH, OVERLAY_HEIGHT),
        visible: false,
        resizable: false,
        position: None,
        focused: Some(false),
        accept_first_mouse: None,
    });
    log::info!("[instant-overlay] precreated (hidden)");
}

/// 创建 instant 浮窗（如不存在），并显示 + 推送状态。
///
/// `state`: "listening" | "processing" | "polishing" | "done"
/// `text`: 实时识别文字或最终文字（listening 期间可空）。
pub fn show_instant_overlay(app: &AppHandle, state: &str, text: &str) {
    let win = match app.get_webview_window(WINDOW_LABEL) {
        Some(w) => w,
        None => match build_float_window(app, FloatWindowSpec {
            label: WINDOW_LABEL,
            url: "instant-overlay.html",
            title: "Instant",
            inner_size: (OVERLAY_WIDTH, OVERLAY_HEIGHT),
            visible: true,
            resizable: false,
            position: None,
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

    position_bottom_center(app, &win);
    let _ = win.show();
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

/// 把窗口定位到鼠标所在显示器底部居中（逻辑坐标）。
///
/// 用 CGEvent::location() 获取鼠标全局坐标（Quartz 逻辑 points），
/// 用 CGDisplay::active_displays() + bounds() 找鼠标所在的显示器
/// （CoreGraphics 原生逻辑坐标，不除 scale——AGENTS.md 坐标 gotcha）。
/// 鼠标位置不可用时 fallback 到 primary_monitor。
fn position_bottom_center(app: &AppHandle, win: &tauri::WebviewWindow) {
    let mouse = crate::ui::window_position::get_mouse_location();
    log::info!("[instant-overlay] mouse_location={:?}", mouse);

    // 优先路径：用 CGDisplay::bounds()（原生逻辑坐标）找鼠标所在屏
    if let Some((_display_id, origin_x, origin_y, w, h)) =
        crate::ui::window_position::find_monitor_at_mouse(mouse)
    {
        log::info!("[instant-overlay] monitor bounds: origin=({},{}) size=({},{})",
            origin_x, origin_y, w, h);
        let x = origin_x + (w - OVERLAY_WIDTH) / 2.0;
        let y = origin_y + h - OVERLAY_HEIGHT - BOTTOM_MARGIN;
        let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
        return;
    }

    // fallback：primary_monitor（物理坐标除 scale）
    log::warn!("[instant-overlay] no monitor found at mouse, fallback to primary");
    let Some(monitor) = app.primary_monitor().ok().flatten() else { return };
    let scale = monitor.scale_factor();
    let pos = monitor.position();
    let size = monitor.size();
    let x = (size.width as f64 / scale - OVERLAY_WIDTH) / 2.0;
    let y = pos.y as f64 / scale + (size.height as f64 / scale - OVERLAY_HEIGHT - BOTTOM_MARGIN);
    let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
}
