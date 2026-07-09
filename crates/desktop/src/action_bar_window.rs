//! AI 命令面板迷你浮窗——选中文本后热键触发，鼠标上方弹出。
//! 透明无边框 always_on_top，单例 show/hide toggle。

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use parking_lot::Mutex;
use once_cell::sync::Lazy;

pub const WINDOW_LABEL: &str = "action_bar_window";

static TEMPORARILY_HIDDEN_WINDOWS: Lazy<Mutex<Vec<String>>> = Lazy::new(|| Mutex::new(Vec::new()));

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

        // macOS 平台下的常规窗口临时隐藏逻辑，避免全局激活将背景窗口带到最前覆盖其他应用
        #[cfg(target_os = "macos")]
        {
            use objc2::MainThreadMarker;
            use objc2_app_kit::NSApplication;
            if let Some(mtm) = MainThreadMarker::new() {
                let app_ns = NSApplication::sharedApplication(mtm);
                // 只有当我们的 app 当前处于非活动状态（在后台）时，才临时隐藏，防止它们被强行带到前台
                if !app_ns.isActive() {
                    let mut hidden = Vec::new();
                    for label in &["settings_window", "compact_editor_window"] {
                        if let Some(w) = app.get_webview_window(label) {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                                hidden.push(label.to_string());
                            }
                        }
                    }
                    *TEMPORARILY_HIDDEN_WINDOWS.lock() = hidden;
                }
            }
        }

        let _ = win.show();
        let _ = win.set_focus(); // 调用标准 Tauri 接口获取键盘焦点，激活 App 进程
        let _ = app.emit("action-bar://show", ());
    }
}

/// 隐藏浮窗。
pub fn hide_action_bar_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.hide();
    }

    // 恢复临时隐藏的常规窗口
    let hidden = {
        let mut guard = TEMPORARILY_HIDDEN_WINDOWS.lock();
        std::mem::take(&mut *guard)
    };

    if !hidden.is_empty() {
        // macOS 下，先将我们的 app 进程 deactivate（退回后台），
        // 这样重新 show() 它们时，它们会显示但依然保持在后台，不覆盖当前用户活动的应用。
        #[cfg(target_os = "macos")]
        {
            use objc2::MainThreadMarker;
            use objc2_app_kit::NSApplication;
            if let Some(mtm) = MainThreadMarker::new() {
                let app_ns = NSApplication::sharedApplication(mtm);
                app_ns.deactivate();
            }
        }

        for label in hidden {
            if let Some(w) = app.get_webview_window(&label) {
                let _ = w.show();
            }
        }
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
