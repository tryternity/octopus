//! 终端启动器抽象——trait + Terminal.app 实现。

use std::path::Path;

pub trait TerminalLauncher {
    /// 在新终端窗口执行命令，cwd 指定工作目录。
    fn spawn(&self, command: &str, cwd: &Path) -> Result<(), String>;
}

/// 一期实现：Terminal.app via AppleScript `do script`（打开新窗口）。
pub struct TerminalAppLauncher;

impl TerminalLauncher for TerminalAppLauncher {
    #[cfg(target_os = "macos")]
    fn spawn(&self, command: &str, cwd: &Path) -> Result<(), String> {
        let cwd_str = cwd.to_string_lossy();
        // 组装完整 shell 命令：cd 到工作目录 → 执行命令
        let full_cmd = format!("cd {} && {}", shell_quote(&cwd_str), command);
        // AppleScript：tell application "Terminal" → do script（新窗口）→ activate
        let script = format!(
            r#"tell application "Terminal"
    do script "{}"
    activate
end tell"#,
            escape_applescript_string(&full_cmd)
        );
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| format!("启动 Terminal.app 失败: {}", e))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Terminal.app 启动失败: {}", stderr));
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn spawn(&self, _command: &str, _cwd: &Path) -> Result<(), String> {
        Err("仅 macOS 支持 Terminal.app 启动".into())
    }
}

/// AppleScript 字符串转义：双引号和反斜杠。
fn escape_applescript_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// shell 引号包裹路径（处理含空格的路径）。
fn shell_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_applescript_string() {
        assert_eq!(escape_applescript_string(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(escape_applescript_string(r#"C:\path"#), r#"C:\\path"#);
    }

    #[test]
    fn test_shell_quote() {
        assert_eq!(shell_quote("/Users/My User"), r#""/Users/My User""#);
        assert_eq!(shell_quote(r#"a"b"#), r#""a\"b""#);
    }
}
