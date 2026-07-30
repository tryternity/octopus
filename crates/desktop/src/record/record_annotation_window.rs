//! 录屏标注 overlay 窗口——录屏开始后显示，用户画标注被 SCK 录进视频。
//!
//! **关键设计**（2026-07-26 Tauri 真实窗口 e2e 验证）：
//! - overlay 用 **always_on_top**（总在最上）——SCK **会**录到 always_on_top 窗口内容
//!   （之前 PyObjC spike 说「不录」是错的——Python subprocess 窗口没真显示）
//! - overlay 窗口比选区大（选区 + 工具栏空间），三选逻辑决定工具栏在选区外/内
//! - Canvas 限制在选区区域（被录），工具栏在选区外（不被录）
//! - 部分穿透：select 工具=穿透模式（工具栏不穿透+选区穿透），标注工具=不穿透
//!
//! 详见 spec `docs/superpowers/specs/2026-07-25-record-area-annotation-design.md`。
//!
//! 仅 macOS。

#![cfg(target_os = "macos")]

use tauri::{AppHandle, Manager};
use crate::ui::window_factory::{build_float_window, FloatWindowSpec};

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

    // ⚠️ 用 display_id 直接查 CGDisplay::bounds()（2026-07-28 修复副屏 bug）。
    // 之前用 Tauri monitor 坐标推断匹配——但 selection.x/y 是相对 display 的局部物理坐标，
    // 加到每个 monitor.position() 后，主屏和副屏都可能匹配（find 返回第一个=主屏）→ overlay 建在主屏。
    // 现在用 CGDisplay::new(display_id).bounds() 直接拿选区所在 display 的逻辑边界。
    #[cfg(target_os = "macos")]
    let (mon_x, mon_y, mon_w, mon_h, scale) = {
        use core_graphics::display::CGDisplay;
        let bounds = CGDisplay::new(display_id).bounds();
        // CGDisplay::bounds() 返回逻辑 points（已含 scale，无需再除）。
        // scale 从 Tauri monitor 匹配拿（bounds 不含 scale 信息）。
        let monitors = app.available_monitors().unwrap_or_default();
        let scale = monitors.iter()
            .find(|m| {
                let sf = m.scale_factor();
                let mx = m.position().x as f64 / sf;
                let my = m.position().y as f64 / sf;
                // CGDisplay bounds.origin 是逻辑坐标，与 Tauri position（物理÷scale）一致
                (bounds.origin.x - mx).abs() < 1.0 && (bounds.origin.y - my).abs() < 1.0
            })
            .map(|m| m.scale_factor())
            .unwrap_or(2.0); // fallback Retina
        (bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height, scale)
    };
    #[cfg(not(target_os = "macos"))]
    let (mon_x, mon_y, mon_w, mon_h, scale) = {
        let monitors = app.available_monitors().unwrap_or_default();
        let mon = monitors.first();
        let scale = mon.map(|m| m.scale_factor()).unwrap_or(1.0);
        let mon = mon.map(|m| {
            (
                m.position().x as f64 / scale,
                m.position().y as f64 / scale,
                m.size().width as f64 / scale,
                m.size().height as f64 / scale,
            )
        }).unwrap_or((0.0, 0.0, 1920.0, 1080.0));
        (mon.0, mon.1, mon.2, mon.3, scale)
    };

    // 选区在屏幕上的全局逻辑位置（物理 → 逻辑 / scale）
    let sel_global_x = mon_x + (x as f64 / scale);
    let sel_global_y = mon_y + (y as f64 / scale);
    let sel_logical_w = w as f64 / scale;
    let sel_logical_h = h as f64 / scale;

    // 窗口 = 选区所在显示器全屏（与截图 Screenshot 同模式）。
    let win_x = mon_x;
    let win_y = mon_y;
    let win_w = mon_w;
    let win_h = mon_h;
    // Canvas 在窗口内的偏移（选区相对显示器原点的逻辑坐标）
    let canvas_offset_x = sel_global_x - mon_x;
    let canvas_offset_y = sel_global_y - mon_y;
    let toolbar_pos = "auto";

    log::info!(
        "[annotation] overlay: display_id={} 选区逻辑 ({},{},{},{}) 全屏窗口 ({},{},{},{}) canvas_offset=({},{})",
        display_id, sel_global_x, sel_global_y, sel_logical_w, sel_logical_h,
        win_x, win_y, win_w, win_h, canvas_offset_x, canvas_offset_y
    );

    // TOOLBAR_ZONE：前端 mount 后通过 invoke set_toolbar_zone 传回工具栏实际位置
    // （前端用 computeToolbarPosition 算，与截图同算法）。后端不再猜测工具栏位置。
    // 初始化为全 0（poller 启动时若前端还没传回，按全屏穿透处理）。
    *TOOLBAR_ZONE.lock() = (0.0, 0.0, 0.0, 0.0);

    // URL 注入选区参数 + 工具栏位置（前端 mount 时解析）
    let url = format!(
        "record-annotation.html?display_id={}&x={}&y={}&width={}&height={}&scale={}&toolbar={}&canvas_ox={}&canvas_oy={}&canvas_w={}&canvas_h={}",
        display_id, x, y, w, h, scale, toolbar_pos, canvas_offset_x, canvas_offset_y, sel_logical_w, sel_logical_h
    );

    let _ = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: &url,
        title: "",
        inner_size: (win_w, win_h),
        visible: true,
        resizable: false,
        position: Some((win_x, win_y)),
        focused: None,
        accept_first_mouse: None,
    })
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

/// 前端把工具栏实际位置（computeToolbarPosition 算出）传回后端。
/// poller 用此区域判定鼠标穿透（工具栏区域不穿透，其他穿透）。
/// 全屏窗口模式下后端不再猜测工具栏位置——前端算好后传回。
#[tauri::command]
pub fn set_toolbar_zone(_app: AppHandle, x: f64, y: f64, w: f64, h: f64) {
    *TOOLBAR_ZONE.lock() = (x, y, w, h);
}
