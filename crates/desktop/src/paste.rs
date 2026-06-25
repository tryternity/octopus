// src/paste.rs

use crate::config::AppConfig;
use anyhow::Result;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use log::info;
use octopus_clipboard::ClipboardHandle;
use std::time::Duration;

/// Cmd+V 后等待系统粘贴落地、再恢复原剪贴板的延迟。
const PASTE_RESTORE_DELAY: Duration = Duration::from_millis(200);

/// Paste method configuration
#[derive(Debug)]
pub enum PasteMethod {
    Clipboard,
    Direct,
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
pub fn paste(text: &str, handle: &ClipboardHandle, config: &AppConfig) -> Result<()> {
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
            write_to_clipboard(text, handle)?;
        }
        PasteMethod::Clipboard => {
            paste_via_clipboard(text, handle, wtc)?;
        }
        PasteMethod::Direct => {
            paste_direct(text, handle, wtc)?;
        }
    }

    Ok(())
}

fn write_to_clipboard(text: &str, handle: &ClipboardHandle) -> Result<()> {
    handle.write_text(text)?;
    Ok(())
}

fn paste_via_clipboard(
    text: &str,
    handle: &ClipboardHandle,
    write_to_clipboard: bool,
) -> Result<()> {
    let saved = if !write_to_clipboard {
        handle.read_text().unwrap_or_default()
    } else {
        String::new()
    };

    handle.write_text(text)?;

    std::thread::sleep(Duration::from_millis(50));

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("Enigo init failed: {}", e))?;

    #[cfg(target_os = "macos")]
    let mod_key = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let mod_key = Key::Control;

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

    std::thread::sleep(PASTE_RESTORE_DELAY);

    if !write_to_clipboard && !saved.is_empty() {
        let _ = handle.write_text(&saved);
    }

    Ok(())
}

fn paste_direct(
    text: &str,
    handle: &ClipboardHandle,
    write_to_clipboard: bool,
) -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| anyhow::anyhow!("Enigo init failed: {}", e))?;

    #[cfg(target_os = "linux")]
    {
        if try_linux_direct_typing(text) {
            if write_to_clipboard {
                let _ = handle.write_text(text);
            }
            return Ok(());
        }
        info!("Falling back to enigo for direct input");
    }

    enigo
        .text(text)
        .map_err(|e| anyhow::anyhow!("Direct type failed: {}", e))?;

    if write_to_clipboard {
        handle.write_text(text)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn try_linux_direct_typing(text: &str) -> bool {
    use std::process::Command;

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
