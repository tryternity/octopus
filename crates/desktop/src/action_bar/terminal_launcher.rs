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
        let script = build_terminal_script(command, &cwd.to_string_lossy());
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

/// AppleScript 字符串转义：双引号、反斜杠、换行、制表符。
/// AppleScript 字符串字面量不能含裸换行（do script "..." 会在换行处截断），
/// 多文件 agent prompt 用 \n join 后必须转义为 \\n 字面量。
fn escape_applescript_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// shell 单引号转义：用单引号包裹，内部单引号用 '"'"' 转义。
/// 单引号内 $/`/\ 等全部字面，严格安全——cwd 来自用户文件路径（可能含
/// $()/反引号等 shell 元字符），必须用单引号防命令注入。
/// 与 agent_adapter::shell_escape_single 保持一致的安全级别。
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// 组装 Terminal.app AppleScript 脚本字符串（纯函数，可测试）。
/// 返回完整的 `tell application "Terminal" / do script / activate / end tell` 脚本。
pub fn build_terminal_script(command: &str, cwd: &str) -> String {
    let full_cmd = format!("cd {} && {}", shell_quote(cwd), command);
    format!(
        r#"tell application "Terminal"
    do script "{}"
    activate
end tell"#,
        escape_applescript_string(&full_cmd)
    )
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
    fn test_escape_applescript_string_plain() {
        assert_eq!(escape_applescript_string("hello world"), "hello world");
    }

    #[test]
    fn test_escape_applescript_string_multiple_quotes() {
        assert_eq!(escape_applescript_string(r#"a"b"c"#), r#"a\"b\"c"#);
    }

    #[test]
    fn test_escape_applescript_string_newline() {
        // 换行必须转义为 \n 字面量——否则 do script "..." 在换行处截断
        assert_eq!(escape_applescript_string("line1\nline2"), "line1\\nline2");
        assert_eq!(escape_applescript_string("a\r\nb"), "a\\r\\nb");
        assert_eq!(escape_applescript_string("a\tb"), "a\\tb");
    }

    #[test]
    fn test_shell_quote() {
        // 单引号包裹——含空格的路径安全
        assert_eq!(shell_quote("/Users/My User"), "'/Users/My User'");
    }

    #[test]
    fn test_shell_quote_no_spaces() {
        assert_eq!(shell_quote("/tmp"), "'/tmp'");
    }

    #[test]
    fn test_shell_quote_empty() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn test_shell_quote_single_quote_in_path() {
        // 路径含单引号用 '"'"' 转义
        assert_eq!(shell_quote("/Users/it's"), "'/Users/it'\"'\"'s'");
    }

    #[test]
    fn test_shell_quote_injection_attempt() {
        // cwd 含 $() 命令替换——单引号内全字面，不执行
        let q = shell_quote("/path/$(whoami)");
        assert_eq!(q, "'/path/$(whoami)'");
        // 反引号同样字面
        let q2 = shell_quote("/path/`whoami`");
        assert_eq!(q2, "'/path/`whoami`'");
    }

    #[test]
    fn test_build_terminal_script_basic() {
        let script = build_terminal_script("claude", "/Users/x");
        assert!(script.contains(r#"tell application "Terminal""#));
        assert!(script.contains("do script"));
        assert!(script.contains("activate"));
        // cwd 被单引号包裹（AppleScript 转义后单引号是字面字符不需转义）
        assert!(script.contains("cd '/Users/x'"));
        assert!(script.contains("&& claude"));
    }

    #[test]
    fn test_build_terminal_script_cwd_with_spaces() {
        let script = build_terminal_script("echo hi", "/Users/My User/docs");
        // 含空格的路径必须被引号包裹（AppleScript 转义后 \" 形式）
        assert!(script.contains("My User"));
    }

    #[test]
    fn test_build_terminal_script_command_with_quotes() {
        // 命令中的双引号被 AppleScript 转义为 \"
        let script = build_terminal_script(r#"echo "hello""#, "/tmp");
        assert!(script.contains(r#"echo \"hello\""#));
    }

    #[test]
    fn test_build_terminal_script_structure() {
        let script = build_terminal_script("ls", "/tmp");
        // 验证完整 AppleScript 结构
        assert!(script.starts_with("tell application \"Terminal\""));
        assert!(script.contains("end tell"));
        assert!(script.contains("activate"));
    }
}
