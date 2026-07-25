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
    let mon_h = mon.size().height as f64 / scale;
    let sel_global_x = mon_x + (x / scale);
    let sel_global_y = mon_y + (y / scale);
    let sel_logical_w = w / scale;
    let sel_logical_h = h / scale;

    // ── 工具栏三选逻辑（与截图 screenshot_commands L750-766 完全一致）──
    // 用户决策：工具栏不需要被录进视频（只有标注才需要）。所以 overlay 窗口扩展
    // 容纳工具栏——选区下方优先，不够则上方，都不够则选区内部底部（被录可接受）。
    //
    // TOOLBAR_MARGIN：工具栏与选区的间距（8px，与截图一致）
    // TOOLBAR_H：工具栏高度（44px）
    // POPOVER_H：popover 高度估算（200px，工具栏弹出颜色/线宽时需要空间）
    const TOOLBAR_H: f64 = 44.0;
    const TOOLBAR_MARGIN: f64 = 8.0;
    const POPOVER_H: f64 = 200.0;
    let toolbar_space = TOOLBAR_H + TOOLBAR_MARGIN + POPOVER_H; // 工具栏 + popover 总空间

    let below_space = mon_h - (sel_global_y - mon_y + sel_logical_h + TOOLBAR_MARGIN);
    let above_space = sel_global_y - mon_y;
    let toolbar_below = below_space >= toolbar_space;
    let toolbar_above = !toolbar_below && above_space >= toolbar_space;

    // 窗口扩展方向 + 位置（让工具栏在选区外，Canvas 对齐选区）
    let (win_x, win_y, win_w, win_h, canvas_offset_x, canvas_offset_y, toolbar_pos) =
        if toolbar_below {
            // 工具栏在选区下方：窗口 = 选区 + 下方工具栏空间
            (
                sel_global_x,
                sel_global_y,
                sel_logical_w,
                sel_logical_h + toolbar_space,
                0.0,                  // Canvas X 偏移（窗口内）
                0.0,                  // Canvas Y 偏移
                "below",              // 工具栏位置标识（前端用）
            )
        } else if toolbar_above {
            // 工具栏在选区上方：窗口 = 上方工具栏空间 + 选区
            (
                sel_global_x,
                sel_global_y - toolbar_space,
                sel_logical_w,
                sel_logical_h + toolbar_space,
                0.0,
                toolbar_space,        // Canvas Y 偏移（窗口内，工具栏在上方）
                "above",
            )
        } else {
            // 兜底：工具栏在选区内部底部（被录进视频，可接受）
            // 窗口 = 选区尺寸（不扩展），工具栏覆盖在选区底部
            (
                sel_global_x,
                sel_global_y,
                sel_logical_w,
                sel_logical_h,
                0.0,
                0.0,
                "inside",
            )
        };

    log::info!(
        "[annotation] overlay: display_id={} 选区逻辑 ({},{},{},{}) toolbar={} 窗口 ({},{},{},{})",
        display_id, sel_global_x, sel_global_y, sel_logical_w, sel_logical_h,
        toolbar_pos, win_x, win_y, win_w, win_h
    );

    // 设置工具栏区域（poller 用——穿透模式下此区域不穿透）。
    // 工具栏在窗口内的位置（逻辑坐标）：
    // - below：Canvas 下方（canvas_oy + canvas_h 到窗口底部）
    // - above：窗口顶部（0 到 toolbar_space）
    // - inside：Canvas 内部底部（canvas_oy + canvas_h - 52 到 canvas_oy + canvas_h）
    let toolbar_zone = match toolbar_pos {
        "below" => (0.0, canvas_offset_y + sel_logical_h, sel_logical_w, toolbar_space),
        "above" => (0.0, 0.0, sel_logical_w, toolbar_space),
        _ => (0.0, canvas_offset_y + sel_logical_h - 52.0, sel_logical_w, 52.0),
    };
    *TOOLBAR_ZONE.lock() = toolbar_zone;

    // URL 注入选区参数 + 工具栏位置（前端 mount 时解析）
    let url = format!(
        "record-annotation.html?display_id={}&x={}&y={}&width={}&height={}&scale={}&toolbar={}&canvas_ox={}&canvas_oy={}&canvas_w={}&canvas_h={}",
        display_id, x, y, w, h, scale, toolbar_pos, canvas_offset_x, canvas_offset_y, sel_logical_w, sel_logical_h
    );

    let _ = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::App(url.into()),
    )
    .title("")
    .inner_size(win_w, win_h)
    .position(win_x, win_y)
    .always_on_top(true)
    .decorations(false)
    .transparent(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(true)
    .build()
    .map_err(|e| {
        log::error!("[annotation] overlay 窗口创建失败: {e}");
        format!("overlay 窗口创建失败: {e}")
    })?;

    // 启动点击穿透轮询（参考 result_window::start_click_through_poller）
    // 工具栏区域接收鼠标（可点按钮/画标注），其他区域穿透到下层应用。
    start_annotation_click_through_poller(app.clone());

    Ok(())
}

// 工具栏尺寸常量已移到 create_annotation_window 内部（不再模块级）。

/// 穿透模式标志：true=整个窗口穿透（操作下层应用），false=不穿透（操作标注）。
/// 由前端 emit "record-annotation://passthrough" 切换（点「鼠标」工具→穿透，点其他工具→不穿透）。
static ANNOTATION_PASSTHROUGH: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 工具栏区域（窗口内逻辑坐标）：穿透模式下此区域不穿透（可点工具栏按钮）。
/// 由 create_annotation_window 设置（canvas 底部以下到窗口底部）。
static TOOLBAR_ZONE: parking_lot::Mutex<(f64, f64, f64, f64)> =
    parking_lot::Mutex::new((0.0, 0.0, 0.0, 0.0)); // (x, y, w, h) 逻辑坐标

/// 启动标注 overlay 的点击穿透轮询。
///
/// 用户决策（2026-07-25）：
/// - **标注模式**（默认，passthrough=false）：整个窗口不穿透，用户画标注/选/删/移动
/// - **穿透模式**（passthrough=true）：整个窗口穿透，用户操作下层应用（录屏内容）
/// - 切换方式：点工具栏「鼠标」按钮→穿透模式；点其他标注工具→切回标注模式
///
/// 参考 `result_window::start_click_through_poller`：Rust 线程读 ANNOTATION_PASSTHROUGH
/// 状态切换 setIgnoresMouseEvents（前端 setIgnoreMouseEvents(true) 后窗口不收鼠标事件，
/// 无法检测光标重新进入 → 必须后端轮询）。
fn start_annotation_click_through_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(33));
        let mut cur_ignore = false;

        loop {
            poll.tick().await;

            let Some(win) = app.get_webview_window(WINDOW_LABEL) else {
                break;
            };
            if !win.is_visible().unwrap_or(false) {
                break;
            }

            let passthrough = ANNOTATION_PASSTHROUGH.load(std::sync::atomic::Ordering::Relaxed);

            // 标注模式（passthrough=false）：整个窗口不穿透（画标注/选删移动）
            if !passthrough {
                if cur_ignore {
                    set_annotation_ignores_mouse(&win, false);
                    cur_ignore = false;
                }
                continue;
            }

            // 穿透模式（passthrough=true）：工具栏区域不穿透，其他穿透。
            // 用 TOOLBAR_ZONE（create_annotation_window 时设置的窗口内逻辑坐标）判定。
            let (mx, my) = match win.cursor_position() {
                Ok(p) => (p.x, p.y),
                Err(_) => continue,
            };
            let (wx, wy) = match win.outer_position() {
                Ok(p) => (p.x as f64, p.y as f64),
                Err(_) => continue,
            };
            let sf = win.scale_factor().unwrap_or(1.0);

            // TOOLBAR_ZONE 是窗口内逻辑坐标 → 转物理坐标 + 加窗口偏移
            let (tz_x, tz_y, tz_w, tz_h) = *TOOLBAR_ZONE.lock();
            let zone_phys_x = wx + tz_x * sf;
            let zone_phys_y = wy + tz_y * sf;
            let zone_phys_w = tz_w * sf;
            let zone_phys_h = tz_h * sf;

            let in_toolbar = mx >= zone_phys_x && mx <= zone_phys_x + zone_phys_w
                && my >= zone_phys_y && my <= zone_phys_y + zone_phys_h;
            let want_ignore = !in_toolbar;

            // 诊断日志（passthrough 模式下每秒打一次）
            if want_ignore != cur_ignore {
                log::info!(
                    "[annotation-poller] passthrough mouse=({},{}) win=({},{}) zone=({},{},{},{}) in_toolbar={} → ignore={}",
                    mx, my, wx, wy, zone_phys_x, zone_phys_y, zone_phys_w, zone_phys_h, in_toolbar, want_ignore
                );
            }

            if want_ignore != cur_ignore {
                set_annotation_ignores_mouse(&win, want_ignore);
                cur_ignore = want_ignore;
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn set_annotation_ignores_mouse(win: &tauri::WebviewWindow, ignore: bool) {
    // 双保险：Tauri API（同步） + NSWindow 直调（run_on_main_thread 异步）。
    // result_window 也用同样模式（L217-228 + L232）。
    let _ = win.set_ignore_cursor_events(ignore);
    let win_clone = win.clone();
    let _ = win.run_on_main_thread(move || {
        if let Ok(ptr) = win_clone.ns_window() {
            if !ptr.is_null() {
                let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                ns_win.setIgnoresMouseEvents(ignore);
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn set_annotation_ignores_mouse(win: &tauri::WebviewWindow, ignore: bool) {
    let _ = win.set_ignore_cursor_events(ignore);
}

/// 关闭标注 overlay 窗口（stop 时调用）。
pub fn close_annotation_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.destroy();
    }
}

/// 切换标注 overlay 的鼠标透传（标注模式 / 透传模式 toggle）。
///
/// passthrough=true：整个窗口穿透（操作下层应用 / 录屏内容）
/// passthrough=false：整个窗口不穿透（画标注 / 选/删/移动标注）
///
/// 实际切换由 poller 按 ANNOTATION_PASSTHROUGH 状态执行（poller 33ms tick）。
#[tauri::command]
pub fn set_annotation_passthrough(_app: AppHandle, passthrough: bool) {
    ANNOTATION_PASSTHROUGH.store(passthrough, std::sync::atomic::Ordering::Relaxed);
    log::info!("[annotation] passthrough={}（poller 下一 tick 生效）", passthrough);
}
