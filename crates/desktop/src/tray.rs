// src/tray.rs

use crate::config::DesktopConfig;
use log::info;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, Runtime};

/// Tray icon state for display purposes
pub enum TrayState {
    Idle,
    Recording,
    Processing,
}

/// Create the system tray icon and its context menu.
///
/// The tray icon id is set to "main" via [`TrayIconBuilder::with_id`] so that
/// [`update_tray_label`] can look it up later with `app.tray_by_id("main")`.
/// The Coordinator must be managed as Tauri state (Task 10) for the toggle
/// handler to work.
pub fn create_tray<R: Runtime>(app: &tauri::AppHandle<R>, config: &DesktopConfig) {
    let toggle = MenuItem::with_id(app, "toggle", "开始录音", true, None::<&str>)
        .expect("failed to create toggle menu item");
    let engine_info = MenuItem::with_id(
        app,
        "engine_info",
        format!("引擎: {} ({})", config.asr_engine, config.engine_mode),
        false,
        None::<&str>,
    )
    .expect("failed to create engine_info menu item");
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .expect("failed to create quit menu item");

    let menu =
        Menu::with_items(app, &[&toggle, &engine_info, &quit]).expect("failed to create tray menu");

    let _tray = TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .cloned()
                .unwrap_or_else(|| Image::from_bytes(include_bytes!("../icons/icon.png")).unwrap()),
        )
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("octopus - Speech to Text")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => {
                info!("Tray: toggle recording");
                if let Some(coordinator) = app.try_state::<crate::coordinator::Coordinator>() {
                    coordinator.toggle();
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
///
/// Looks up the tray by id "main" and rebuilds the menu with the updated
/// toggle label. The engine_info and quit items are recreated as well since
/// `TrayIcon` does not expose a `.menu()` getter in Tauri 2.x.
pub fn update_tray_label<R: Runtime>(app: &tauri::AppHandle<R>, state: TrayState) {
    if let Some(tray) = app.tray_by_id("main") {
        let label = match state {
            TrayState::Idle => "开始录音",
            TrayState::Recording => "■ 停止录音",
            TrayState::Processing => "⏳ 处理中...",
        };

        let toggle = MenuItem::with_id(app, "toggle", label, true, None::<&str>)
            .expect("failed to create toggle menu item");
        let engine_info = MenuItem::with_id(app, "engine_info", "引擎", false, None::<&str>)
            .expect("failed to create engine_info menu item");
        let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
            .expect("failed to create quit menu item");

        let menu = Menu::with_items(app, &[&toggle, &engine_info, &quit])
            .expect("failed to create tray menu");

        let _ = tray.set_menu(Some(menu));
    }
}
