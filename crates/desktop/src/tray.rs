// src/tray.rs

use crate::config::AppConfig;
use log::info;
use std::sync::Mutex;
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

/// Create the system tray icon and its context menu.
///
/// 菜单文案设计：操作项统一四字宽度，状态项带前缀符号（▶/■/⏳）。
/// 分组：语音识别 → 引擎信息（只读分隔线）→ 截图/剪贴板 → 设置/退出。
pub fn create_tray(app: &tauri::AppHandle, config: &AppConfig) {
    let toggle = MenuItem::with_id(app, "toggle", "语音识别", true, None::<&str>)
        .expect("failed to create toggle menu item");
    let engine_info = MenuItem::with_id(
        app,
        "engine_info",
        format!("引擎  {} · {}", config.asr_engine, config.engine_mode),
        false,
        None::<&str>,
    )
    .expect("failed to create engine_info menu item");

    // 分隔线：引擎信息 vs 功能区
    let sep1 = PredefinedMenuItem::separator(app)
        .expect("failed to create separator");

    let screenshot = MenuItem::with_id(app, "screenshot", "开始截图", true, None::<&str>)
        .expect("failed to create screenshot menu item");
    let clipboard = MenuItem::with_id(app, "clipboard", "剪  贴  板", true, None::<&str>)
        .expect("failed to create clipboard menu item");

    // 分隔线：功能区 vs 设置/退出
    let sep2 = PredefinedMenuItem::separator(app)
        .expect("failed to create separator2");

    let settings = MenuItem::with_id(app, "settings", "系统管理", true, None::<&str>)
        .expect("failed to create settings menu item");
    let notepad = MenuItem::with_id(app, "notepad", "记\u{3000}事\u{3000}本", true, None::<&str>)
        .expect("failed to create notepad menu item");
    let quit = MenuItem::with_id(app, "quit", "退出系统", true, None::<&str>)
        .expect("failed to create quit menu item");

    let menu = Menu::with_items(app, &[
        &toggle, &engine_info, &sep1,
        &screenshot, &clipboard, &notepad, &sep2,
        &settings, &quit,
    ])
    .expect("failed to create tray menu");

    // 存储 handle 供后续更新使用
    {
        let mut items = TRAY_ITEMS.lock().unwrap();
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
            "notepad" => {
                info!("Tray: open notepad");
                crate::notepad_window::open_notepad(app.clone());
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
        .expect("failed to build tray icon");
}

/// Update the toggle menu item label based on the current state.
pub fn update_tray_label(_app: &tauri::AppHandle, state: TrayState) {
    let label = match state {
        TrayState::Idle => "语音识别",
        TrayState::Recording => "停止识别",
        TrayState::Processing => "处理中…",
    };

    let items = TRAY_ITEMS.lock().unwrap();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.toggle.set_text(label);
    }
}

/// Update the engine info menu item label dynamically.
pub fn update_tray_engine_label(_app: &tauri::AppHandle, engine_name: &str, engine_mode: &str) {
    let label = format!("引擎  {} · {}", engine_name, engine_mode);
    let items = TRAY_ITEMS.lock().unwrap();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.engine_info.set_text(label);
    }
}

/// Update the screenshot menu item: 正常 ↔ 灰掉
pub fn update_tray_screenshot_label(active: bool) {
    let items = TRAY_ITEMS.lock().unwrap();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.screenshot.set_enabled(!active);
    }
}
