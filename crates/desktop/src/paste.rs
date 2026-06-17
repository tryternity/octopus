// src/paste.rs

use crate::config::AppConfig;
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
    config: &AppConfig,
) -> Result<()> {
    let method = PasteMethod::from(config.paste_method.as_str());
    let wtc = config.write_to_clipboard;
    info!(
        "Pasting via {:?}, write_to_clipboard={}, text len: {}",
        method,
        wtc,
        text.len()
    );

    match method {
        PasteMethod::None => {
            // None 模式：唯一目的就是写剪贴板，忽略 write_to_clipboard 配置
            write_to_clipboard(text, app_handle)?;
        }
        PasteMethod::Clipboard => {
            paste_via_clipboard(text, app_handle, wtc)?;
        }
        PasteMethod::Direct => {
            paste_direct(text, app_handle, wtc)?;
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

fn paste_via_clipboard<R: Runtime>(
    text: &str,
    app_handle: &tauri::AppHandle<R>,
    write_to_clipboard: bool,
) -> Result<()> {
    let clipboard = app_handle.clipboard();

    // 仅在不保留识别结果时，才需要保存原剪贴板以便恢复
    let saved = if !write_to_clipboard {
        clipboard.read_text().unwrap_or_default()
    } else {
        String::new()
    };

    clipboard
        .write_text(text)
        .map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))?;

    std::thread::sleep(Duration::from_millis(50));

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("Enigo init failed: {}", e))?;

    #[cfg(target_os = "macos")]
    let mod_key = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let mod_key = Key::Control;

    // macOS：用固定虚拟键码（kVK_ANSI_V=9）而非 Key::Unicode('v')。
    // enigo 0.6.1 的 Key::Unicode 在 macOS 上走 get_layoutdependent_keycode，
    // 该函数循环调用 Carbon TIS API（TISCopyCurrentKeyboardInputSource /
    // UCKeyTranslate），这些 HIToolbox API 非线程安全，在 spawn_blocking 的
    // 非主线程中调用会触发 macOS 线程断言 → SIGTRAP（Trace/BPT trap: 5）。
    #[cfg(target_os = "macos")]
    let v_key = Key::Other(9);
    #[cfg(not(target_os = "macos"))]
    let v_key = Key::Unicode('v');

    enigo
        .key(mod_key, Direction::Press)
        .map_err(|e| anyhow::anyhow!("Mod press: {}", e))?;
    enigo
        .key(v_key, Direction::Click)
        .map_err(|e| anyhow::anyhow!("V click: {}", e))?;
    enigo
        .key(mod_key, Direction::Release)
        .map_err(|e| anyhow::anyhow!("Mod release: {}", e))?;

    std::thread::sleep(Duration::from_millis(50));

    // 仅在不保留识别结果时恢复原剪贴板
    if !write_to_clipboard {
        let _ = clipboard.write_text(&saved);
    }

    Ok(())
}

fn paste_direct<R: Runtime>(
    text: &str,
    app_handle: &tauri::AppHandle<R>,
    write_to_clipboard: bool,
) -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("Enigo init failed: {}", e))?;

    #[cfg(target_os = "linux")]
    {
        if try_linux_direct_typing(text) {
            if write_to_clipboard {
                let clipboard = app_handle.clipboard();
                let _ = clipboard.write_text(text);
            }
            return Ok(());
        }
        info!("Falling back to enigo for direct input");
    }

    enigo
        .text(text)
        .map_err(|e| anyhow::anyhow!("Direct type failed: {}", e))?;

    // 粘贴完成后按需写剪贴板
    if write_to_clipboard {
        let clipboard = app_handle.clipboard();
        clipboard
            .write_text(text)
            .map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))?;
    }
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
