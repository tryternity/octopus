use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW_LABEL: &str = "clipboard_window";
const BLUR_GRACE_MS: i64 = 300;

static LAST_SHOWN_MS: AtomicI64 = AtomicI64::new(0);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn create_clipboard_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        LAST_SHOWN_MS.store(now_ms(), Ordering::Relaxed);
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
            // Grace period：show 后 300ms 内忽略 blur（窗口刚弹出时焦点抖动）
            let elapsed = now_ms() - LAST_SHOWN_MS.load(Ordering::Relaxed);
            if elapsed > BLUR_GRACE_MS {
                let _ = win_clone.hide();
            }
        }
    });

    Ok(())
}

pub fn toggle_clipboard_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        if window.is_visible().unwrap_or(false) {
            window.hide()?;
        } else {
            LAST_SHOWN_MS.store(now_ms(), Ordering::Relaxed);
            window.show()?;
            window.set_focus()?;
        }
    } else {
        create_clipboard_window(app)?;
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            LAST_SHOWN_MS.store(now_ms(), Ordering::Relaxed);
            window.show()?;
            window.set_focus()?;
        }
    }
    Ok(())
}
