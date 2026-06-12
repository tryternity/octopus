// src/paste.rs

use crate::config::DesktopConfig;
use anyhow::Result;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use log::info;
use std::time::Duration;
use tauri::Runtime;
use tauri_plugin_clipboard_manager::ClipboardExt;

/// Paste method configuration
#[derive(Debug)]
pub enum PasteMethod {
    /// Write to clipboard + Cmd/Ctrl+V
    Clipboard,
    /// Direct input to active window
    Direct,
    /// Only write to clipboard, no auto-paste
    None,
}

impl From<&str> for PasteMethod {
    fn from(s: &str) -> Self {
        match s {
            "direct" => PasteMethod::Direct,
            "none" => PasteMethod::None,
            _ => PasteMethod::Clipboard,
        }
    }
}

/// Paste transcribed text to the active window
pub fn paste<R: Runtime>(
    text: &str,
    app_handle: &tauri::AppHandle<R>,
    config: &DesktopConfig,
) -> Result<()> {
    let method = PasteMethod::from(config.paste_method.as_str());
    info!("Pasting via {:?}, text len: {}", method, text.len());

    match method {
        PasteMethod::None => {
            write_to_clipboard(text, app_handle)?;
        }
        PasteMethod::Clipboard => {
            paste_via_clipboard(text, app_handle)?;
        }
        PasteMethod::Direct => {
            paste_direct(text)?;
        }
    }

    Ok(())
}

fn write_to_clipboard<R: Runtime>(text: &str, app_handle: &tauri::AppHandle<R>) -> Result<()> {
    let clipboard = app_handle.clipboard();
    clipboard
        .write_text(text)
        .map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))?;
    Ok(())
}

fn paste_via_clipboard<R: Runtime>(text: &str, app_handle: &tauri::AppHandle<R>) -> Result<()> {
    let clipboard = app_handle.clipboard();

    // 1. Save current clipboard content
    let saved = clipboard.read_text().unwrap_or_default();

    // 2. Write transcribed text to clipboard
    clipboard
        .write_text(text)
        .map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))?;

    // 3. Wait for clipboard to take effect
    std::thread::sleep(Duration::from_millis(50));

    // 4. Send Cmd+V (macOS) / Ctrl+V (Linux/Windows)
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("Enigo init failed: {}", e))?;

    #[cfg(target_os = "macos")]
    let mod_key = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let mod_key = Key::Control;

    enigo
        .key(mod_key, Direction::Press)
        .map_err(|e| anyhow::anyhow!("Mod press: {}", e))?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| anyhow::anyhow!("V click: {}", e))?;
    enigo
        .key(mod_key, Direction::Release)
        .map_err(|e| anyhow::anyhow!("Mod release: {}", e))?;

    // 5. Wait for paste to complete
    std::thread::sleep(Duration::from_millis(50));

    // 6. Restore original clipboard content
    let _ = clipboard.write_text(&saved);

    Ok(())
}

fn paste_direct(text: &str) -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("Enigo init failed: {}", e))?;

    #[cfg(target_os = "linux")]
    {
        if try_linux_direct_typing(text) {
            return Ok(());
        }
        info!("Falling back to enigo for direct input");
    }

    enigo
        .text(text)
        .map_err(|e| anyhow::anyhow!("Direct type failed: {}", e))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn try_linux_direct_typing(text: &str) -> bool {
    use std::process::Command;

    // X11: xdotool
    if Command::new("which")
        .arg("xdotool")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        if Command::new("xdotool")
            .args(["type", "--clearmodifiers", "--", text])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return true;
        }
    }

    // Wayland: wtype
    if Command::new("which")
        .arg("wtype")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        if Command::new("wtype")
            .arg("--")
            .arg(text)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return true;
        }
    }

    false
}
