use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

const WINDOW_LABEL: &str = "clipboard_window";

/// docked 模式下浮窗是否处于展开态（Rust 侧真相源，前端同步镜像）。
/// 解决 macOS `is_focused()` 在收缩态不可靠的问题——用显式状态替代焦点推断。
static DOCK_EXPANDED: AtomicBool = AtomicBool::new(false);

/// Moved 事件 save_current_position 节流：上次写盘的秒数（-1=从未写）。
/// 防拖拽时每像素都同步写 SQLite。同一秒内只写一次。
static LAST_SAVE_SEC: AtomicI64 = AtomicI64::new(-1);

/// 将动态 String edge 转为 &'static str（仅 "left"/"right"，无需堆分配）。
fn edge_static(s: &str) -> &'static str {
    match s {
        "left" => "left",
        "right" => "right",
        _ => "right", // fallback
    }
}

pub fn create_clipboard_window(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("剪贴板历史")
    .inner_size(300.0, 600.0)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(true)
    .transparent(true)
    .shadow(false)
    .visible(false)
    .build()?;

    // 恢复上次位置（不可见时 fallback 到屏幕居中）
    crate::window_position::restore_window_position(&window, WINDOW_LABEL, |w| {
        if let Ok(Some(m)) = w.primary_monitor() {
            let x = (m.size().width as f64 / m.scale_factor() - 300.0) / 2.0;
            let y = (m.size().height as f64 / m.scale_factor() - 600.0) / 2.0;
            let _ = w.set_position(tauri::Position::Logical(
                tauri::LogicalPosition::new(x, y),
            ));
        }
    });

    // 恢复 dock 状态：如果上次 docked，以 collapsed 态打开（仅 macOS）
    #[cfg(target_os = "macos")]
    {
    let dock_edge = crate::window_position::load_dock_state(WINDOW_LABEL);
    if let Some(ref edge) = dock_edge {
        if edge == "right" || edge == "left" {
            let (_db_x, db_y) = crate::window_position::load_window_position(WINDOW_LABEL)
                .unwrap_or((0.0, 0.0));
            if let Ok(Some(monitor)) = window.current_monitor().or(window.primary_monitor()) {
                let scale = monitor.scale_factor();
                let x = if edge == "right" {
                    monitor.position().x as f64 / scale
                        + monitor.size().width as f64 / scale
                        - 300.0
                } else {
                    monitor.position().x as f64 / scale
                };
                let _ = window.set_position(tauri::Position::Logical(
                    tauri::LogicalPosition::new(x, db_y),
                ));
            }
            let _ = app.emit("clipboard://dock-changed", edge.as_str());
            DOCK_EXPANDED.store(false, Ordering::SeqCst);
            crate::clipboard_dock::start_edge_poll(app.clone(), window.clone(), edge_static(edge));
        }
    }
    }

    // 移动结束后保存位置 + 失焦时恢复被隐藏的 Regular 窗口
    let win_clone = window.clone();
    let app_clone = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Moved(_) => {
            // 节流：同一秒内只写一次 DB（防拖拽时每像素同步写 SQLite）
            let now_sec = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if now_sec != LAST_SAVE_SEC.load(Ordering::Relaxed) {
                LAST_SAVE_SEC.store(now_sec, Ordering::Relaxed);
                crate::window_position::save_current_position(&win_clone, WINDOW_LABEL);
            }

            // 吸附检测仅 macOS（依赖 NSWindow setIgnoresMouseEvents）
            #[cfg(target_os = "macos")]
            {
            // 检测新吸附
            if let Some(edge) = detect_dock_edge(&win_clone) {
                let prev_dock = crate::window_position::load_dock_state(WINDOW_LABEL);
                let is_already_collapsed = prev_dock.as_deref() == Some(edge)
                    && !DOCK_EXPANDED.load(Ordering::SeqCst);
                // 已处于某 edge 的收缩态时，不切换到另一个 edge
                //（防多显示器边界上下拖拽时 right↔left 横跳）
                let is_docked_on_other_edge = prev_dock.as_deref().is_some_and(|p| p != edge && p != "none")
                    && !DOCK_EXPANDED.load(Ordering::SeqCst);
                if !is_already_collapsed && !is_docked_on_other_edge {
                    crate::window_position::save_dock_state(WINDOW_LABEL, edge);
                    DOCK_EXPANDED.store(false, Ordering::SeqCst);
                    crate::clipboard_dock::start_edge_poll(app_clone.clone(), win_clone.clone(), edge);
                    let _ = app_clone.emit("clipboard://dock-changed", edge);
                    log::info!("clipboard docked to {}", edge);
                }
                return;
            }

            // 检测解吸附：之前 docked 但现在不在边缘
            let prev_dock = crate::window_position::load_dock_state(WINDOW_LABEL);
            if let Some(ref prev) = prev_dock {
                if prev == "right" || prev == "left" {
                    crate::window_position::save_dock_state(WINDOW_LABEL, "none");
                    DOCK_EXPANDED.store(false, Ordering::SeqCst);
                    crate::clipboard_dock::stop_edge_poll(&win_clone);
                    let _ = app_clone.emit("clipboard://dock-changed", "none");
                    log::info!("clipboard undocked");
                }
            }
            } // cfg macos
        }
        tauri::WindowEvent::Focused(false) => {
            // 剪贴板失焦（用户切到其他 app）——恢复被隐藏的 Regular 窗口
            #[cfg(target_os = "macos")]
            { crate::activation::restore_hidden_windows_only(&app_clone); }

            // docked 态下失焦 → 收缩（防重复：DOCK_EXPANDED 已 false 则跳过）
            #[cfg(target_os = "macos")]
            if DOCK_EXPANDED.load(Ordering::SeqCst) {
                let docked = crate::window_position::load_dock_state(WINDOW_LABEL);
                if let Some(ref edge) = docked {
                    if edge == "right" || edge == "left" {
                        DOCK_EXPANDED.store(false, Ordering::SeqCst);
                        let _ = app_clone.emit("clipboard://collapse", ());
                        // 窗口隐藏时不启动轮询（防 CPU 空转）
                        if win_clone.is_visible().unwrap_or(false) {
                            crate::clipboard_dock::start_edge_poll(app_clone.clone(), win_clone.clone(), edge_static(edge));
                        }
                    }
                }
            }
        }
        _ => {}
    });

    Ok(())
}

/// 检测窗口是否应吸附到屏幕边缘。
/// 返回 Some("right") / Some("left") / None。
fn detect_dock_edge(window: &tauri::WebviewWindow) -> Option<&'static str> {
    const DOCK_THRESHOLD: f64 = 10.0;
    const WIN_W: f64 = 300.0;
    const WIN_H: f64 = 600.0;

    let pos = window.outer_position().ok()?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let win_x = pos.x as f64 / scale;
    let win_y = pos.y as f64 / scale;
    let center_x = win_x + WIN_W / 2.0;
    let center_y = win_y + WIN_H / 2.0;

    let monitors = window.available_monitors().unwrap_or_default();
    let current = monitors.iter().find(|m| {
        let ms = m.scale_factor();
        let mx = m.position().x as f64 / ms;
        let my = m.position().y as f64 / ms;
        let mw = m.size().width as f64 / ms;
        let mh = m.size().height as f64 / ms;
        center_x >= mx && center_x <= mx + mw && center_y >= my && center_y <= my + mh
    })?;

    let ms = current.scale_factor();
    let mon_right = current.position().x as f64 / ms + current.size().width as f64 / ms;
    let mon_left = current.position().x as f64 / ms;

    let dist_right = (mon_right - (win_x + WIN_W)).abs();
    let dist_left = (win_x - mon_left).abs();

    if dist_right <= DOCK_THRESHOLD && dist_right <= dist_left {
        Some("right")
    } else if dist_left <= DOCK_THRESHOLD {
        Some("left")
    } else {
        None
    }
}

/// Tauri 命令：展开 dock 浮窗（前端 DockBar onMouseEnter 调用）。
#[tauri::command]
pub fn clipboard_dock_expand(app: AppHandle) {
    DOCK_EXPANDED.store(true, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        crate::clipboard_dock::stop_edge_poll(&window);
    }
    let _ = app.emit("clipboard://expand", ());
}

/// Tauri 命令：收缩 dock 浮窗。
#[tauri::command]
pub fn clipboard_dock_collapse(app: AppHandle) {
    DOCK_EXPANDED.store(false, Ordering::SeqCst);
    let docked = crate::window_position::load_dock_state(WINDOW_LABEL);
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        if let Some(edge) = docked.filter(|e| e == "right" || e == "left") {
            crate::clipboard_dock::start_edge_poll(app.clone(), window, edge_static(&edge));
        }
    }
    let _ = app.emit("clipboard://collapse", ());
}

/// 注册剪贴板浮窗全局快捷键。main 启动注册 + set_config 热重载共用，
/// 与 shortcut::register_shortcut / result_window::register_edit_global_shortcut 范式一致：
/// 解析 + on_shortcut，失败返回 Err（供调用方回滚旧快捷键）。
pub fn register_clipboard_shortcut(
    app: &tauri::AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                let _ = toggle_clipboard_window(&app_handle);
            }
        })
        .map_err(|e| format!("Failed to register clipboard shortcut '{}': {}", shortcut_str, e))?;
    Ok(())
}

pub fn toggle_clipboard_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let visible = window.is_visible().unwrap_or(false);
        let focused = window.is_focused().unwrap_or(false);

        // dock 状态：处于 docked 模式时，快捷键逻辑不同于普通 toggle
        let docked = crate::window_position::load_dock_state(WINDOW_LABEL)
            .filter(|e| e == "right" || e == "left");

        if let Some(_edge) = &docked {
            // docked 模式：用 DOCK_EXPANDED 原子状态判断（不依赖 is_focused）
            let expanded = DOCK_EXPANDED.load(Ordering::SeqCst);
            if expanded {
                // 展开 → 收缩
                DOCK_EXPANDED.store(false, Ordering::SeqCst);
                let docked_edge = crate::window_position::load_dock_state(WINDOW_LABEL);
                if let Some(e) = docked_edge.filter(|e| e == "right" || e == "left") {
                    crate::clipboard_dock::start_edge_poll(app.clone(), window.clone(), edge_static(&e));
                }
                let _ = app.emit("clipboard://collapse", ());
            } else {
                // 收缩 → 展开 + 获焦
                DOCK_EXPANDED.store(true, Ordering::SeqCst);
                crate::clipboard_dock::stop_edge_poll(&window);
                #[cfg(target_os = "macos")]
                { crate::activation::before_floating_window_show(app); }
                window.show()?;
                let _ = app.emit("clipboard://expand", ());
                window.set_focus()?;
            }
        } else if visible && focused {
            // 非 docked：可见且有焦点 → 隐藏
            window.hide()?;
            #[cfg(target_os = "macos")]
            { crate::activation::after_floating_window_hide(app); }
        } else {
            // 非 docked：不可见或无焦点 → show + focus
            #[cfg(target_os = "macos")]
            { crate::activation::before_floating_window_show(app); }
            window.show()?;
            window.set_focus()?;
        }
    } else {
        create_clipboard_window(app)?;
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            #[cfg(target_os = "macos")]
            { crate::activation::before_floating_window_show(app); }
            window.show()?;
            window.set_focus()?;
        }
    }
    Ok(())
}
