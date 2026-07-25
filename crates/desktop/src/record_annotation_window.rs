//! 录屏标注 overlay 窗口——录屏开始后显示，用户画标注被 SCK 录进视频。
//!
//! **关键设计**（spike7/8 验证）：
//! - overlay 用**普通窗口 level**（非 always_on_top）—— SCK 不录 floating 浮层
//! - SCK 录窗口 buffer 内容，与层级/可见性无关（切应用时标注仍被录到）
//! - overlay 尺寸 = 选区尺寸，位置 = 选区在屏幕上的全局位置（精确覆盖选区）
//! - 标注渲染在前端（复用 lib/annotation），SCK 录到 overlay 窗口的画面
//!
//! 仅 macOS。

#![cfg(target_os = "macos")]

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const WINDOW_LABEL: &str = "record_annotation_window";

/// 创建标注 overlay 窗口（按选区位置 + 尺寸）。
///
/// 在 record_commands::start_with_config 成功后 + Source::Area 时调用。
/// URL query 传选区参数（前端 RecordAnnotation.tsx mount 时解析）：
/// - display_id / x / y / width / height（物理像素）
/// - scale（显示器缩放，前端逻辑像素转换用）
pub fn create_annotation_window(
    app: &AppHandle,
    selection: &octopus_record::Source,
) -> Result<(), String> {
    // 仅 Source::Area 有意义（display/window 录制不弹标注 overlay）
    let (display_id, x, y, w, h) = match selection {
        octopus_record::Source::Area { display_id, x, y, width, height } => {
            (*display_id, *x as f64, *y as f64, *width as f64, *height as f64)
        }
        _ => return Ok(()), // 非 Area 静默跳过
    };

    // 已存在则先销毁（避免重复创建）
    if let Some(old) = app.get_webview_window(WINDOW_LABEL) {
        let _ = old.destroy();
    }

    // 拿选区所在显示器（用 active_display_for_point 查 CGDirectDisplayID 对应的 Tauri monitor）
    let monitors = app.available_monitors().unwrap_or_default();
    let mon = monitors.iter().find(|m| {
        // Tauri monitor.position() 是物理坐标；display_id 是 CGDirectDisplayID
        // 用 bounds 命中更可靠，但 Tauri 没暴露 display_id 映射
        // 简化：用 selection 的物理坐标 + scale 推逻辑位置匹配 monitor
        let scale = m.scale_factor();
        let mx = m.position().x as f64 / scale;
        let my = m.position().y as f64 / scale;
        let mw = m.size().width as f64 / scale;
        let mh = m.size().height as f64 / scale;
        // 选区左上角逻辑坐标
        let sel_x_logical = mx + (x / scale);
        let sel_y_logical = my + (y / scale);
        sel_x_logical >= mx && sel_x_logical < mx + mw && sel_y_logical >= my && sel_y_logical < my + mh
    }).or_else(|| monitors.first());

    let mon = match mon {
        Some(m) => m,
        None => {
            log::warn!("[annotation] 找不到选区所在显示器，不创建 overlay");
            return Ok(());
        }
    };

    let scale = mon.scale_factor();
    // 选区在屏幕上的全局逻辑位置（物理 → 逻辑 / scale）
    let mon_x = mon.position().x as f64 / scale;
    let mon_y = mon.position().y as f64 / scale;
    let sel_global_x = mon_x + (x / scale);
    let sel_global_y = mon_y + (y / scale);
    let sel_logical_w = w / scale;
    let sel_logical_h = h / scale;

    log::info!(
        "[annotation] 创建 overlay: display_id={} 选区逻辑 ({},{},{},{}) scale={}",
        display_id, sel_global_x, sel_global_y, sel_logical_w, sel_logical_h, scale
    );

    // URL 注入选区参数（前端 mount 时解析，用于 Canvas 尺寸 + 标注坐标）
    let url = format!(
        "record-annotation.html?display_id={}&x={}&y={}&width={}&height={}&scale={}",
        display_id, x, y, w, h, scale
    );

    let _ = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::App(url.into()),
    )
    .title("")
    .inner_size(sel_logical_w, sel_logical_h)
    .position(sel_global_x, sel_global_y)
    // ⚠️ 关键：不用 always_on_top（spike 验证 SCK 不录 floating 浮层）
    .decorations(false)
    .transparent(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(true) // 直接显示（与 picker 不同，picker 是 ready 后统一 show）
    .build()
    .map_err(|e| {
        log::error!("[annotation] overlay 窗口创建失败: {e}");
        format!("overlay 窗口创建失败: {e}")
    })?;

    Ok(())
}

/// 关闭标注 overlay 窗口（stop 时调用）。
pub fn close_annotation_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.destroy();
    }
}

/// 切换标注 overlay 的鼠标透传（标注模式 / 透传模式 toggle）。
///
/// passthrough=true：setIgnoreMouseEvents(true)，鼠标穿透到下层应用
/// passthrough=false：正常接收鼠标（画标注）
#[tauri::command]
pub fn set_annotation_passthrough(app: AppHandle, passthrough: bool) {
    set_annotation_passthrough_inner(&app, passthrough);
}

fn set_annotation_passthrough_inner(app: &AppHandle, passthrough: bool) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.set_ignore_cursor_events(passthrough);
        log::debug!("[annotation] passthrough={}", passthrough);
    }
}
