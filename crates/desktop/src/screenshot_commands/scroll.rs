//! screenshot_commands 滚动截图子模块（长截图拼接）。
//!
//! 2026-07-30 从原 screenshot_commands.rs 拆出（Task 1.2）。
//! 包含：close_all_screenshot_windows + register/unregister_scroll_esc + ScrollStopMode /
//! ScrollRecordingGuard / SendApp / 全部 macOS Cocoa helper + InteractiveRect +
//! start_scroll_recording（580 行巨函数，原样搬）+ stop_scroll_recording(_with_mode)。
//!
//! `start_scroll_recording` 内部逻辑紧密耦合（选区定位 → 显示器检测 → 滚动循环 → 拼接），
//! 本次纯搬家不拆函数体。

use parking_lot::Mutex;
use tauri::{Emitter, Manager};
use base64::{Engine, engine::general_purpose};
use octopus_clipboard::ClipboardHandle;

use crate::error_util::e2s_ctx;
use super::{
    TOTAL_WINDOWS,
    format_file_size,
    get_primary_screen_height,
    get_window_cocoa_frame,
    right_mouse_button_down,
};

pub(crate) fn close_all_screenshot_windows(app_handle: &tauri::AppHandle) {
    let labels: Vec<String> = app_handle
        .webview_windows()
        .keys()
        .filter(|k| k.starts_with("screenshot_"))
        .cloned()
        .collect();
    for label in &labels {
        if let Some(win) = app_handle.get_webview_window(label) {
            let _ = win.destroy();
        }
    }
    TOTAL_WINDOWS.store(0, std::sync::atomic::Ordering::SeqCst);
    crate::tray::update_tray_screenshot_label(false);
}

// ── 滚动截图 ──

/// 用户停止时的操作模式：保存文件 / 复制入库 / 取消
#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
enum ScrollStopMode {
    Copy = 0,
    Save = 1,
    Cancel = 2,
}

static SCROLL_STOP_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

static SCROLL_RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// scrolling 启动时注册全局 ESC，handler 调 stop_scroll 逻辑。
///
/// scrolling 时键盘焦点在下层应用（activate_prev_app 把焦点交给被滚动 app，让用户能滚动），
/// Screenshot 窗口的 DOM 级 onKeyDown / keydown listener 收不到 ESC。只能用全局快捷键。
/// 与 `record_hotkey::register_stop_hotkey` 同范式（录屏/scroll 截图互斥，不会同时注册）。
///
/// handler 直接在后端 stop（不走前端 invoke，因为前端收不到 ESC）：
/// 1. 设 SCROLL_STOP_MODE = Copy（默认，与 stop_scroll_recording 一致）
/// 2. 设 SCROLL_RECORDING = false（让消费循环退出，任务体收尾处理 finalize/入库/关窗）
fn register_scroll_esc(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let esc: Shortcut = "Escape".parse().map_err(|e| e2s_ctx("parse scroll ESC", e))?;
    app.global_shortcut()
        .on_shortcut(esc, move |_app, _scut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            log::info!("[scroll] ESC 全局快捷键触发 → 停止 scroll（焦点在下层应用，DOM 收不到）");
            // 默认 copy 模式（与 stop_scroll_recording 命令一致；用户用按钮时走 _with_mode 设其他模式）
            SCROLL_STOP_MODE.store(ScrollStopMode::Copy as u8, std::sync::atomic::Ordering::SeqCst);
            SCROLL_RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
            // 不做其他——任务体会看到 SCROLL_RECORDING=false 退出循环，走 finalize/入库/关窗
        })
        .map_err(|e| e2s_ctx("register scroll ESC", e))?;
    log::info!("[scroll] 全局 ESC 已注册（scrolling 模式）");
    Ok(())
}

/// scrolling 停止时注销全局 ESC，让其他窗口的 DOM 级 ESC 重新生效。
/// 未注册时 unregister 是 no-op，不会报错。失败仅 warn 不阻断。
fn unregister_scroll_esc(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
    match "Escape".parse::<Shortcut>() {
        Ok(sc) => {
            if let Err(e) = app.global_shortcut().unregister(sc) {
                log::warn!("[scroll] 全局 ESC 注销失败（不影响功能）: {e}");
            } else {
                log::info!("[scroll] 全局 ESC 已注销（scrolling 结束）");
            }
        }
        Err(e) => log::warn!("[scroll] ESC 解析失败（无法注销）: {e}"),
    }
}


/// RAII 守卫：drop 时把 `SCROLL_RECORDING` 重置为 false + 注销 scrolling 全局 ESC。
///
/// `start_scroll_recording` 在 `swap(true)` 成功后 spawn 异步任务，任务体里的早返回
/// （截图窗口已关闭 / CG 获取活动显示器失败 / 首帧截取失败）以及 panic 都不会重置标志，
/// 会导致 `SCROLL_RECORDING` 永久停留在 true —— 此后任何滚动截图尝试都立即返回
/// "already in progress"，必须重启应用才能恢复。在 spawn 开头持有一份守卫，任何退出
/// 路径（早返回 / 正常结束 / panic / runtime 取消）都会 drop 它 → 重置标志 + 注销 ESC，幂等无副作用
/// （正常停止路径由前端 `stop_scroll_recording` 先设 false 让循环退出，再 drop 守卫重复置 false）。
///
/// 持有 Option<AppHandle>：drop 时若有则调 unregister_scroll_esc（scrolling 时 register 过）。
/// None 表示启动失败前未 register（早返回路径），drop 时跳过 unregister。
struct ScrollRecordingGuard {
    app: Option<tauri::AppHandle>,
}
impl Drop for ScrollRecordingGuard {
    fn drop(&mut self) {
        SCROLL_RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(app) = &self.app {
            #[cfg(target_os = "macos")]
            unregister_scroll_esc(app);
        }
    }
}

#[cfg(target_os = "macos")]
struct SendApp(objc2::rc::Retained<objc2_app_kit::NSRunningApplication>);
unsafe impl Send for SendApp {}
unsafe impl Sync for SendApp {}

#[cfg(target_os = "macos")]
static PREV_ACTIVE_APP: Mutex<Option<SendApp>> = Mutex::new(None);

#[cfg(target_os = "macos")]
pub(crate) fn save_frontmost_app() {
    use objc2_app_kit::{NSWorkspace, NSRunningApplication};
    let workspace = NSWorkspace::sharedWorkspace();
    if let Some(app) = workspace.frontmostApplication() {
        let curr = NSRunningApplication::currentApplication();
        let is_current = app.processIdentifier() == curr.processIdentifier();
        if !is_current {
            if let Some(name) = app.localizedName() {
                log::info!("Scroll screenshot: saved frontmost app '{}'", name);
            }
            let mut guard = PREV_ACTIVE_APP.lock();
            *guard = Some(SendApp(app));
        } else {
            log::info!("Scroll screenshot: ignored saving current app");
        }
    }
}

#[cfg(target_os = "macos")]
fn activate_prev_app(win: &tauri::WebviewWindow) {
    let app_opt = {
        let guard = PREV_ACTIVE_APP.lock();
        guard.as_ref().map(|p| p.0.clone())
    };
    let _ = win.run_on_main_thread(move || {
        if let Some(app) = app_opt {
            // NSApplicationActivateAllWindows = 1 << 0（1 << 1 在 macOS 14+ deprecated，详见 activation.rs）
            let success = app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(1 << 0));
            log::info!("Scroll screenshot: activated previous app on main thread, success={}", success);
        } else {
            log::info!("Scroll screenshot: no previous app to activate, deactivating ourselves");
            if let Some(mtm) = objc2::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                app.deactivate();
            }
        }
    });
}

/// macOS：获取指定坐标下最上层非截图应用的 window owner PID。
#[cfg(target_os = "macos")]
fn get_window_pid_at_point(x: f64, y: f64) -> Option<i32> {
    use core_graphics::display::CGDisplay;
    let windows = CGDisplay::window_list_info(
        core_graphics::display::kCGWindowListOptionOnScreenOnly,
        None,
    )?;
    let curr_pid = std::process::id() as i32;

    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use core_foundation::number::CFNumber;

    for item in windows.iter() {
        let dict_ref = *item as CFTypeRef;
        if dict_ref.is_null() { continue; }
        let dict: CFDictionary<CFString, CFTypeRef> = unsafe { TCFType::wrap_under_get_rule(dict_ref as *const _) };

        let key_pid = CFString::new("kCGWindowOwnerPID");
        let pid_item = dict.get(&key_pid);
        let pid_ptr: CFTypeRef = *pid_item;
        if pid_ptr.is_null() { continue; }
        let pid_num: CFNumber = unsafe { TCFType::wrap_under_get_rule(pid_ptr as *const _) };
        let pid = pid_num.to_i32()?;
        if pid == curr_pid { continue; }

        // 检查窗口 bounds 是否包含该点
        let key_bounds = CFString::new("kCGWindowBounds");
        let bounds_item = dict.get(&key_bounds);
        let bounds_ptr: CFTypeRef = *bounds_item;
        if bounds_ptr.is_null() { continue; }
        let bdict: CFDictionary<CFString, CFTypeRef> = unsafe { TCFType::wrap_under_get_rule(bounds_ptr as *const _) };
        let get_f64 = |key: &str| -> f64 {
            let k = CFString::new(key);
            let item = bdict.get(&k);
            let ptr: CFTypeRef = *item;
            if ptr.is_null() { return 0.0; }
            let n: CFNumber = unsafe { TCFType::wrap_under_get_rule(ptr as *const _) };
            n.to_f64().unwrap_or(0.0)
        };
        let (bx, by, bw, bh) = (get_f64("X"), get_f64("Y"), get_f64("Width"), get_f64("Height"));
        if x >= bx && x < bx + bw && y >= by && y < by + bh {
            return Some(pid);
        }
    }
    None
}

/// macOS：通过 PID 激活应用（主线程执行）。
#[cfg(target_os = "macos")]
fn activate_app_by_pid(ah: &tauri::AppHandle, pid: i32) {
    use objc2_app_kit::NSRunningApplication;
    // 通过任意窗口的 run_on_main_thread 在主线程执行激活
    if let Some(win) = ah.webview_windows().values().next() {
        let _ = win.run_on_main_thread(move || {
            if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
                // NSApplicationActivateAllWindows = 1 << 0（1 << 1 在 macOS 14+ deprecated）
                let success = app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(1 << 0));
                if success {
                    log::debug!("[scroll] activated app pid={} for scroll focus", pid);
                }
            }
        });
    }
}

/// macOS：获取 NSWindow 的 windowNumber（用于 CGWindowListCreateImage 排除 overlay 窗口）
#[cfg(target_os = "macos")]
fn get_window_number(win: &tauri::WebviewWindow) -> Option<u32> {
    let ptr = win.ns_window().ok()?;
    if ptr.is_null() { return None; }
    let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
    Some(ns_win.windowNumber() as u32)
}

#[cfg(not(target_os = "macos"))]
fn get_window_number(_win: &tauri::WebviewWindow) -> Option<u32> { None }

#[cfg(target_os = "macos")]
fn set_window_ignores_mouse_events(win: &tauri::WebviewWindow, ignore: bool) {
    let win_clone = win.clone();
    let label = win.label().to_string();
    let _ = win.run_on_main_thread(move || {
        if let Ok(ptr) = win_clone.ns_window() {
            if !ptr.is_null() {
                let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                ns_win.setIgnoresMouseEvents(ignore);
                log::info!("[scroll-diag] NSWindow '{}' setIgnoresMouseEvents({}) completed on main thread", label, ignore);
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn set_window_ignores_mouse_events(win: &tauri::WebviewWindow, ignore: bool) {
    let _ = win.set_ignore_cursor_events(ignore);
}





#[cfg(target_os = "macos")]
fn set_app_active_on_main(win: &tauri::WebviewWindow, active: bool) {
    use objc2_app_kit::NSApplication;
    use objc2::MainThreadMarker;
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let _ = win.run_on_main_thread(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            if active {
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
            } else {
                app.deactivate();
            }
        }
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
}

/// 前端传递的交互区域（工具栏、预览窗等），窗口局部逻辑坐标。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct InteractiveRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[tauri::command]
pub async fn start_scroll_recording(
    x: f64, y: f64, w: f64, h: f64,
    win_label: String,
    interactive_rects: Vec<InteractiveRect>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if SCROLL_RECORDING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("Scroll recording is already in progress".into());
    }

    let ah = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        // RAII 守卫：本任务体任何退出（早返回 / 正常结束 / panic / runtime 取消）都重置
        // SCROLL_RECORDING=false + 注销 scrolling 全局 ESC，避免功能永久锁死或 ESC 残留。
        // app 字段初始为 None——register_scroll_esc 成功后才设 Some（drop 时才 unregister）。
        let mut _scroll_guard = ScrollRecordingGuard { app: None };
        // ── 通过 win_label 定位选区所在的截图窗口（spec §6.4）──
        let sel_win = match ah.get_webview_window(&win_label) {
            Some(w) => w,
            None => {
                log::error!("start_scroll_recording: window '{}' not found", win_label);
                return;
            }
        };

        // 窗口原点：用 CGDisplay::bounds() 获取 Quartz 逻辑原点（最可靠）。
        // 截图窗口全屏覆盖显示器，所以窗口原点 = 显示器逻辑原点。
        // outer_position()/sf 在混合 DPI 下可能不准（Tauri 物理 vs Quartz 逻辑断层）。
        #[cfg(target_os = "macos")]
        let (win_origin_x, win_origin_y) = {
            let primary_h = get_primary_screen_height();
            if let Some((cx, cy, _, ch)) = get_window_cocoa_frame(&sel_win) {
                (cx, primary_h - (cy + ch))
            } else {
                (0.0, 0.0)
            }
        };
        #[cfg(not(target_os = "macos"))]
        let (win_origin_x, win_origin_y) = {
            let sf = sel_win.scale_factor().unwrap_or(1.0);
            match sel_win.outer_position() {
                Ok(p) => (p.x as f64 / sf, p.y as f64 / sf),
                Err(_) => (0.0, 0.0),
            }
        };
        log::debug!("[scroll] win_origin=({},{}) sel_local=({},{},{},{})", win_origin_x, win_origin_y, x, y, w, h);
        // 选区的全局逻辑坐标 = 窗口原点 + CSS 偏移
        let sel = crate::screenshot_geometry::compute_selection_global(
            win_origin_x, win_origin_y, x, y, w, h,
        );
        let sel_global_x = sel.x;
        let sel_global_y = sel.y;

        // ── 找到选区所在的显示器 + scale ──
        let monitors_raw = ah.available_monitors().unwrap_or_default();
        let monitors: Vec<crate::screenshot_geometry::MonitorRect> = monitors_raw
            .iter()
            .map(|m| {
                let sf = m.scale_factor();
                crate::screenshot_geometry::MonitorRect {
                    x: m.position().x as f64 / sf,
                    y: m.position().y as f64 / sf,
                    w: m.size().width as f64 / sf,
                    h: m.size().height as f64 / sf,
                    scale: sf,
                }
            })
            .collect();
        let mon_idx = crate::screenshot_geometry::find_monitor_for_point(
            &monitors,
            sel_global_x + w / 2.0,
            sel_global_y + h / 2.0,
        ).or_else(|| (!monitors.is_empty()).then_some(0));
        let (scale, mon_logical_x, mon_logical_y, _mon_phys_x, _mon_phys_y): (f64, f64, f64, i32, i32) = match mon_idx {
            Some(idx) => {
                let m = &monitors[idx];
                let mr = &monitors_raw[idx];
                (m.scale, m.x, m.y, mr.position().x, mr.position().y)
            }
            None => (1.0, 0.0, 0.0, 0, 0),
        };
        let mon_rect = crate::screenshot_geometry::MonitorRect {
            x: mon_logical_x, y: mon_logical_y, w: 0.0, h: 0.0, scale,
        };

        // 选区在该显示器内的物理像素偏移
        let crop = crate::screenshot_geometry::compute_physical_crop(&sel, &mon_rect);
        let px = crop.px;
        let py = crop.py;
        let pw = crop.pw;
        let ph = crop.ph;

        log::info!(
            "Scroll recording: win_label={}, sel=({},{},{},{}), global=({},{},{}), scale={}, crop phys=({},{},{},{})",
            win_label, x, y, w, h, sel_global_x, sel_global_y, scale, scale,
            px, py, pw, ph,
        );

        // ── macOS：获取 display_id + overlay windowNumber（spec §6.4 CGWindowList 排除）──
        #[cfg(target_os = "macos")]
        let (_display_id, exclude_wid, target_wid) = {
            use core_graphics::display::CGDisplay;
            let displays = match CGDisplay::active_displays() {
                Ok(d) => d,
                Err(_) => { log::error!("CGGetActiveDisplayList failed"); return; }
            };
            let hit = displays.iter().find(|&&id| {
                let bounds = CGDisplay::new(id).bounds();
                let cx = sel_global_x + w / 2.0;
                let cy = sel_global_y + h / 2.0;
                cx >= bounds.origin.x && cx < bounds.origin.x + bounds.size.width
                    && cy >= bounds.origin.y && cy < bounds.origin.y + bounds.size.height
            }).copied().unwrap_or(0);
            let wid = get_window_number(&sel_win).unwrap_or(0);

            // Find target window ID from the app under the selection area (not PREV_ACTIVE_APP).
            // 用选区中心点检测下方的应用窗口，确保截到的是选区下方的真实内容。
            let target_wid = {
                let cx = sel_global_x + w / 2.0;
                let cy = sel_global_y + h / 2.0;
                if let Some(pid) = get_window_pid_at_point(cx, cy) {
                    let found = octopus_capx::capture::find_window_id_by_pid(pid);
                    log::info!("Scroll capture: app under selection (pid={}) yielded window ID {:?}", pid, found);
                    found
                } else {
                    None
                }
            };

            log::debug!("[scroll-diag] display_id={}, exclude_wid={} (windowNumber), target_wid={:?}, displays={:?}",
                hit, wid, target_wid, displays);
            (hit, wid, target_wid)
        };

        #[cfg(not(target_os = "macos"))]
        let exclude_wid: u32 = 0;
        #[cfg(not(target_os = "macos"))]
        let target_wid: Option<u32> = None;

        // 获取所有截图窗口 label（用于 set_ignore_cursor_events）
        let scroll_labels: Vec<String> = ah
            .webview_windows()
            .keys()
            .filter(|k| k.starts_with("screenshot_"))
            .cloned()
            .collect();

        // 录制开始：保持 always_on_top(true) + set_ignore_cursor_events(true) + deactivate
        #[cfg(target_os = "macos")]
        {
            for label in &scroll_labels {
                if let Some(win) = ah.get_webview_window(label) {
                    let _ = win.set_always_on_top(true);
                }
            }
        }
        for label in &scroll_labels {
            if let Some(win) = ah.get_webview_window(label) {
                set_window_ignores_mouse_events(&win, true);
            }
        }
        // scrolling 全局 ESC：焦点将交给下层应用（activate_prev_app），DOM 收不到 ESC，
        // 用全局快捷键兜底。register 成功后让 guard 持有 app（drop 时 unregister）。
        // 失败仅 warn 不阻断——最坏情况是 scrolling 时 ESC 不响应（仍可用预览窗按钮停止）。
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = register_scroll_esc(&ah) {
                log::warn!("[scroll] 全局 ESC 注册失败（不影响 scroll，可用预览窗按钮停止）: {e}");
            } else {
                _scroll_guard.app = Some(ah.clone());
            }
        }
        #[cfg(target_os = "macos")]
        {
            activate_prev_app(&sel_win);
            log::debug!("[scroll] manual mode: activated previous app for scroll passthrough");
            // Wait 120ms for window activation transition to complete and repaint in active state
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }

        // 独立鼠标监听线程：30ms 高频轮询，与截图循环解耦。
        // 鼠标在任意交互区域（工具栏/预览窗）→ set_ignore_cursor_events(false)（可点击）；
        // 离开 → set_ignore_cursor_events(true)（滚动穿透）。不调 activate/deactivate。
        // 鼠标穿透轮询：macOS 专属（CGEvent 全局鼠标追踪 + set_ignore_cursor_events 穿透
        // + 激活下方应用）。core_graphics 仅 macOS 可用，整个轮询线程 cfg gate；
        // 非 macOS 不启动（滚动截图穿透为 macOS 专属优化），interactive_rects 标记已用。
        #[cfg(target_os = "macos")]
        {
            let mon_labels = scroll_labels.clone();
            let mon_ah = ah.clone();
            let mon_winx = win_origin_x;
            let mon_winy = win_origin_y;
            let mon_rects = interactive_rects;
            // 选区全局几何（用于右键取消：选区外右键停止 scroll）
            let mon_sel_x = sel_global_x;
            let mon_sel_y = sel_global_y;
            let mon_sel_w = w;
            let mon_sel_h = h;
            tauri::async_runtime::spawn(async move {
                use core_graphics::event::CGEvent;
                use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
                let mut poll = tokio::time::interval(std::time::Duration::from_millis(16));
                let mut cur_passthrough = true;
                let mut last_active_pid: i32 = 0;
                let mut activate_check_count = 0u32;
                let mut last_check_x: f64 = 0.0;
                let mut last_check_y: f64 = 0.0;
                // 右键边沿检测：只在 false→true（刚按下）瞬间触发停止，避免持续按住时反复触发
                let mut prev_right_down = false;
                while SCROLL_RECORDING.load(std::sync::atomic::Ordering::SeqCst) {
                    poll.tick().await;
                    let (mouse_x, mouse_y) = if let Ok(src) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
                        if let Ok(evt) = CGEvent::new(src) {
                            let loc = evt.location();
                            (loc.x, loc.y)
                        } else { (0.0, 0.0) }
                    } else { (0.0, 0.0) };

                    let lx = mouse_x - mon_winx;
                    let ly = mouse_y - mon_winy;
                    let in_interactive = mon_rects.iter().any(|r| {
                        lx >= r.x && lx <= r.x + r.width && ly >= r.y && ly <= r.y + r.height
                    });
                    let want = !in_interactive;
                    if want != cur_passthrough {
                        for label in &mon_labels {
                            if let Some(win) = mon_ah.get_webview_window(label) {
                                set_window_ignores_mouse_events(&win, want);
                            }
                        }
                        cur_passthrough = want;
                    }

                    // 右键取消：选区外（含穿透区）右键按下 → 停止 scroll。
                    // 边沿检测：只在刚按下瞬间触发（prev=false 且 curr=true）。
                    // 选区内右键不处理（避免与标注操作冲突，且选区内右键本就少见）。
                    let curr_right_down = right_mouse_button_down();
                    if curr_right_down && !prev_right_down {
                        let in_sel = mouse_x >= mon_sel_x
                            && mouse_x <= mon_sel_x + mon_sel_w
                            && mouse_y >= mon_sel_y
                            && mouse_y <= mon_sel_y + mon_sel_h;
                        if !in_sel {
                            log::info!("[scroll] 选区外右键按下 → 停止 scroll");
                            SCROLL_STOP_MODE.store(ScrollStopMode::Copy as u8, std::sync::atomic::Ordering::SeqCst);
                            SCROLL_RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
                            // 不 break——让循环条件自然退出（保持与其他停止路径一致）
                        }
                    }
                    prev_right_down = curr_right_down;

                    // 每 ~800ms（每 50 个 tick）且鼠标移动超过 10px 时检测鼠标下方的应用。
                    // CGWindowListCopyWindowInfo 是昂贵的系统 API，避免高频空转。
                    activate_check_count += 1;
                    if want && activate_check_count >= 50 {
                        activate_check_count = 0;
                        let moved = (mouse_x - last_check_x).abs() + (mouse_y - last_check_y).abs();
                        if moved > 10.0 {
                            last_check_x = mouse_x;
                            last_check_y = mouse_y;
                            if let Some(pid) = get_window_pid_at_point(mouse_x, mouse_y) {
                                if pid != last_active_pid {
                                    activate_app_by_pid(&mon_ah, pid);
                                    last_active_pid = pid;
                                }
                            }
                        }
                    }
                }
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = interactive_rects;
        }


        // ── 首帧（只截选区区域，排除 overlay 窗口）──
        let target_wid_first = target_wid;
        let first_result = tokio::task::spawn_blocking(move || {
            #[cfg(target_os = "macos")]
            {
                let cap = if let Some(wid) = target_wid_first {
                    octopus_capx::capture::capture_window_region(
                        wid, sel_global_x, sel_global_y, w, h,
                    )?
                } else {
                    octopus_capx::capture::capture_region_excluding_window(
                        exclude_wid, sel_global_x, sel_global_y, w, h,
                    )?
                };
                let img = image::RgbaImage::from_raw(cap.width, cap.height, cap.rgba_bytes)
                    .ok_or_else(|| anyhow::anyhow!("failed to create RgbaImage"))?;
                anyhow::Ok(img)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let full = octopus_capx::capture::capture_single_monitor(mon_phys_x, mon_phys_y)?;
                // 直接内存只读裁剪返回 RgbaImage，避免全量 Clone 与 PNG 往返
                let img = octopus_capx::capture::crop_region_rgba_direct(full.width, full.height, &full.rgba_bytes, px, py, pw, ph)?;
                anyhow::Ok(img)
            }
        }).await;

        let first_img = match first_result { Ok(Ok(img)) => img, _ => return };
        let mut stitcher = octopus_capx::stitch::Stitcher::new(first_img, Default::default());

        let _ = ah.emit("scroll://started", ());

        // ── 生产/消费解耦（借鉴 snow-shot，对比 spec §3-A）──
        // 生产 task：高频截屏 → watch 通道（覆盖=丢旧保新，内存恒定）；
        // 消费循环（本 task）：出最新帧 → 拼接 → 预览编码 → emit。
        // 这样 capture 节拍不再被 process_frame / preview 编码拖漂。
        let (frame_tx, mut frame_rx) =
            tokio::sync::watch::channel::<Option<image::RgbaImage>>(None);

        let ah2 = ah.clone();

        // 生产 task：30ms 截屏，send 进 watch（覆盖前值 = 丢旧保新）。
        // RECORDING=false → 退出循环 → frame_tx 随 task drop → 消费 changed() 得 Err。
        let prod_handle = tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(30));
            interval.tick().await;
            while SCROLL_RECORDING.load(std::sync::atomic::Ordering::SeqCst) {
                interval.tick().await;

                // 截屏：只截选区区域，CGWindowList 排除 overlay 窗口（只截底层应用内容）
                let target_wid_loop = target_wid;
                let capture_result = tokio::task::spawn_blocking(move || {
                    #[cfg(target_os = "macos")]
                    {
                        let cap = if let Some(wid) = target_wid_loop {
                            octopus_capx::capture::capture_window_region(
                                wid, sel_global_x, sel_global_y, w, h,
                            )?
                        } else {
                            octopus_capx::capture::capture_region_excluding_window(
                                exclude_wid, sel_global_x, sel_global_y, w, h,
                            )?
                        };
                        let img = image::RgbaImage::from_raw(cap.width, cap.height, cap.rgba_bytes)
                            .ok_or_else(|| anyhow::anyhow!("failed to create RgbaImage"))?;
                        anyhow::Ok(img)
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        let full = octopus_capx::capture::capture_single_monitor(mon_phys_x, mon_phys_y)?;
                        // 直接只读内存裁剪，避免全量 Clone
                        let img = octopus_capx::capture::crop_region_rgba_direct(full.width, full.height, &full.rgba_bytes, px, py, pw, ph)?;
                        anyhow::Ok(img)
                    }
                }).await;

                let frame = match capture_result { Ok(Ok(img)) => img, _ => continue };
                // watch send 覆盖前值：消费跟不上时自动丢旧保新，截帧节拍不被拖
                let _ = frame_tx.send(Some(frame));
            }
            // frame_tx 随 task 结束 drop
        });

        // 消费循环：出最新帧 → 拼接 → 预览编码 → emit。stitcher &mut 全程在此侧，无共享。
        let mut last_frame: Option<image::RgbaImage> = None;
        while let Ok(()) = frame_rx.changed().await {
            let frame = match frame_rx.borrow().clone() {
                Some(f) => f,
                None => continue, // 首帧前 sentinel
            };
            // last_frame 用于 finalize，process_frame 只借用——避免双重 clone
            let _added = stitcher.process_frame(&frame).unwrap_or(false);
            last_frame = Some(frame);

            // 截图帧 JPEG + 预览图编码移入 spawn_blocking（CPU 密集，避免阻塞 async 线程）
            let preview_w = 400u32;
            let max_preview_h = 1200u32;
            let canvas_h_now = stitcher.height();
            let canvas_w_now = stitcher.canvas_w();
            let (crop_src_h, crop_y) = crate::screenshot_geometry::compute_preview_crop(
                canvas_h_now, canvas_w_now, preview_w, max_preview_h,
            );
            let canvas_buf_slice = stitcher.canvas_buf_slice(crop_y, crop_src_h);
            let frame_for_jpg = last_frame.as_ref().unwrap().clone();
            let scale_for_phys = scale;

            // 预览编码 fire-and-forget：不 await，避免阻塞下一帧拼接（关键路径）。
            // 若 await：消费跟不上生产 → watch 丢帧 → canvas 滞后 → 单帧累积位移
            // 超 NCC search 失配（e2e 实测 772px/帧致拼接中断）。
            let emit_ah = ah2.clone();
            tokio::task::spawn_blocking(move || {
                // 选区实时画面 JPEG
                let mut frame_jpg = Vec::new();
                let frame_rgb = image::DynamicImage::ImageRgba8(frame_for_jpg).into_rgb8();
                let mut jpg_enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut frame_jpg, 80);
                let _ = jpg_enc.encode(&frame_rgb, frame_rgb.width(), frame_rgb.height(), image::ExtendedColorType::Rgb8);
                let frame_b64 = general_purpose::STANDARD.encode(&frame_jpg);

                // 预览图：从 canvas_buf 底部切片重建小 RgbaImage
                // P1-4 优化（2026-07-17）：预览编码 PNG→JPEG——1-2MB→100-300KB，
                // 编码快 3-5×，肉眼无差。预览只是视觉反馈不是入库数据。
                let canvas_cropped = image::RgbaImage::from_raw(canvas_w_now, crop_src_h, canvas_buf_slice)
                    .unwrap_or_else(|| image::RgbaImage::new(canvas_w_now, crop_src_h));
                let preview_h = (preview_w * canvas_cropped.height() / canvas_cropped.width()).min(max_preview_h);
                let preview = image::imageops::resize(&canvas_cropped, preview_w, preview_h, image::imageops::FilterType::Triangle);
                let preview_rgb = image::DynamicImage::ImageRgba8(preview).into_rgb8();
                let mut preview_jpg = Vec::new();
                let mut jpg_enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut preview_jpg, 80);
                let _ = jpg_enc.encode(&preview_rgb, preview_rgb.width(), preview_rgb.height(), image::ExtendedColorType::Rgb8);
                let preview_b64 = general_purpose::STANDARD.encode(&preview_jpg);

                let phys_height = (canvas_h_now as f64 / scale_for_phys) as u32;

                let _ = emit_ah.emit("scroll://frame", serde_json::json!({
                    "frame": frame_b64,
                    "preview": preview_b64,
                    "height": canvas_h_now,
                    "phys_height": phys_height,
                }));
            });
        }

        // 生产 task 必先退出（RECORDING false → frame_tx drop → 消费 changed Err），
        // 等其收尾再进入停止流程（finalize / 入库 / 窗口管理）。
        let _ = prod_handle.await;

        // 录制结束：先恢复鼠标事件 + 重新激活 app（避免假死）
        for label in &scroll_labels {
            if let Some(win) = ah.get_webview_window(label) {
                set_window_ignores_mouse_events(&win, false);
            }
        }
        #[cfg(target_os = "macos")]
        set_app_active_on_main(&sel_win, true);

        // 先关闭截图窗口（用户感知"立即停止"）
        close_all_screenshot_windows(&ah);

        // 补全最后一帧的完整可见区域（含底部 sticky footer）
        if let Some(ref lf) = last_frame {
            let _ = stitcher.finalize(lf);

            // finalize 后再 emit 一帧预览（spawn_blocking 避免阻塞事件循环）
            let canvas = stitcher.canvas().clone();
            let preview_b64 = tokio::task::spawn_blocking(move || {
                let preview_w = 400u32;
                let max_preview_h = 1200u32;
                let (crop_src_h, crop_y) = crate::screenshot_geometry::compute_preview_crop(
                    canvas.height(), canvas.width(), preview_w, max_preview_h,
                );
                let canvas_cropped = image::imageops::crop_imm(&canvas, 0, crop_y, canvas.width(), crop_src_h).to_image();
                let preview_h = (preview_w * canvas_cropped.height() / canvas_cropped.width()).min(max_preview_h);
                // finalize 预览与实时帧（line 1254）用同一 Triangle 滤波——
                // CatmullRom（4-tap）比 Triangle（2-tap）慢 3-5×，finalize 关键路径无感差异。
                let preview = image::imageops::resize(&canvas_cropped, preview_w, preview_h, image::imageops::FilterType::Triangle);
                let mut preview_png = Vec::new();
                let _ = preview.write_to(&mut std::io::Cursor::new(&mut preview_png), image::ImageFormat::Png);
                general_purpose::STANDARD.encode(&preview_png)
            }).await.unwrap_or_default();
            let final_height = stitcher.height();
            let _ = ah2.emit("scroll://frame", serde_json::json!({
                "frame": preview_b64,
                "preview": preview_b64,
                "height": final_height,
                "phys_height": (final_height as f64 / scale) as u32,
            }));
        }

        // 写入 DB（不在此处关窗口，等 emit scroll://done 后前端处理完再关）
        let stop_mode = match SCROLL_STOP_MODE.load(std::sync::atomic::Ordering::SeqCst) {
            1 => ScrollStopMode::Save,
            2 => ScrollStopMode::Cancel,
            _ => ScrollStopMode::Copy,
        };
        SCROLL_STOP_MODE.store(0, std::sync::atomic::Ordering::SeqCst);

        if stop_mode == ScrollStopMode::Cancel {
            // 取消：不入库，直接关窗口
            close_all_screenshot_windows(&ah);
            return;
        }

        // 消费 stitcher 一次性 move 出 canvas——避免 canvas().clone() 复制整张画布
        // （P0-2 修复：38MB/次 × 3 次 → 0 次大 clone）。此后 stitcher 不可再用。
        let canvas = stitcher.into_canvas();
        let ah3 = ah.clone();
        let ah4 = ah.clone();

        // 先做 PNG 快速编码（剪贴板和入库都需要的基础数据）。
        // canvas 后续还要 move 给 db_task，此处仅借用——用 DynamicImage::from borrow 编码。
        let png_bytes = {
            let img = image::DynamicImage::ImageRgba8(canvas.clone());
            let mut png = Vec::new();
            let mut cursor = std::io::Cursor::new(&mut png);
            let png_encoder = image::codecs::png::PngEncoder::new_with_quality(
                &mut cursor,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Up,
            );
            let _ = img.write_with_encoder(png_encoder);
            if png.is_empty() {
                log::error!("[scroll] PNG encoding produced empty bytes");
            }
            png
        };
        let hash = octopus_clipboard::image::hash_bytes(&png_bytes);
        let item_id = octopus_clipboard::store::chrono_millis();

        // 线程一：立即写剪贴板（用户最关心，~1s）
        let png_for_clipboard = png_bytes.clone();
        let ah_clipboard = ah4.clone();
        let clipboard_task = tokio::task::spawn_blocking(move || {
            if let Some(handle) = ah_clipboard.try_state::<std::sync::Arc<ClipboardHandle>>() {
                if let Err(e) = handle.write_image(&png_for_clipboard) {
                    log::error!("[scroll] Failed to write clipboard: {}", e);
                }
            }
        });

        // 线程二：WebP 编码 + DB 入库（后台，~2-3s）
        let canvas_for_db = canvas;
        let hash_for_db = hash.clone();
        let id_for_db = item_id;
        let _db_task = tokio::task::spawn_blocking(move || {
            let img = image::DynamicImage::ImageRgba8(canvas_for_db);
            let encoded = match octopus_clipboard::image::encode_image(&img) {
                Ok(e) => e,
                Err(_) => return,
            };
            if let Err(e) = octopus_infra::db::with_db(|conn| {
                octopus_clipboard::store::insert_image_data(conn, &hash_for_db, &encoded.image_blob, &encoded.thumb_blob, img.width() as i64, img.height() as i64)
            }) {
                log::error!("[scroll] Failed to insert image_data: {}", e);
            }
            if let Err(e) = octopus_infra::db::with_db(|conn| {
                octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
                    id: id_for_db, item_type: octopus_clipboard::ItemType::Image,
                    content: String::new(),
                    ref_data: Some(hash_for_db.clone()),
                    meta_info: Some(octopus_clipboard::MetaInfo {
                        w: Some(img.width()), h: Some(img.height()), size: Some(format_file_size(encoded.image_blob.len() as u64)),
                        ..Default::default()
                    }),
                    created_at: octopus_clipboard::store::iso_now(),
                    has_thumbnail: Some(1), is_rich: false,
                })
            }) {
                log::error!("[scroll] Failed to insert clipboard_item: {}", e);
            }
        });

        // 等剪贴板写入完成（~1s），DB 入库在后台继续
        let _ = clipboard_task.await;

        let _ = ah3.emit("scroll://done", serde_json::json!({ "id": item_id }));
        let _ = ah3.emit("clipboard://changed", ());

        // 保存模式：Rust 端直接弹对话框——移入 spawn_blocking 防阻塞 async 线程
        if stop_mode == ScrollStopMode::Save {
            let ah_clone = ah.clone();
            let png_for_save = png_bytes.clone();
            tokio::task::spawn_blocking(move || {
                use tauri_plugin_dialog::DialogExt;
                let save_path = ah_clone.dialog()
                    .file()
                    .add_filter("PNG 图片", &["png"])
                    .set_file_name("scroll-screenshot.png")
                    .blocking_save_file();
                if let Some(path) = save_path {
                    if let Some(p) = path.as_path() {
                        let _ = std::fs::write(p, &png_for_save);
                    }
                }
            })
            .await
            .ok();
        }

        // 窗口已在上方提前关闭
    });

    Ok(())
}

#[tauri::command]
pub fn stop_scroll_recording() {
    SCROLL_RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// 前端设置停止模式（保存/复制/取消），然后停止录制
#[tauri::command]
pub fn stop_scroll_recording_with_mode(mode: String) {
    let m = match mode.as_str() {
        "save" => ScrollStopMode::Save,
        "cancel" => ScrollStopMode::Cancel,
        _ => ScrollStopMode::Copy,
    };
    SCROLL_STOP_MODE.store(m as u8, std::sync::atomic::Ordering::SeqCst);
    SCROLL_RECORDING.store(false, std::sync::atomic::Ordering::SeqCst);
}

