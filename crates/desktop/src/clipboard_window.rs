use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW_LABEL: &str = "clipboard_window";

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

    // 移动结束后保存位置 + 失焦时恢复被隐藏的 Regular 窗口
    let win_clone = window.clone();
    let app_clone = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Moved(_) => {
            crate::window_position::save_current_position(&win_clone, WINDOW_LABEL);

            if let Some(edge) = detect_dock_edge(&win_clone) {
                crate::window_position::save_dock_state(WINDOW_LABEL, edge);
                let _ = app_clone.emit("clipboard://dock-changed", edge);
                log::info!("clipboard docked to {}", edge);
            }
        }
        tauri::WindowEvent::Focused(false) => {
            // 剪贴板失焦（用户切到其他 app）——恢复被隐藏的 Regular 窗口
            #[cfg(target_os = "macos")]
            { crate::activation::restore_hidden_windows_only(&app_clone); }
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
        if visible && focused {
            window.hide()?;
            #[cfg(target_os = "macos")]
            { crate::activation::after_floating_window_hide(app); }
        } else {
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
