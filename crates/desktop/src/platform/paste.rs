// src/paste.rs

use crate::config::AppConfig;
use anyhow::Result;
use enigo::{Enigo, Keyboard, Settings};
#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Key};
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
    let switch_ime = config.switch_input_source_on_paste;
    info!(
        "Pasting via {:?}, write_to_clipboard={}, switch_ime={}, text len: {}",
        method,
        wtc,
        switch_ime,
        text.len()
    );

    match method {
        PasteMethod::None => {
            write_to_clipboard(text, handle)?;
        }
        PasteMethod::Clipboard => {
            paste_via_clipboard(text, handle, wtc, switch_ime)?;
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

/// 剪贴板备份：write_to_clipboard=false 粘贴后还原用户原内容。
/// 按格式备份，避免 ASR 文本覆盖掉用户原有的图片 / 文件（旧实现只 read_text，
/// 图片/文件被吞成空串 → 不还原 → 丢失）。
enum ClipboardBackup {
    Text(String),
    Image(octopus_clipboard::RustImageData),
    Files(Vec<String>),
    Empty,
}

/// 探测当前剪贴板格式并备份（优先级 files > image > text，与 watcher 一致）。
fn backup_clipboard(handle: &ClipboardHandle) -> ClipboardBackup {
    use octopus_clipboard::ContentFormat;
    if handle.has(ContentFormat::Files) {
        if let Ok(files) = handle.read_files() {
            if !files.is_empty() {
                return ClipboardBackup::Files(files);
            }
        }
    }
    if handle.has(ContentFormat::Image) {
        if let Ok(img) = handle.read_image() {
            return ClipboardBackup::Image(img);
        }
    }
    if handle.has(ContentFormat::Text) {
        if let Ok(text) = handle.read_text() {
            if !text.is_empty() {
                return ClipboardBackup::Text(text);
            }
        }
    }
    ClipboardBackup::Empty
}

/// 还原备份到剪贴板（内部 write_*/set_image 均设 suppress，
/// 避免还原被 watcher 当作新条目记录）。
fn restore_clipboard(handle: &ClipboardHandle, backup: ClipboardBackup) {
    match backup {
        ClipboardBackup::Text(t) => {
            let _ = handle.write_text(&t);
        }
        ClipboardBackup::Image(img) => {
            let _ = handle.set_image(img);
        }
        ClipboardBackup::Files(f) => {
            let _ = handle.write_files(f);
        }
        ClipboardBackup::Empty => {}
    }
}

fn paste_via_clipboard(
    text: &str,
    handle: &ClipboardHandle,
    write_to_clipboard: bool,
    switch_ime: bool,
) -> Result<()> {
    let saved = if !write_to_clipboard {
        backup_clipboard(handle)
    } else {
        ClipboardBackup::Empty
    };

    handle.write_text(text)?;

    std::thread::sleep(Duration::from_millis(50));

    // 三段式文本注入：切到 ASCII 输入源 → Cmd+V → guard drop 时恢复。
    // 避免 CJK IME composing 状态下粘贴出乱码（参考 VoxFlow VoxFlowTextInsertion）。
    let _ime_guard = if switch_ime {
        crate::platform::input_source::switch_to_ascii_for_paste()
    } else {
        None
    };

    // 2026-07-20 perf：原 enigo 三段式（Press Mod → Click V → Release Mod）改用统一
    // keystroke 模块（macOS 走 CGEvent，与 focus_tracker 共用）。其他平台 keystroke
    // 是 no-op，保留 enigo fallback（仅非 macOS 走 enigo）。
    #[cfg(target_os = "macos")]
    {
        crate::platform::keystroke::paste()?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("Enigo init failed: {}", e))?;
        let mod_key = Key::Control;
        let v_key = Key::Unicode('v');
        enigo.key(mod_key, Direction::Press).map_err(|e| anyhow::anyhow!("Mod press: {}", e))?;
        enigo.key(v_key, Direction::Click).map_err(|e| anyhow::anyhow!("V click: {}", e))?;
        enigo.key(mod_key, Direction::Release).map_err(|e| anyhow::anyhow!("Mod release: {}", e))?;
    }

    std::thread::sleep(PASTE_RESTORE_DELAY);

    if !write_to_clipboard {
        restore_clipboard(handle, saved);
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
