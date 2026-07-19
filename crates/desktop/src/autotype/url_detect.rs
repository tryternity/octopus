//! 用 AppleScript 取当前浏览器 active tab URL。
//!
//! 支持：Chrome / Safari / Firefox / Edge / Brave / Arc。
//! 首次调用会触发 macOS 权限授权框。

use anyhow::{Context, Result};
use std::process::Command;
use std::time::Duration;

const OSA_TIMEOUT: Duration = Duration::from_secs(5);

fn run_osascript(script: &str) -> Result<String> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let s = script.to_string();
    std::thread::spawn(move || {
        let out = Command::new("osascript").arg("-e").arg(&s).output();
        let _ = tx.send(out);
    });
    let output = rx
        .recv_timeout(OSA_TIMEOUT)
        .context("osascript 执行超时（可能未授权）")??;
    if !output.status.success() {
        anyhow::bail!(
            "osascript 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn frontmost_bundle_id() -> Result<String> {
    // NSWorkspace 通过 osascript 取
    let script = r#"
    tell application "System Events"
        set frontApp to first application process whose frontmost is true
        set bundleId to bundle identifier of frontApp
    end tell
    "#;
    Ok(run_osascript(script)?)
}

fn script_for_browser(bundle_id: &str) -> Option<&'static str> {
    match bundle_id {
        "com.google.Chrome" | "com.microsoft.edgemac" | "com.brave.Browser" => Some(
            r#"tell application "Google Chrome" to get URL of active tab of front window"#,
        ),
        "com.apple.Safari" => Some(
            r#"tell application "Safari" to get URL of current tab of front window"#,
        ),
        "org.mozilla.firefox" => Some(
            r#"tell application "System Events" to tell process "Firefox"
                get value of text field 1 of group 1 of toolbar 1 of window 1
            end tell"#,
        ),
        "company.thebrowser.Browser" => Some(
            r#"tell application "Arc" to get URL of active tab of front window"#,
        ),
        _ => None,
    }
}

pub fn current_browser_url() -> Result<Option<String>> {
    let bundle_id = match frontmost_bundle_id() {
        Ok(id) => id,
        Err(_) => return Ok(None),
    };
    let script = match script_for_browser(&bundle_id) {
        Some(s) => s,
        None => return Ok(None), // 前台不是已知浏览器
    };
    match run_osascript(script) {
        Ok(url) if !url.is_empty() => Ok(Some(url)),
        Ok(_) => Ok(None),
        Err(e) => {
            log::warn!("URL 检测失败 for {}: {}", bundle_id, e);
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_for_browser_chrome() {
        assert!(script_for_browser("com.google.Chrome").is_some());
    }

    #[test]
    fn test_script_for_browser_unknown() {
        assert!(script_for_browser("com.unknown.app").is_none());
    }

    #[test]
    fn test_script_for_browser_safari() {
        assert!(script_for_browser("com.apple.Safari").is_some());
    }

    #[test]
    fn test_script_for_browser_firefox() {
        assert!(script_for_browser("org.mozilla.firefox").is_some());
    }

    /// Chromium 系（Chrome / Edge / Brave）共享同一份 AppleScript——
    /// 都基于 Chrome 的 AppleScript 字典。三者返回必须相同（不只是 is_some）。
    #[test]
    fn test_script_for_browser_chromium_family_shares_script() {
        let chrome = script_for_browser("com.google.Chrome");
        let edge = script_for_browser("com.microsoft.edgemac");
        let brave = script_for_browser("com.brave.Browser");
        assert!(chrome.is_some(), "Chrome 应支持");
        assert_eq!(chrome, edge, "Edge 应与 Chrome 共享脚本");
        assert_eq!(chrome, brave, "Brave 应与 Chrome 共享脚本");
        // 字面量应引用 Google Chrome 应用名
        assert!(
            chrome.unwrap().contains("Google Chrome"),
            "Chromium 系脚本应指向 Google Chrome"
        );
    }

    /// Arc 浏览器应被识别为独立分支（不同 AppleScript 字典）。
    #[test]
    fn test_script_for_browser_arc() {
        let arc = script_for_browser("company.thebrowser.Browser");
        assert!(arc.is_some(), "Arc 应支持");
        assert!(
            arc.unwrap().contains("Arc"),
            "Arc 脚本应指向 Arc 应用名（而非 Chrome）"
        );
    }

    /// 空 bundle id → None（不应误匹配默认分支）。
    #[test]
    fn test_script_for_browser_empty_bundle_id_returns_none() {
        assert!(script_for_browser("").is_none());
    }
}
