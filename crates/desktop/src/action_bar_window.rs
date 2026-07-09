//! AI 命令面板迷你浮窗——选中文本后热键触发，鼠标上方弹出。
//! 透明无边框 always_on_top，单例 show/hide toggle。

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const WINDOW_LABEL: &str = "action_bar_window";

/// 创建窗口（应用启动时调用，visible=false）。
pub fn create_action_bar_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("")
    .inner_size(380.0, 76.0) // 每行 ~36px × 2 + padding + 2px 余量
    .decorations(false)
    .always_on_top(true)
    .transparent(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(false)
    .build();
}

/// 在指定坐标显示浮窗（鼠标上方）。emit 事件让前端刷新 context。
pub fn show_action_bar_window(app: &AppHandle, x: f64, y: f64) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(x, y),
        ));
        // show 但不 set_focus——避免激活 app 导致 Regular 窗口
        // （settings/compact_editor）被带到前台。与语音识别快捷键一致。
        let _ = win.show();
        let _ = app.emit("action-bar://show", ());
    }
}

/// 隐藏浮窗。
pub fn hide_action_bar_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.hide();
    }
}

/// 注册全局热键。与 register_clipboard_shortcut 范式一致。
pub fn register_action_bar_shortcut(
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
                crate::action_bar_commands::trigger_action_bar(app_handle.clone());
            }
        })
        .map_err(|e| format!("Failed to register action bar shortcut '{}': {}", shortcut_str, e))?;
    Ok(())
}
