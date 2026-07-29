//! 跨平台「在文件管理器定位文件」+「用默认程序打开 URL/文件」helper。
//!
//! 抽自 search_commands::reveal_path（三分支版）+ clipboard_commands/record_commands
//! 的重复实现（2026-07-29 DRY 重构）。统一正确性 + 修复 3 处 macOS-only 硬编码的
//! 跨平台缺陷（reveal_recording / reveal_subtitle / open_recording_file 原本 Win/Linux 不可用）。

use std::path::Path;

/// 在文件管理器中定位文件（macOS Finder `open -R` / Windows Explorer `/select,` / Linux `xdg-open parent`）。
///
/// macOS 检查退出码（失败返 Err）；Windows/Linux 是 fire-and-forget（spawn 后不检查，
/// 与原 clipboard_commands::reveal_in_file_manager 一致——explorer/xdg-open 的退出码语义不可靠）。
pub(crate) fn reveal_path(path: impl AsRef<Path>) -> Result<(), String> {
    let path = path.as_ref();
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open")
            .args(["-R"])
            .arg(path)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("定位失败（exit {}）: {}", status, path.display()));
        }
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
    Ok(())
}

/// 在文件管理器定位文件（lossy 版：失败仅 log，不返 Err）。
///
/// 给「reveal 失败不应影响主流程」的场景用（如录屏停止后自动 reveal——失败不该让停止失败）。
pub(crate) fn reveal_path_lossy(path: impl AsRef<Path>) {
    if let Err(e) = reveal_path(path) {
        log::warn!("[sys_open] reveal 失败: {e}");
    }
}

/// 用系统默认程序打开 URL 或文件（macOS `open` / Windows `cmd /c start ""` / Linux `xdg-open`）。
///
/// 检查退出码（失败返 Err）。覆盖 open_url + open_path 语义（macOS/Linux 同命令，Windows 都用 `cmd /c start ""`）。
pub(crate) fn open_with_default(target: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("open")
            .arg(target)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("打开失败（exit {}）: {}", status, target));
        }
    }
    #[cfg(target_os = "windows")]
    {
        let status = std::process::Command::new("cmd")
            .args(["/c", "start", ""])
            .arg(target)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("打开失败（exit {}）: {}", status, target));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("xdg-open")
            .arg(target)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("打开失败（exit {}）: {}", status, target));
        }
    }
    Ok(())
}
