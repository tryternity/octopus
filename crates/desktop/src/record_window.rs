//! 录屏配置浮窗——Cmd+Shift+R 弹出，用户选源（display/window/area）+ 音频开关 + 开始。
//!
//! 设计参考 `password_generator_window.rs`（按需创建 + `before_floating_window_show`）
//! 与 `overlay_window::show_at_mouse`（鼠标附近位置计算）。
//!
//! 浮窗与 Settings 的 RecordingPanel 并存：
//! - 浮窗 = 快捷入口（Cmd+Shift+R），选源 + 开始
//! - Settings RecordingPanel = 历史管理 + 备用开始（用默认配置）
//!
//! 仅 macOS：录屏 helper 只 mac 实现。windows/linux 编译时此模块为空
//! （mod 声明处 cfg gate）。

#![cfg(target_os = "macos")]

use tauri::{AppHandle, Emitter, Manager};
use crate::window_factory::{build_float_window, FloatWindowSpec};

pub const WINDOW_LABEL: &str = "record_config_window";

const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 480.0;

/// 创建浮窗（单例，已存在则跳过）。setup hook 预创建 + 首次 show 时兜底调。
pub fn create_record_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: "record-config.html",
        title: "录制设置",
        inner_size: (WIDTH, HEIGHT),
        visible: false,
        resizable: false,
        position: None,
        focused: None,
        accept_first_mouse: None,
    })
    .map_err(|e| log::error!("[record-window] 窗口创建失败: {e}"));
}

/// 在鼠标附近显示浮窗（参考 overlay_window::show_at_mouse 的位置算法）。
///
/// fallback：鼠标位置不可用时，居中显示在主屏。
pub fn show_record_window(app: &AppHandle) {
    create_record_window(app);
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let (x, y) = compute_position(app);
        let _ = win.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(x, y),
        ));

        // macOS：浮窗 show 前隐藏常规窗口，避免 Regular 策略激活把主窗口带前台
        crate::activation::before_floating_window_show(app);

        let _ = win.show();
        let _ = win.set_focus();
        // 通知前端浮窗已显示（前端可拉取最新源列表）
        let _ = app.emit("record-config://show", ());
    }
}

/// 隐藏浮窗（保留窗口实例，下次 show 复用）。
pub fn hide_record_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.hide();
        crate::activation::after_floating_window_hide(app);
    }
}

/// 切换浮窗可见性（可见→hide；不可见→show）。当前未用，保留供未来 tray menu 复用。
#[allow(dead_code)]
pub fn toggle_record_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        if win.is_visible().unwrap_or(false) {
            hide_record_window(app);
        } else {
            show_record_window(app);
        }
    } else {
        show_record_window(app);
    }
}

/// 算浮窗左上角坐标——主屏水平居中 + 垂直上 1/3 位置。
///
/// 录屏是确定性操作（与鼠标当前位置无关），浮窗固定显示在主屏视觉焦点区
/// （上 1/3 处），避免遮挡屏幕底部 dock / 状态栏，也避免鼠标位置在副屏时
/// 浮窗跑到副屏（多屏环境下用户期望浮窗在主屏）。
///
/// fallback（拿不到主屏，极少见）：屏幕原点偏移 (100, 100)。
fn compute_position(app: &AppHandle) -> (f64, f64) {
    app.primary_monitor()
        .ok()
        .flatten()
        .and_then(|m| {
            let scale = m.scale_factor();
            let pos = m.position();
            let sz = m.size();
            let mon_w = sz.width as f64 / scale;
            let mon_h = sz.height as f64 / scale;
            // 水平居中：显示器中心 - 浮窗宽度/2
            let x = pos.x as f64 + mon_w / 2.0 - WIDTH / 2.0;
            // 垂直上 1/3：显示器顶部 + 高度/3 - 浮窗高度/2（浮窗中心对齐上 1/3 线）
            let y = pos.y as f64 + mon_h / 3.0 - HEIGHT / 2.0;
            Some((x, y.max(0.0)))
        })
        .unwrap_or((100.0, 100.0))
}
