//! 透明浮动窗口的通用建窗 helper（2026-07-29 DRY 重构）。
//!
//! 8-10 个浮动窗口（action_bar / overlay / password_generator / record_config /
//! result / clipboard / record_control / record_annotation / screenshot / record_area_picker）
//! 共享 5 参数透明浮动默认（transparent + no-decor + always_on-top + skip-taskbar + no-shadow），
//! 差异仅在 label / url / title / inner_size / visible / resizable / position。
//!
//! **只封装 builder 链**——build 后副作用（on_window_event / poller / 激活策略）保留在
//! 各窗口的 create 函数里，在 build_float_window 返回后接。单例策略由调用方预检查。

use tauri::{AppHandle, LogicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// 透明浮动窗口配置。
pub struct FloatWindowSpec<'a> {
    pub label: &'a str,
    /// WebviewUrl::App 的路径（如 "overlay.html"）。
    pub url: &'a str,
    pub title: &'a str,
    pub inner_size: (f64, f64),
    /// false = 启动隐藏（后续 show），true = 立即可见。
    pub visible: bool,
    pub resizable: bool,
    /// None = 不设（系统默认居中），Some = 逻辑坐标。
    pub position: Option<(f64, f64)>,
    /// None = builder 默认（focused），Some(false) = 非激活悬浮窗（result_window 用，避免抢焦点）。
    pub focused: Option<bool>,
    /// None = builder 默认，Some(true) = accept_first_mouse（result_window 用，非激活窗首次点击可靠）。
    pub accept_first_mouse: Option<bool>,
}

/// 构建透明浮动窗口（封装 5 参数默认 + spec 参数），返回 WebviewWindow 供调用方接 build 后副作用。
///
/// 平台中立（不用 cfg）——record_* 的模块级 `#![cfg(macos)]` 不受影响。
/// 不做单例检查（3 种变体：return / destroy-rebuild / focus-return），由调用方在调用前处理。
pub fn build_float_window(app: &AppHandle, spec: FloatWindowSpec) -> tauri::Result<WebviewWindow> {
    let mut builder = WebviewWindowBuilder::new(app, spec.label, WebviewUrl::App(spec.url.into()))
        .title(spec.title)
        .inner_size(spec.inner_size.0, spec.inner_size.1)
        // 透明浮动窗口 5 参数默认（8-10 个窗口共享）
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .resizable(spec.resizable)
        .visible(spec.visible);
    if let Some((x, y)) = spec.position {
        builder = builder.position(x, y);
    }
    if let Some(focused) = spec.focused {
        builder = builder.focused(focused);
    }
    if let Some(afm) = spec.accept_first_mouse {
        builder = builder.accept_first_mouse(afm);
    }
    builder.build()
}

/// 用逻辑坐标设置窗口位置（便捷 helper，封装 LogicalPosition 构造）。
#[allow(dead_code)]
pub fn set_logical_position(win: &WebviewWindow, x: f64, y: f64) -> tauri::Result<()> {
    win.set_position(tauri::Position::Logical(LogicalPosition::new(x, y)))
}
