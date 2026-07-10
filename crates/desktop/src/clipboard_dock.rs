//! 剪贴板浮窗 dock（吸附收缩）鼠标穿透控制。
//!
//! 与 result_window::start_click_through_poller 统一模式：
//! - 用 Tauri 跨平台 `cursor_position()`（非 CGEvent）读全局鼠标位置
//! - 物理坐标直接比较（不做 scale 换算，多显示器不同 DPI 安全）
//! - `setIgnoreCursorEvents` via `run_on_main_thread`（macOS）/ `set_ignore_cursor_events`（其他平台）
//! - tokio interval 33ms（非 std::thread::sleep 16ms）
//!
//! Wayland 限制：cursor_position() 返回 (0,0)，穿透失效（协议层限制，无解）。
//! 非 macOS 平台用 Tauri set_ignore_cursor_events 替代 NSWindow setIgnoresMouseEvents。

use std::sync::atomic::{AtomicBool, Ordering};

static POLL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 启动鼠标位置轮询：鼠标在细条区域 → 可交互，否则穿透。
/// 需传入 AppHandle（tokio spawn 需要），edge 从 DB 读取。
pub fn start_edge_poll(_app: tauri::AppHandle, window: tauri::WebviewWindow, edge: &'static str) {
    POLL_ACTIVE.store(true, Ordering::SeqCst);

    tauri::async_runtime::spawn(async move {
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(33));
        let mut cur_ignore = false;

        // 初始设为穿透
        set_ignores(&window, true);

        while POLL_ACTIVE.load(Ordering::SeqCst) {
            poll.tick().await;

            // 用 Tauri cursor_position() + outer_position()——都是物理坐标，直接比较
            let (mx, my) = match window.cursor_position() {
                Ok(p) => (p.x, p.y),
                Err(_) => continue,
            };
            let (wx, wy) = match window.outer_position() {
                Ok(p) => (p.x as f64, p.y as f64),
                Err(_) => continue,
            };
            let sf = window.scale_factor().unwrap_or(1.0);
            let win_w = 300.0 * sf;
            let win_h = 600.0 * sf;
            let win_right = wx + win_w;
            let win_bottom = wy + win_h;

            // 细条区域（物理坐标）：边缘 10px 容差
            let in_bar = match edge {
                "right" => {
                    mx >= win_right - 10.0 * sf && mx <= win_right + 2.0 * sf
                        && my >= wy && my <= win_bottom
                }
                "left" => {
                    mx >= wx - 2.0 * sf && mx <= wx + 10.0 * sf
                        && my >= wy && my <= win_bottom
                }
                _ => false,
            };

            let want = !in_bar;
            if want != cur_ignore {
                set_ignores(&window, want);
                cur_ignore = want;
            }
        }
    });
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
