// src/tray.rs

use crate::config::AppConfig;
use log::info;
use std::sync::Mutex;
use tauri::{Emitter, image::Image};
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
/// 分组：语音识别 → 引擎信息（只读分隔线）→ 截图/剪贴板/记事本 → 设置/退出。
pub fn create_tray(app: &tauri::AppHandle, config: &AppConfig) {
    let _ = ASR_SHORTCUT.set(config.asr_shortcut.clone());
    let toggle_text = format!("语音识别（{}）", fmt_shortcut(&config.asr_shortcut));
    let toggle = MenuItem::with_id(app, "toggle", &toggle_text, true, None::<&str>)
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

    let screenshot_text = format!("开始截图（{}）", fmt_shortcut(&config.screenshot_shortcut));
    let screenshot = MenuItem::with_id(app, "screenshot", &screenshot_text, true, None::<&str>)
        .expect("failed to create screenshot menu item");
    let clipboard_text = format!("剪  贴  板（{}）", fmt_shortcut(&config.clipboard_shortcut));
    let clipboard = MenuItem::with_id(app, "clipboard", &clipboard_text, true, None::<&str>)
        .expect("failed to create clipboard menu item");
    let notepad = MenuItem::with_id(app, "notepad", "记  事  本", true, None::<&str>)
        .expect("failed to create notepad menu item");

    // 分隔线：功能区 vs 设置/退出
    let sep2 = PredefinedMenuItem::separator(app)
        .expect("failed to create separator2");

    let settings = MenuItem::with_id(app, "settings", "系统管理", true, None::<&str>)
        .expect("failed to create settings menu item");
    let quit = MenuItem::with_id(app, "quit", "退出系统", true, None::<&str>)
        .expect("failed to create quit menu item");

    let scroll_capture = MenuItem::with_id(app, "scroll_capture", "滚动截屏", true, None::<&str>)
        .expect("failed to create scroll_capture menu item");

    let menu = Menu::with_items(app, &[
        &toggle, &engine_info, &sep1,
        &screenshot, &scroll_capture, &clipboard, &notepad, &sep2,
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
            "scroll_capture" => {
                if scroll_capture::is_recording_active() {
                    info!("Tray: stop scroll capture");
                    scroll_capture::stop();
                } else {
                    info!("Tray: start scroll capture");
                    let app_handle2 = app.clone();
                    let app_handle3 = app.clone();
                    let _ = app_handle2.run_on_main_thread(move || {
                    scroll_capture::start(Box::new(move |png_bytes| {
                        let hash = octopus_clipboard::image::sha256_hex(&png_bytes);
                        let img = match image::load_from_memory(&png_bytes) { Ok(i) => i, Err(_) => return };
                        let encoded = match octopus_clipboard::image::encode_to_webp(&img) { Ok(e) => e, Err(_) => return };
                        let item_id = octopus_clipboard::store::chrono_millis();
                        let _ = octopus_infra::db::with_db(|conn| {
                            octopus_clipboard::store::insert_image_data(conn, &hash, &encoded.webp_blob, &encoded.thumb_blob, img.width() as i64, img.height() as i64)
                        });
                        let _ = octopus_infra::db::with_db(|conn| {
                            octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
                                id: item_id, item_type: octopus_clipboard::ItemType::Image,
                                content: hash.clone(), search_text: String::new(),
                                created_at: octopus_clipboard::store::iso_now(),
                                blob_hash: Some(hash), width: Some(img.width() as i64),
                                height: Some(img.height() as i64), has_thumbnail: Some(1),
                                file_count: None, is_rich: false,
                            })
                        });
                        if let Some(handle) = app_handle3.try_state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>() {
                            let _ = handle.write_image(&png_bytes);
                        }
                        let _ = app_handle3.emit("clipboard://changed", ());
                    }));
                });
                }
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
    let sc = ASR_SHORTCUT.get().map(|s| fmt_shortcut(s)).unwrap_or_default();
    let label = match state {
        TrayState::Idle => format!("语音识别（{}）", sc),
        TrayState::Recording => "停止识别".to_string(),
        TrayState::Processing => "处理中…".to_string(),
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
