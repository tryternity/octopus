//! Finder 选中捕获——检测前台是否 Finder + AppleScript 拿 selection POSIX 路径。
//! 所有 osascript 调用通过子线程 + recv_timeout 做 5s 超时，
//! 防止首次自动化权限对话框永久挂起导致 TRIGGER_IN_PROGRESS 死锁。

use std::sync::mpsc;
use std::time::Duration;

/// osascript 超时时间。
const OSA_TIMEOUT: Duration = Duration::from_secs(5);

/// 带超时的 osascript 执行。
/// 用子线程跑 osascript，主线程 recv_timeout，超时直接返回 Err。
fn run_osascript_timeout(script: &str) -> Result<String, String> {
    let (tx, rx) = mpsc::channel();
    let script = script.to_string();
    std::thread::spawn(move || {
        let result = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output();
        let _ = tx.send(result);
    });
    match rx.recv_timeout(OSA_TIMEOUT) {
        Ok(Ok(output)) => {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).to_string())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                Err(stderr)
            }
        }
        Ok(Err(e)) => Err(format!("osascript 执行失败: {}", e)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err("osascript 超时（可能等待自动化权限对话框）".into())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("osascript 线程异常退出".into())
        }
    }
}

/// 前台 app 是否为 Finder（com.apple.finder）。
///
/// 2026-07-20 perf：从 osascript 改 NSWorkspace.frontmostApplication.bundleIdentifier
/// 直调（< 1ms vs osascript 启动 ~200-400ms）。
pub fn is_finder_frontmost() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::platform::app_context::macos_ax::frontmost_bundle_id().as_deref() == Some("com.apple.finder")
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// 获取 Finder 当前选中文件的 POSIX 路径列表。空选中返回空 Vec。
pub fn get_finder_selection() -> Result<Vec<String>, String> {
    #[cfg(not(target_os = "macos"))]
    {
        return Err("仅 macOS 支持 Finder 选中捕获".into());
    }

    #[cfg(target_os = "macos")]
    {
        let script = r#"
tell application "Finder"
    set sel to selection
    if (count of sel) = 0 then return ""
    set paths to ""
    repeat with f in sel
        set paths to paths & (POSIX path of (f as alias)) & linefeed
    end repeat
    return paths
end tell
"#;
        let result = run_osascript_timeout(script)?;
        let files: Vec<String> = result
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_finder_frontmost_returns_bool() {
        // 仅验证返回类型是 bool，不验证具体值（取决于运行环境）
        let _ = is_finder_frontmost();
    }
}
