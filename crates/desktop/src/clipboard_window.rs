use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW_LABEL: &str = "clipboard_window";

pub fn create_clipboard_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("剪贴板历史")
    .inner_size(420.0, 600.0)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(true)
    .visible(false)
    .build()?;

    let win_clone = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(false) = event {
            let _ = win_clone.hide();
        }
    });

    Ok(())
}

pub fn toggle_clipboard_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        if window.is_visible().unwrap_or(false) {
            window.hide()?;
        } else {
            window.show()?;
            window.set_focus()?;
        }
    } else {
        create_clipboard_window(app)?;
        // show after creation
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            window.show()?;
            window.set_focus()?;
        }
    }
    Ok(())
}
