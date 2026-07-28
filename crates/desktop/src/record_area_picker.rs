//! 录屏区域选区 picker——多屏全屏透明覆盖，用户拖框选区域。
//!
//! 完全复用 screenshot 的窗口创建 + ready 同步 + 坐标换算模式：
//! - 窗口创建参考 `screenshot_commands::start_screenshot`（L67-227）
//! - ready 同步参考 `show_screenshot_window` + READY_COUNT/TOTAL_WINDOWS
//! - 坐标换算参考 `start_scroll_recording`（L935-1010）—— Cocoa frame + Y 翻转 +
//!   compute_selection_global + find_monitor_for_point + compute_physical_crop
//!
//! **与 screenshot 的关键差异**：
//! - **不截图**（picker 是半透明黑遮罩，不显示桌面截图背景）
//! - label 前缀 `record_area_picker_`（与 screenshot_* 分离，互不影响）
//! - URL `area-picker.html`（独立 vite entry）
//! - 选区完成后 emit `record-area://selected` 给 record_config_window（不是入库）
//!
//! 仅 macOS（cfg gate，与 record_commands 同）。

#![cfg(target_os = "macos")]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// 并发门控（防狂按快捷键重复触发）。
static PICKER_BUSY: AtomicBool = AtomicBool::new(false);

/// ready 同步计数器（前端 mount 后 invoke show_record_area_picker_window 累加）。
static READY_COUNT: AtomicU32 = AtomicU32::new(0);
static TOTAL_WINDOWS: AtomicU32 = AtomicU32::new(0);

/// 启动区域选区 picker（多屏全屏透明覆盖）。
///
/// 调用时机：用户在配置浮窗 RecordConfig.tsx 的 area tab 点「选择区域」按钮。
/// 流程：hide 配置浮窗 → 每屏创建 picker 窗口 → 前端 ready 后统一 show →
/// 用户拖框 → mouseup 调 confirm_record_area_picker → emit + 关 picker + show 配置浮窗。
#[tauri::command]
pub async fn start_record_area_picker(app_handle: AppHandle) -> Result<(), String> {
    // 并发门控
    if PICKER_BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Err("area picker already in progress".into());
    }
    struct BusyGuard;
    impl Drop for BusyGuard {
        fn drop(&mut self) {
            PICKER_BUSY.store(false, Ordering::SeqCst);
        }
    }
    let _guard = BusyGuard;

    // hide 配置浮窗（避免双 always_on_top 冲突，picker 显示时浮窗让位）
    crate::record_window::hide_record_window(&app_handle);

    let tauri_monitors = app_handle
        .available_monitors()
        .map_err(|e| format!("获取显示器失败: {}", e))?;

    // 清理旧 picker 窗口（filter record_area_picker_ 前缀）
    let old_labels: Vec<String> = app_handle
        .webview_windows()
        .keys()
        .filter(|k| k.starts_with("record_area_picker_"))
        .cloned()
        .collect();
    for label in &old_labels {
        if let Some(win) = app_handle.get_webview_window(label) {
            let _ = win.destroy();
        }
    }

    // session_id 确保 label 唯一（destroy 异步，新 label 避免冲突）
    let session_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    READY_COUNT.store(0, Ordering::SeqCst);
    TOTAL_WINDOWS.store(0, Ordering::SeqCst);

    // 每屏创建一个全屏透明 picker 窗口
    for (i, tauri_mon) in tauri_monitors.iter().enumerate() {
        let scale = tauri_mon.scale_factor();
        let pos_x = tauri_mon.position().x as f64 / scale; // 物理 → 逻辑
        let pos_y = tauri_mon.position().y as f64 / scale;
        let log_w = tauri_mon.size().width as f64 / scale;
        let log_h = tauri_mon.size().height as f64 / scale;

        let label = format!("record_area_picker_{}_{}", session_id, i);

        let window_result = WebviewWindowBuilder::new(
            &app_handle,
            &label,
            WebviewUrl::App("area-picker.html".into()),
        )
        .title("")
        .decorations(false)
        .always_on_top(true) // picker 本身要浮在最上（让用户看到遮罩）；它不是标注 overlay
        .transparent(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false) // 初始隐藏，前端 ready 后统一 show
        .position(pos_x, pos_y)
        .inner_size(log_w, log_h)
        .build();

        if let Err(e) = &window_result {
            log::error!("Failed to create area picker window '{}': {}", label, e);
            continue;
        }

        TOTAL_WINDOWS.fetch_add(1, Ordering::SeqCst);
        log::info!(
            "Area picker window '{}' at ({},{}) {}x{} (scale {})",
            label, pos_x, pos_y, log_w, log_h, scale,
        );
    }

    // 3s 超时 fallback（防前端 ready 信号丢失导致窗口永久不显示）
    {
        let ah = app_handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let count = READY_COUNT.load(Ordering::SeqCst);
            let total = TOTAL_WINDOWS.load(Ordering::SeqCst);
            if count < total {
                log::warn!("Area picker show timeout: {}/{} ready, force showing", count, total);
                show_all_picker_windows(&ah);
            }
        });
    }

    Ok(())
}

/// 前端 picker 组件 mount 完成后调此命令（累加 READY_COUNT，达总数后统一 show）。
#[tauri::command]
pub fn show_record_area_picker_window(app_handle: AppHandle) {
    let count = READY_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    let total = TOTAL_WINDOWS.load(Ordering::SeqCst);
    if count >= total && total > 0 {
        show_all_picker_windows(&app_handle);
    }
}

fn show_all_picker_windows(app_handle: &AppHandle) {
    let labels: Vec<String> = app_handle
        .webview_windows()
        .keys()
        .filter(|k| k.starts_with("record_area_picker_"))
        .cloned()
        .collect();
    for label in &labels {
        if let Some(window) = app_handle.get_webview_window(label) {
            let _ = window.show();
        }
    }
    // 聚焦主屏窗口（_0 结尾）
    if let Some(ml) = labels.iter().find(|l| l.ends_with("_0")) {
        if let Some(window) = app_handle.get_webview_window(ml) {
            let _ = window.set_focus();
        }
    }
}

/// 用户拖框完成后调（拖完即确认，不需二次点确认）。
///
/// 坐标换算完全复用 screenshot 的 start_scroll_recording 调用链：
/// 1. 拿 picker 窗口原点（Cocoa frame + Y 翻转）
/// 2. compute_selection_global 选区全局化
/// 3. Tauri monitor → MonitorRect[]
/// 4. find_monitor_for_point 选区中心点命中
/// 5. compute_physical_crop 物理裁剪
/// 6. active_display_for_point 查 display_id
/// 7. emit 物理像素给 record_config_window
#[tauri::command]
pub async fn confirm_record_area_picker(
    app_handle: AppHandle,
    win_label: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    use crate::screenshot_geometry::{
        compute_physical_crop, compute_selection_global, find_monitor_for_point, MonitorRect,
    };

    let sel_win = app_handle
        .get_webview_window(&win_label)
        .ok_or_else(|| format!("picker window '{}' not found", win_label))?;

    // 1. 拿窗口原点（Cocoa frame + Y 翻转，与 start_scroll_recording L937-948 完全一致）
    let primary_h = crate::screenshot_commands::get_primary_screen_height();
    let (win_origin_x, win_origin_y) = match crate::screenshot_commands::get_window_cocoa_frame(&sel_win) {
        Some((cx, cy, _, ch)) => {
            let oy = primary_h - (cy + ch);
            log::info!(
                "[area-picker] cocoa_frame=({},{},{},{}) primary_h={} → win_origin=({},{})",
                cx, cy, ch, ch, primary_h, cx, oy
            );
            (cx, oy)
        }
        None => {
            log::warn!("[area-picker] get_window_cocoa_frame 失败，用 (0,0) 兜底");
            (0.0, 0.0)
        }
    };
    log::info!(
        "[area-picker] sel_local=({},{},{},{}) → global=({},{})",
        x, y, w, h, win_origin_x + x, win_origin_y + y
    );

    // 2. 选区全局化
    let sel = compute_selection_global(win_origin_x, win_origin_y, x, y, w, h);

    // 3. 构造 MonitorRect[]（Tauri monitor 物理 → 逻辑 / scale）
    let monitors_raw = app_handle.available_monitors().unwrap_or_default();
    let monitors: Vec<MonitorRect> = monitors_raw
        .iter()
        .map(|m| {
            let sf = m.scale_factor();
            MonitorRect {
                x: m.position().x as f64 / sf,
                y: m.position().y as f64 / sf,
                w: m.size().width as f64 / sf,
                h: m.size().height as f64 / sf,
                scale: sf,
            }
        })
        .collect();

    // 4. 命中检测（选区中心点）
    let center_x = sel.x + w / 2.0;
    let center_y = sel.y + h / 2.0;
    log::info!(
        "[area-picker] monitors: {}",
        monitors.iter().enumerate()
            .map(|(i, m)| format!("[{}]({},{},{},{},scale={})", i, m.x, m.y, m.w, m.h, m.scale))
            .collect::<Vec<_>>().join(" ")
    );
    log::info!("[area-picker] 选区中心=({},{})", center_x, center_y);
    let mon_idx = find_monitor_for_point(&monitors, center_x, center_y)
        .or_else(|| (!monitors.is_empty()).then_some(0));
    let mon_idx = match mon_idx {
        Some(idx) => idx,
        None => return Err("找不到选区所在的显示器".into()),
    };

    // 5. 物理裁剪
    let crop = compute_physical_crop(&sel, &monitors[mon_idx]);

    // 6. 查 display_id
    let display_id = crate::screenshot_commands::active_display_for_point(center_x, center_y);
    if display_id == 0 {
        return Err("无法确定选区所在的 display_id".into());
    }

    log::info!(
        "[area-picker] selected: display_id={} phys ({},{},{},{}) [monitor {} scale {}]",
        display_id, crop.px, crop.py, crop.pw, crop.ph,
        monitors[mon_idx].x, monitors[mon_idx].scale
    );

    // 7. emit 给 record_config_window（物理像素，与 protocol.rs::Source::Area 对齐）
    let payload = serde_json::json!({
        "display_id": display_id,
        "x": crop.px as i32,
        "y": crop.py as i32,
        "width": crop.pw,
        "height": crop.ph,
    });
    let _ = app_handle.emit("record-area://selected", payload);

    // 8. 关 picker。
    // 不 show 配置浮窗——新流程是「框选完直接开始录制」（listener 收到 record-area://selected
    // 后立即调 startRecordingWithSource → 成功后 getCurrentWindow().hide()）。
    // 如果这里 show，会和 listener 的 hide 并发导致「设置窗口一闪而过」。
    // cancel_record_area_picker 才 show（用户取消框选 → 回配置）。
    close_all_picker_windows(&app_handle);

    Ok(())
}

/// Esc / 右键取消（关 picker + show 配置浮窗）。
#[tauri::command]
pub fn cancel_record_area_picker(app_handle: AppHandle) {
    log::info!("[area-picker] cancelled");
    close_all_picker_windows(&app_handle);
    crate::record_window::show_record_window(&app_handle);
}

fn close_all_picker_windows(app_handle: &AppHandle) {
    let labels: Vec<String> = app_handle
        .webview_windows()
        .keys()
        .filter(|k| k.starts_with("record_area_picker_"))
        .cloned()
        .collect();
    for label in &labels {
        if let Some(window) = app_handle.get_webview_window(label) {
            let _ = window.destroy();
        }
    }
    READY_COUNT.store(0, Ordering::SeqCst);
    TOTAL_WINDOWS.store(0, Ordering::SeqCst);
}
