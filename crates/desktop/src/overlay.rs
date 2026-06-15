// src/overlay.rs

use crate::config::AppConfig;
use log::debug;
use tauri::{AppHandle, Emitter, Manager};

#[cfg(target_os = "linux")]
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

const OVERLAY_WIDTH: f64 = 400.0;
const OVERLAY_HEIGHT: f64 = 40.0;

/// Create the overlay window (hidden by default).
pub fn create_overlay(app: &AppHandle, config: &AppConfig) {
    if config.overlay_position == "none" {
        debug!("Overlay disabled in config");
        return;
    }

    // Skip if already exists
    if app.get_webview_window("recording_overlay").is_some() {
        return;
    }

    let builder = tauri::WebviewWindowBuilder::new(
        app,
        "recording_overlay",
        tauri::WebviewUrl::App("overlay/index.html".into()),
    )
    .title("Recording")
    .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .focused(false)
    .visible(false)
    .shadow(false);

    match builder.build() {
        Ok(window) => {
            init_platform_overlay(&window, config);
            debug!("Overlay window created");
        }
        Err(e) => debug!("Failed to create overlay: {}", e),
    }
}

/// Platform-specific overlay initialization.
#[cfg(target_os = "linux")]
fn init_platform_overlay(window: &tauri::webview::WebviewWindow, config: &AppConfig) {
    if gtk_layer_shell::is_supported() {
        if let Ok(gtk_window) = window.gtk_window() {
            gtk_window.init_layer_shell();
            gtk_window.set_layer(Layer::Overlay);
            gtk_window.set_keyboard_mode(KeyboardMode::None);
            gtk_window.set_exclusive_zone(0);
            let anchor_top = config.overlay_position == "top";
            gtk_window.set_anchor(Edge::Top, anchor_top);
            gtk_window.set_anchor(Edge::Bottom, !anchor_top);
            debug!("GTK layer shell initialized for overlay window");
        }
    } else {
        debug!("GTK layer shell not available, falling back to regular window");
    }
}

/// Platform-specific overlay initialization (no-op on non-Linux).
#[cfg(not(target_os = "linux"))]
fn init_platform_overlay(_window: &tauri::webview::WebviewWindow, _config: &AppConfig) {}

/// Show the overlay with state: "recording" or "transcribing".
#[allow(dead_code)] // overlay 显示路径已被 result_window 取代；保留入口待 overlay 子系统复用决策
pub fn show_overlay(app: &AppHandle, state: &str) {
    if let Some(window) = app.get_webview_window("recording_overlay") {
        let _ = window.show();
        let _ = window.emit("show-overlay", state);
    }
}

/// Hide the overlay with fade-out animation.
pub fn hide_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("recording_overlay") {
        let _ = window.emit("hide-overlay", ());
        let window_clone = window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let _ = window_clone.hide();
        });
    }
}

/// 显示流式识别的部分文本。
#[allow(dead_code)] // 流式部分文本改走 result_window::update_result；保留入口待复用
pub fn show_partial_text(app: &AppHandle, text: &str) {
    if let Some(window) = app.get_webview_window("recording_overlay") {
        let _ = window.emit("partial-result", text);
    }
}
