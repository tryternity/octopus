//! 剪贴板浮窗 dock（吸附收缩）鼠标穿透控制。
//!
//! 复用 screenshot_commands.rs 已验证的穿透模式：
//! 16ms 轮询 CGEvent 鼠标位置，鼠标在细条区域 → setIgnoresMouseEvents(false)，
//! 透明区域 → setIgnoresMouseEvents(true)。通过 run_on_main_thread 在主线程执行。

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;
    use std::time::Duration;

    static POLL_ACTIVE: AtomicBool = AtomicBool::new(false);
    static DOCK_EDGE: OnceLock<std::sync::Mutex<Option<&'static str>>> = OnceLock::new();

    fn edge_lock() -> &'static std::sync::Mutex<Option<&'static str>> {
        DOCK_EDGE.get_or_init(|| std::sync::Mutex::new(None))
    }

    /// 启动鼠标位置轮询：鼠标在细条区域 → 可交互，否则穿透。
    pub fn start_edge_poll(window: tauri::WebviewWindow, edge: &'static str) {
        *edge_lock().lock().unwrap() = Some(edge);
        POLL_ACTIVE.store(true, Ordering::SeqCst);

        std::thread::spawn(move || {
            let mut cur_passthrough = true; // 初始穿透
            // 初始设为穿透
            set_ignores(&window, true);

            while POLL_ACTIVE.load(Ordering::SeqCst) {
                let mouse = get_mouse_position();
                let win_pos = window.outer_position().unwrap_or_default();
                let scale = window.scale_factor().unwrap_or(1.0);
                let win_x = win_pos.x as f64 / scale;
                let win_y = win_pos.y as f64 / scale;
                let win_right = win_x + 300.0;
                let win_bottom = win_y + 600.0;

                let edge_val = edge_lock().lock().unwrap().clone();
                let in_bar = match edge_val.as_deref() {
                    Some("right") => {
                        mouse.0 >= win_right - 10.0 && mouse.0 <= win_right + 2.0
                            && mouse.1 >= win_y && mouse.1 <= win_bottom
                    }
                    Some("left") => {
                        mouse.0 >= win_x - 2.0 && mouse.0 <= win_x + 10.0
                            && mouse.1 >= win_y && mouse.1 <= win_bottom
                    }
                    _ => false,
                };

                let want_passthrough = !in_bar;
                if want_passthrough != cur_passthrough {
                    set_ignores(&window, want_passthrough);
                    cur_passthrough = want_passthrough;
                }

                std::thread::sleep(Duration::from_millis(16));
            }
        });
    }

    /// 停止轮询 + 恢复正常鼠标事件。
    pub fn stop_edge_poll(window: &tauri::WebviewWindow) {
        POLL_ACTIVE.store(false, Ordering::SeqCst);
        *edge_lock().lock().unwrap() = None;
        set_ignores(window, false);
    }

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

    fn get_mouse_position() -> (f64, f64) {
        use core_graphics::event::CGEvent;
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
        if let Ok(src) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
            if let Ok(evt) = CGEvent::new(src) {
                let loc = evt.location();
                return (loc.x, loc.y);
            }
        }
        (0.0, 0.0)
    }
}

#[cfg(target_os = "macos")]
pub use macos::{start_edge_poll, stop_edge_poll};

#[cfg(not(target_os = "macos"))]
pub fn start_edge_poll(_window: tauri::WebviewWindow, _edge: &'static str) {}
#[cfg(not(target_os = "macos"))]
pub fn stop_edge_poll(_window: &tauri::WebviewWindow) {}
