//! Finder 选中捕获——检测前台是否 Finder + AppleScript 拿 selection POSIX 路径。

/// 前台 app 是否为 Finder（com.apple.finder）。
pub fn is_finder_frontmost() -> bool {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(r#"tell application "System Events" to get bundle identifier of first application process whose frontmost is true"#)
            .output();
        match output {
            Ok(o) if o.status.success() => {
                let bid = String::from_utf8_lossy(&o.stdout).trim().to_string();
                bid == "com.apple.finder"
            }
            _ => false,
        }
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
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| format!("osascript 执行失败: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("AppleScript 错误: {}", stderr));
        }
        let result = String::from_utf8_lossy(&output.stdout);
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
