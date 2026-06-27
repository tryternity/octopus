use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

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

    // 移动结束后保存位置
    let win_clone = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Moved(_) = event {
            crate::window_position::save_current_position(&win_clone, WINDOW_LABEL);
        }
    });

    Ok(())
}

pub fn toggle_clipboard_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        // toggle 方向按"焦点"而非"可见性"判断：剪贴板窗口是 always-on-top，
        // 失焦后仍 visible，若用 is_visible() 决定方向，失焦时按一次会被当成
        // "已显示→收起"先藏掉，第二次按键才弹出。改为仅当"可见且有焦点"才收起；
        // 失焦（或不可见）一律 show + set_focus 激活——即"失焦状态按快捷键直接弹出"。
        let visible = window.is_visible().unwrap_or(false);
        let focused = window.is_focused().unwrap_or(false);
        if visible && focused {
            window.hide()?;
        } else {
            window.show()?;
            window.set_focus()?;
        }
    } else {
        create_clipboard_window(app)?;
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            window.show()?;
            window.set_focus()?;
        }
    }
    Ok(())
}
