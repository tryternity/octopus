//! 录制控制浮窗——display/window 录制时显示的桌面 pill。
//!
//! 与 `record_annotation_window` 的关系：
//! - **Area 录制** → 创建 RecordAnnotation（带 9 种标注画布 + 工具栏）
//! - **Display/Window 录制** → 创建本模块的控制浮窗（仅 pill：红点 + 时长 + 暂停/停止）
//! 两者互斥，不会同时出现。
//!
//! 设计：
//! - **位置**：录制所在屏右下角（display 有 display_id，MVP fallback 主屏；window 录制也 fallback 主屏）
//! - **不穿透**：pill 必须能直接点按钮（与 RecordAnnotation 部分穿透不同）
//! - **always_on_top**：保持置顶（会被 SCK 录进 display/window 视频——用户主动选项，接受）
//! - **跟随 stop_and_store 关闭**：在 main.rs/record_hotkey/tray 三条停止路径都调 close_control_window
//!
//! 详见 `docs/superpowers/specs/2026-07-26-record-control-window.md`。

use octopus_record::Source;
use tauri::{AppHandle, Manager};
use crate::window_factory::{build_float_window, FloatWindowSpec};

pub const WINDOW_LABEL: &str = "record_control_window";

/// pill 浮窗固定尺寸（逻辑像素）。
///
/// 紧凑布局：红点(7px) + gap + 时长(mm:ss ~28px) + gap + 暂停(24px) + 停止(24px)。
/// 实测 130×38 够用；太长显得突兀（用户反馈原 200×56 太长）。
const WIDTH: f64 = 130.0;
const HEIGHT: f64 = 38.0;

/// 创建控制浮窗。仅 Display/Window 录制创建；Area 录制静默跳过（用 RecordAnnotation）。
///
/// 已存在则先 destroy 重建（单例保证）。失败仅 warn 不阻断录制。
pub fn create_control_window(app: &AppHandle, source: &Source) {
    // Area 录制用 RecordAnnotation，不重复创建控制浮窗
    if matches!(source, Source::Area { .. }) {
        return;
    }

    // 单例：已存在则销毁重建（避免 stale state）
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.destroy();
    }

    let (x, y) = compute_position(app, source);

    // URL query 传 source 类型（前端按 display/window 渲染不同 label，MVP 都一样）
    let url = format!("record-control.html?source={}", source_type_str(source));

    let result = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: &url,
        title: "",
        inner_size: (WIDTH, HEIGHT),
        visible: true,
        resizable: false,
        position: Some((x, y)),
        focused: None,
        accept_first_mouse: None,
    });

    if let Err(e) = result {
        log::warn!("[record] 控制浮窗创建失败（不影响录制）: {e}");
    }
}

/// 关闭控制浮窗（单例 destroy）。
pub fn close_control_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.destroy();
    }
}

/// 算 pill 左上角坐标——录制所在显示器右下角，留 16px 内边距。
///
/// **Display 录制**：`display_id` 是 CGDirectDisplayID，直接用 CoreGraphics
/// `CGDisplay::new(id).bounds()` 拿到该屏的**逻辑** CGRect（CoreGraphics 原生返回
/// 逻辑 points，已含 scale，无需再除）。这是修复 2026-07-26 副屏 bug 的关键——
/// 之前用 `app.primary_monitor()` 永远定位到主屏，且 `Monitor::position()` 返回
/// 物理像素未除 scale，双重错误导致副屏录制时 pill 跑到主屏右下角（见 AGENTS.md
/// 物理/逻辑坐标 gotcha）。
///
/// **Window 录制**：window_id → display 查询复杂，MVP 仍 fallback 主屏。
/// **Area 录制**：本函数不应被调到（create_control_window 提前 return）。
fn compute_position(app: &AppHandle, source: &Source) -> (f64, f64) {
    // 优先路径：Display 录制用 CGDirectDisplayID 直接查逻辑边界
    if let Source::Display { display_id } = source {
        if let Some((origin_x, origin_y, w, h)) = cg_display_logical_bounds(*display_id) {
            return pill_bottom_right(origin_x, origin_y, w, h);
        }
        log::warn!(
            "[record] CGDisplay::bounds() 查不到 display_id={display_id}，回退到主屏定位"
        );
    }

    // 回退路径：Tauri Monitor（window 录制、或 CG 查询失败）
    let m = match app.primary_monitor() {
        Ok(Some(m)) => m,
        _ => return (100.0, 100.0), // 极端 fallback：屏幕原点偏移
    };
    // ⚠️ Monitor::position()/size() 都是物理像素，必须除 scale（AGENTS.md gotcha）
    let scale = m.scale_factor();
    let origin_x = m.position().x as f64 / scale;
    let origin_y = m.position().y as f64 / scale;
    let w = m.size().width as f64 / scale;
    let h = m.size().height as f64 / scale;
    pill_bottom_right(origin_x, origin_y, w, h)
}

/// 给定显示器逻辑原点 + 宽高，算 pill 左上角坐标（右下角 - 16px 内边距 - 浮窗尺寸）。
///
/// 注意：副屏可能在主屏**左侧或上方**，此时 `origin` 是负数（如左侧副屏 origin_x=-1920、
/// 上方副屏 origin_y=-800）。不能 `.max(0.0)`——会把 pill 推回主屏（=bug 重现）。
/// pill 坐标 = origin + (w - WIDTH - 16)，副屏在左/上时这是正确的负值，Tauri 接受。
fn pill_bottom_right(origin_x: f64, origin_y: f64, w: f64, h: f64) -> (f64, f64) {
    let x = origin_x + w - WIDTH - 16.0;
    let y = origin_y + h - HEIGHT - 16.0;
    (x, y)
}

/// 用 CoreGraphics 查 CGDirectDisplayID 对应显示器的**逻辑**边界（points）。
///
/// CoreGraphics 的 `CGRect` 原生就是逻辑坐标（不是物理像素），已含 scale，
/// 所以无需像 Tauri Monitor 那样手动除 `scale_factor()`。
///
/// 返回 `(origin_x, origin_y, width, height)`；display_id 为 0（无效）或查询失败返回 None。
#[cfg(target_os = "macos")]
fn cg_display_logical_bounds(display_id: u32) -> Option<(f64, f64, f64, f64)> {
    if display_id == 0 {
        return None;
    }
    use core_graphics::display::CGDisplay;
    let bounds = CGDisplay::new(display_id).bounds();
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return None;
    }
    Some((
        bounds.origin.x,
        bounds.origin.y,
        bounds.size.width,
        bounds.size.height,
    ))
}

#[cfg(not(target_os = "macos"))]
fn cg_display_logical_bounds(_display_id: u32) -> Option<(f64, f64, f64, f64)> {
    None
}

fn source_type_str(s: &Source) -> &'static str {
    match s {
        Source::Display { .. } => "display",
        Source::Window { .. } => "window",
        Source::Area { .. } => "area",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：副屏在主屏**左侧**（origin_x 负值）时，pill 必须落在副屏内（负 x），
    /// 不能被 `.max(0.0)` 推回主屏——这是 2026-07-26 修复前的 bug 重现场景。
    #[test]
    fn pill_bottom_right_secondary_display_on_left() {
        // 左侧副屏：origin=(-1920, 0), size=(1920, 1080)
        let (x, y) = pill_bottom_right(-1920.0, 0.0, 1920.0, 1080.0);
        // 期望 x = -1920 + 1920 - 130 - 16 = -146（落在副屏右下角内侧）
        assert_eq!(x, -146.0, "左侧副屏 pill x 应为负值（在副屏坐标空间内）");
        assert_eq!(y, 1080.0 - HEIGHT - 16.0);
    }

    /// 回归：副屏在主屏**上方**（origin_y 负值）时，pill 必须落在副屏内（负 y）。
    #[test]
    fn pill_bottom_right_secondary_display_on_top() {
        // 上方副屏：origin=(0, -800), size=(1440, 800)
        let (x, y) = pill_bottom_right(0.0, -800.0, 1440.0, 800.0);
        assert_eq!(x, 1440.0 - WIDTH - 16.0);
        assert_eq!(y, -800.0 + 800.0 - HEIGHT - 16.0, "上方副屏 pill y 应为负值");
        assert!(y < 0.0, "上方副屏 pill y 必须为负（在副屏坐标空间）");
    }

    /// 主屏正常场景（origin=0,0）：pill 右下角内边距 16px。
    #[test]
    fn pill_bottom_right_primary_display() {
        let (x, y) = pill_bottom_right(0.0, 0.0, 1440.0, 900.0);
        assert_eq!(x, 1440.0 - WIDTH - 16.0);
        assert_eq!(y, 900.0 - HEIGHT - 16.0);
    }

    /// display_id=0 是无效 CGDirectDisplayID（CGMainDisplayID() = 1），必须返回 None
    /// 而不是尝试查询（避免误中已废弃/无效 display）。
    #[test]
    fn cg_display_logical_bounds_rejects_zero_id() {
        assert!(cg_display_logical_bounds(0).is_none());
    }
}
