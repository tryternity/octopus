//! 剪贴板浮窗 dock（吸附收缩）NSWindow 操作。
//!
//! 核心矛盾：setIgnoresMouseEvents(true) 让透明区域穿透，但细条也收不到事件。
//! 解法：定时轮询鼠标位置，在细条区域内时临时关闭 ignore，离开时开回来。

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

    /// 启动鼠标位置轮询：鼠标在细条区域 → ignore=false，否则 ignore=true。
    pub fn start_edge_poll(window: tauri::WebviewWindow, edge: &'static str) {
        *edge_lock().lock().unwrap() = Some(edge);
        POLL_ACTIVE.store(true, Ordering::SeqCst);

        std::thread::spawn(move || {
            let mut prev_ignore = true;
            while POLL_ACTIVE.load(Ordering::SeqCst) {
                let mouse = get_mouse_position();
                let win_pos = window.outer_position().unwrap_or_default();
                let scale = window.scale_factor().unwrap_or(1.0);
                let win_x = win_pos.x as f64 / scale;
                let win_y = win_pos.y as f64 / scale;
                let win_right = win_x + 300.0;

                let edge_val = edge_lock().lock().unwrap().clone();
                let should_ignore = match edge_val {
                    Some("right") => {
                        // 细条在窗口右侧 8px：mouse_x 在 [win_right-8, win_right] 内 → 不穿透
                        !(mouse.0 >= win_right - 10.0 && mouse.0 <= win_right + 2.0
                            && mouse.1 >= win_y && mouse.1 <= win_y + 600.0)
                    }
                    Some("left") => {
                        // 细条在窗口左侧 8px
                        !(mouse.0 >= win_x - 2.0 && mouse.0 <= win_x + 10.0
                            && mouse.1 >= win_y && mouse.1 <= win_y + 600.0)
                    }
                    _ => false,
                };

                if should_ignore != prev_ignore {
                    let w = window.clone();
                    let _ = window.run_on_main_thread(move || {
                        if let Ok(ptr) = w.ns_window() {
                            if !ptr.is_null() {
                                let ns_win =
                                    unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                                ns_win.setIgnoresMouseEvents(should_ignore);
                            }
                        }
                    });
                    prev_ignore = should_ignore;
                }

                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }

    /// 停止轮询 + 恢复正常鼠标事件。
    pub fn stop_edge_poll(window: &tauri::WebviewWindow) {
        POLL_ACTIVE.store(false, Ordering::SeqCst);
        *edge_lock().lock().unwrap() = None;
        let w = window.clone();
        let _ = window.run_on_main_thread(move || {
            if let Ok(ptr) = w.ns_window() {
                if !ptr.is_null() {
                    let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                    ns_win.setIgnoresMouseEvents(false);
                }
            }
        });
    }

    fn get_mouse_position() -> (f64, f64) {
        use core_graphics::event::CGEvent;
        use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap();
        let event = CGEvent::new(source).unwrap();
        let point = event.location();
        (point.x, point.y)
    }
}

#[cfg(target_os = "macos")]
pub use macos::{start_edge_poll, stop_edge_poll};

#[cfg(not(target_os = "macos"))]
pub fn start_edge_poll(_window: tauri::WebviewWindow, _edge: &'static str) {}
#[cfg(not(target_os = "macos"))]
pub fn stop_edge_poll(_window: &tauri::WebviewWindow) {}
