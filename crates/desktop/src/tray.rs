// src/tray.rs

use crate::config::AppConfig;
use log::info;
use parking_lot::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, Runtime};

/// Tray icon state for display purposes
pub enum TrayState {
    Idle,
    Recording,
    Processing,
}

/// 存储需要动态更新的 MenuItem handle
struct TrayItems<R: Runtime> {
    toggle: MenuItem<R>,
    engine_info: MenuItem<R>,
    screenshot: MenuItem<R>,
}

/// 模块级存储，避免 MenuItem::with_id 重复 ID 导致的 panic
static TRAY_ITEMS: once_cell::sync::Lazy<Mutex<Option<TrayItems<tauri::Wry>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

/// 存储 asr_shortcut 用于 update_tray_label 动态文案
static ASR_SHORTCUT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// 将 Tauri Accelerator 格式（CmdOrCtrl+Shift+A）转为用户可读格式（⌘⇧A）
fn fmt_shortcut(s: &str) -> String {
    if s.is_empty() { return String::new(); }
    s.replace("CmdOrCtrl+", "⌘").replace("Cmd+", "⌘")
     .replace("Shift+", "⇧").replace("Alt+", "⌥")
     .replace("Control+", "⌃").replace("Super+", "⌘")
}

/// Create the system tray icon and its context menu.
///
/// 菜单文案设计：操作项统一四字宽度 + 括号快捷键。
/// 分组：语音识别 → 引擎信息（只读分隔线）→ 截图/剪贴板 → 设置/退出。
pub fn create_tray(app: &tauri::AppHandle, config: &AppConfig) -> Result<(), String> {
    let _ = ASR_SHORTCUT.set(config.asr_shortcut.clone());
    let toggle_text = format!("语音识别（{}）", fmt_shortcut(&config.asr_shortcut));
    let toggle = MenuItem::with_id(app, "toggle", &toggle_text, true, None::<&str>)
        .map_err(|e| format!("toggle menu: {e}"))?;
    let engine_info = MenuItem::with_id(
        app,
        "engine_info",
        format!("引擎  {} · {}", config.asr_engine, config.engine_mode),
        false,
        None::<&str>,
    )
    .map_err(|e| format!("engine_info menu: {e}"))?;

    let sep1 = PredefinedMenuItem::separator(app)
        .map_err(|e| format!("separator: {e}"))?;

    let screenshot_text = format!("开始截图（{}）", fmt_shortcut(&config.screenshot_shortcut));
    let screenshot = MenuItem::with_id(app, "screenshot", &screenshot_text, true, None::<&str>)
        .map_err(|e| format!("screenshot menu: {e}"))?;
    let clipboard_text = format!("剪  贴  板（{}）", fmt_shortcut(&config.clipboard_shortcut));
    let clipboard = MenuItem::with_id(app, "clipboard", &clipboard_text, true, None::<&str>)
        .map_err(|e| format!("clipboard menu: {e}"))?;
    // 图文编辑：打开空白 CompactEditor（临时文本 tab，不写 DB）。
    let compact_editor = MenuItem::with_id(app, "compact_editor", "图文编辑", true, None::<&str>)
        .map_err(|e| format!("compact_editor menu: {e}"))?;

    let sep2 = PredefinedMenuItem::separator(app)
        .map_err(|e| format!("separator2: {e}"))?;

    let settings = MenuItem::with_id(app, "settings", "系统管理", true, None::<&str>)
        .map_err(|e| format!("settings menu: {e}"))?;
    let quit = MenuItem::with_id(app, "quit", "退出系统", true, None::<&str>)
        .map_err(|e| format!("quit menu: {e}"))?;

    let menu = Menu::with_items(app, &[
        &toggle, &engine_info, &sep1,
        &screenshot, &clipboard, &compact_editor, &sep2,
        &settings, &quit,
    ])
    .map_err(|e| format!("tray menu: {e}"))?;

    // 存储 handle 供后续更新使用
    {
        let mut items = TRAY_ITEMS.lock();
        *items = Some(TrayItems {
            toggle: toggle.clone(),
            engine_info: engine_info.clone(),
            screenshot: screenshot.clone(),
        });
    }

    let _tray = TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .cloned()
                .unwrap_or_else(|| Image::from_bytes(include_bytes!("../icons/icon.png")).unwrap()),
        )
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("octopus")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => {
                info!("Tray: toggle recording");
                if let Some(coordinator) = app.try_state::<crate::coordinator::Coordinator>() {
                    coordinator.toggle();
                }
            }
            "clipboard" => {
                info!("Tray: toggle clipboard");
                let _ = crate::clipboard_window::toggle_clipboard_window(app);
            }
            "compact_editor" => {
                info!("Tray: open compact editor (empty)");
                crate::compact_editor_commands::open_temp_compact_editor(app, "");
            }
            "settings" => {
                info!("Tray: open settings");
                crate::settings_window::open_settings(app.clone(), None);
            }
            "screenshot" => {
                info!("Tray: start screenshot");
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::screenshot_commands::start_screenshot(app_handle).await;
                });
            }
            "quit" => {
                info!("Tray: quit");
                app.exit(0);
            }
            _ => {}
        })
        .build(app)
        .map_err(|e| format!("tray icon build: {e}"))?;
    Ok(())
}

/// Update the toggle menu item label based on the current state.
pub fn update_tray_label(_app: &tauri::AppHandle, state: TrayState) {
    let sc = ASR_SHORTCUT.get().map(|s| fmt_shortcut(s)).unwrap_or_default();
    let label = match state {
        TrayState::Idle => format!("语音识别（{}）", sc),
        TrayState::Recording => "停止识别".to_string(),
        TrayState::Processing => "处理中…".to_string(),
    };

    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.toggle.set_text(label);
    }
}

/// Update the engine info menu item label dynamically.
pub fn update_tray_engine_label(_app: &tauri::AppHandle, engine_name: &str, engine_mode: &str) {
    let label = format!("引擎  {} · {}", engine_name, engine_mode);
    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.engine_info.set_text(label);
    }
}

/// Update the screenshot menu item: 正常 ↔ 灰掉
pub fn update_tray_screenshot_label(active: bool) {
    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.screenshot.set_enabled(!active);
    }
}
