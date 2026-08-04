//! 剪贴板浮窗 dock（吸附收缩）鼠标穿透控制。
//!
//! 与 result_window::start_click_through_poller 统一模式：
//! - 用 Tauri 跨平台 `cursor_position()`（非 CGEvent）读全局鼠标位置
//! - 物理坐标直接比较（不做 scale 换算，多显示器不同 DPI 安全）
//! - `setIgnoresMouseEvents` via `run_on_main_thread`（macOS）/ `set_ignore_cursor_events`（其他平台）
//! - **双频率状态机**（2026-07-17）：慢检测 200ms（仅 cursor_position 看是否进高频）+
//!   高频 33ms（鼠标在 dock 边缘时跟踪切换），避免窗口 dock 可见但鼠标远离时
//!   持续 30 FPS IPC 导致 idle CPU 高（同 result_window 的根因）。
//!
//! 线程安全：用自增 POLL_ID 防多线程竞态——每次 start 递增 ID，
//! 旧线程检测到 ID 不匹配自动退出。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static POLL_ACTIVE: AtomicBool = AtomicBool::new(false);
static POLL_ID: AtomicU64 = AtomicU64::new(0);

/// 启动鼠标位置轮询：鼠标在细条区域 → 可交互，否则穿透。
/// 用自增 ID 保证同时只有一个轮询线程存活——旧线程自动退出。
pub fn start_edge_poll(_app: tauri::AppHandle, window: tauri::WebviewWindow, edge: &'static str) {
    // 防重复：如果已有活跃 poll 线程，不重复启动（clipboard_dock_collapse + Focused(false) 可能同时调）
    if POLL_ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    POLL_ACTIVE.store(true, Ordering::SeqCst);
    let my_id = POLL_ID.fetch_add(1, Ordering::SeqCst);

    tauri::async_runtime::spawn(async move {
        let mut slow_poll = tokio::time::interval(std::time::Duration::from_millis(100));
        let mut fast_poll = tokio::time::interval(std::time::Duration::from_millis(33));
        let mut cur_ignore = false;
        let mut in_fast_mode = false;

        // 初始设为穿透
        set_ignores(&window, true);

        loop {
            // ID 不匹配 → 旧线程退出（被更新的 start 替代）
            if POLL_ID.load(Ordering::SeqCst) != my_id.wrapping_add(1) {
                break;
            }
            if !POLL_ACTIVE.load(Ordering::SeqCst) {
                break;
            }

            if !in_fast_mode {
                // ── 慢检测：仅查鼠标是否接近 dock 边缘 ──
                slow_poll.tick().await;
                if POLL_ID.load(Ordering::SeqCst) != my_id.wrapping_add(1) { break; }
                let Some((in_bar, _)) = probe_position(&window, edge) else { continue };
                if in_bar {
                    // 鼠标进入边缘 → 升级到高频，实时跟踪切换
                    fast_poll.reset();
                    in_fast_mode = true;
                }
                // 不在边缘：保持穿透，留在慢模式
                continue;
            }

            // ── 高频跟踪：实时切换 setIgnoresMouseEvents ──
            fast_poll.tick().await;
            if POLL_ID.load(Ordering::SeqCst) != my_id.wrapping_add(1) { break; }
            let Some((in_bar, _)) = probe_position(&window, edge) else {
                in_fast_mode = false;
                continue;
            };
            let want = !in_bar;
            if want != cur_ignore {
                set_ignores(&window, want);
                cur_ignore = want;
            }
            // 鼠标已离开边缘 + 当前是穿透态 → 降级回慢模式
            if want && !in_bar {
                in_fast_mode = false;
            }
        }
    });
}

/// 读鼠标位置 + 窗口几何，判定是否在 dock 边缘内。返回 (in_bar, want_ignore)。
///
/// 全部用物理坐标直接比较（cursor_position / outer_position / outer_size 都是 Physical）。
/// 与 result_window::start_click_through_poller 统一模式。
fn probe_position(window: &tauri::WebviewWindow, edge: &'static str) -> Option<(bool, bool)> {
    let cursor = window.cursor_position().ok()?;
    let (mx, my) = (cursor.x, cursor.y);
    let pos = window.outer_position().ok()?;
    let (wx, wy) = (pos.x as f64, pos.y as f64);
    let size = window.outer_size().ok()?;
    let (win_w, win_h) = (size.width as f64, size.height as f64);
    let win_right = wx + win_w;
    let win_bottom = wy + win_h;

    // dock 边缘检测带：窗口边缘内侧 15px + 外侧 3px
    const DETECT_INNER: f64 = 15.0;
    const DETECT_OUTER: f64 = 3.0;

    let in_bar = match edge {
        "right" => {
            mx >= win_right - DETECT_INNER && mx <= win_right + DETECT_OUTER
                && my >= wy && my <= win_bottom
        }
        "left" => {
            mx >= wx - DETECT_OUTER && mx <= wx + DETECT_INNER
                && my >= wy && my <= win_bottom
        }
        _ => false,
    };
    Some((in_bar, !in_bar))
}

/// 停止轮询 + 恢复正常鼠标事件。
pub fn stop_edge_poll(window: &tauri::WebviewWindow) {
    POLL_ACTIVE.store(false, Ordering::SeqCst);
    set_ignores(window, false);
}

#[cfg(target_os = "macos")]
fn set_ignores(window: &tauri::WebviewWindow, ignore: bool) {
    let win = window.clone();
    let _ = window.run_on_main_thread(move || {
        if let Ok(ptr) = win.ns_window() {
            if !ptr.is_null() {
                let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                ns_win.setIgnoresMouseEvents(ignore);
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn set_ignores(window: &tauri::WebviewWindow, ignore: bool) {
    let _ = window.set_ignore_cursor_events(ignore);
}
