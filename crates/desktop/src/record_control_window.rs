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
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

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

    let result = WebviewWindowBuilder::new(app, WINDOW_LABEL, WebviewUrl::App(url.into()))
        .title("")
        .inner_size(WIDTH, HEIGHT)
        .position(x, y)
        .always_on_top(true)
        .decorations(false)
        .transparent(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .visible(true)
        .build();

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
/// Display 录制：用 display_id 查对应 Monitor（找不到 fallback 主屏）。
/// Window 录制：MVP fallback 主屏（window_id → display 查询复杂，推迟）。
fn compute_position(app: &AppHandle, source: &Source) -> (f64, f64) {
    let monitor = match source {
        Source::Display { display_id } => {
            // tauri Monitor 不直接暴露 CGDirectDisplayID，无法可靠按 display_id 匹配。
            // MVP fallback primary_monitor（与 record_window 配置浮窗一致）。
            // TODO: 精确匹配需 CGDirectDisplayID → NSScreen 查询，未来迭代补。
            let _ = display_id;
            app.primary_monitor().ok().flatten()
        }
        Source::Window { .. } | Source::Area { .. } => app.primary_monitor().ok().flatten(),
    };

    if let Some(m) = monitor {
        let scale = m.scale_factor();
        let pos = m.position();
        let sz = m.size();
        let mon_w = sz.width as f64 / scale;
        let mon_h = sz.height as f64 / scale;
        // 右下角 - 16px 内边距 - 浮窗宽高
        let x = pos.x as f64 + mon_w - WIDTH - 16.0;
        let y = pos.y as f64 + mon_h - HEIGHT - 16.0;
        (x.max(0.0), y.max(0.0))
    } else {
        // 极端 fallback：屏幕原点偏移
        (100.0, 100.0)
    }
}

fn source_type_str(s: &Source) -> &'static str {
    match s {
        Source::Display { .. } => "display",
        Source::Window { .. } => "window",
        Source::Area { .. } => "area",
    }
}
