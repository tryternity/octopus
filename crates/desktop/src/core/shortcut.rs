// src/shortcut.rs

//! Global keyboard shortcut registration.
//!
//! Registers a configurable global shortcut (default: CmdOrCtrl+Shift+Space)
//! that toggles recording on/off via the Coordinator.

use crate::engine::coordinator::Coordinator;
use log::{error, info};
use tauri::{Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// Register a global shortcut that toggles the coordinator on press.
///
/// The `shortcut_str` should be in Tauri's shortcut format, e.g.
/// `"CmdOrCtrl+Shift+Space"`.
///
/// The `tauri_plugin_global_shortcut` plugin **must** already be registered
/// on the Tauri Builder before calling this function.
pub fn register_shortcut<R: Runtime>(
    app: &tauri::AppHandle<R>,
    shortcut_str: &str,
) -> Result<(), String> {
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_str, e))?;

    app.global_shortcut()
        .on_shortcut(shortcut, move |app_handle, _scut, event| {
            if event.state == ShortcutState::Pressed {
                info!("Global shortcut pressed");
                if let Some(coordinator) = app_handle.try_state::<Coordinator>() {
                    coordinator.toggle();
                } else {
                    error!("Coordinator not found in Tauri state");
                }
            }
        })
        .map_err(|e| format!("Failed to register shortcut '{}': {}", shortcut_str, e))?;

    info!("Registered global shortcut: {}", shortcut_str);
    Ok(())
}
