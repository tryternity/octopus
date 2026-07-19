//! Agent 适配器注册表——内置白名单 + DB 用户自定义 + PATH 检测 + 命令模板渲染。

/// Agent 适配器——描述一个 CLI agent 的检测与启动方式。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdapter {
    pub key: String,
    pub display_name: String,
    pub detect_binary: String,
    pub command_template: String,
    pub is_builtin: bool,
    pub is_available: bool,
}

/// 检查 key 是否为内置 adapter（零进程开销，不走 which）。
pub fn is_builtin_key(key: &str) -> bool {
    builtin_adapters().iter().any(|a| a.key == key)
}

/// 内置白名单（一期）
fn builtin_adapters() -> Vec<AgentAdapter> {
    vec![
        AgentAdapter {
            key: "claude".into(),
            display_name: "Claude Code".into(),
            detect_binary: "claude".into(),
            command_template: "claude --add-dir {cwd} {prompt}".into(),
            is_builtin: true,
            is_available: false,
        },
        AgentAdapter {
            key: "pi".into(),
            display_name: "Pi".into(),
            detect_binary: "pi".into(),
            command_template: "pi {files_at} {prompt}".into(),
            is_builtin: true,
            is_available: false,
        },
    ]
}

/// 合并内置 + DB 用户自定义 adapter，逐个检测 PATH。
pub fn list_adapters() -> Vec<AgentAdapter> {
    let mut adapters = builtin_adapters();
    if let Ok(custom) = octopus_infra::db::list_agent_adapter_records() {
        for r in custom {
            adapters.push(AgentAdapter {
                key: r.key,
                display_name: r.display_name,
                detect_binary: r.detect_binary,
                command_template: r.command_template,
                is_builtin: false,
                is_available: false,
            });
        }
    }
    for a in adapters.iter_mut() {
        a.is_available = which(&a.detect_binary);
    }
    adapters
}

/// 重新检测所有 adapter（设置页「刷新检测」按钮用）。
pub fn refresh_detection() -> Vec<AgentAdapter> {
    list_adapters()
}

/// which <binary> —— 检测 PATH 中是否存在二进制。
fn which(binary: &str) -> bool {
    std::process::Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 按模板渲染命令字符串。
/// prompt: 渲染后的 prompt（含 task）
/// files: POSIX 路径列表
/// cwd: 工作目录
///
/// **目录处理**（2026-07-19 v40 修复）：Pi 的 `@<path>` 语法只接受文件，传目录会 EISDIR 崩。
/// `{files_at}` 渲染时检测每个路径：是目录则降级为不加 `@`（让 prompt 文本里的路径
/// 引导 agent 自己 `ls`）；是文件则正常加 `@`。
/// `{files}` 不加 `@`，本来就只是路径列表，文件/目录都安全。
pub fn render_command(template: &str, prompt: &str, files: &[String], cwd: &str) -> String {
    let files_str = files.iter().map(|f| shell_quote(f)).collect::<Vec<_>>().join(" ");
    // files_at：文件加 @ 前缀，目录不加（pi @<dir> 会 EISDIR 崩）
    let files_at_str = files.iter()
        .map(|f| {
            let quoted = shell_quote(f);
            if std::path::Path::new(f).is_dir() {
                // 目录：直接传裸路径，agent 在 prompt 指引下自己 walk
                quoted
            } else {
                format!("@{}", quoted)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    template
        .replace("{prompt}", &shell_escape_single(prompt))
        .replace("{files_at}", &files_at_str)
        .replace("{files}", &files_str)
        .replace("{cwd}", &shell_quote(cwd))
}

/// 路径转义：直接用单引号（严格安全，$ / ` / \ 全部字面）
fn shell_quote(s: &str) -> String {
    shell_escape_single(s)
}

/// shell 单引号转义：用单引号包裹，内部单引号用 '"'"' 转义。
/// 单引号内 $/`/\ 等全部字面，严格安全。
fn shell_escape_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_command_claude() {
        let cmd = render_command(
            "claude --add-dir {cwd} {prompt}",
            "整理这些文件",
            &["/a.pdf".into(), "/b.pdf".into()],
            "/Users/x",
        );
        assert_eq!(cmd, "claude --add-dir '/Users/x' '整理这些文件'");
    }

    #[test]
    fn test_render_command_pi() {
        let cmd = render_command(
            "pi {files_at} {prompt}",
            "make ppt",
            &["/a.pdf".into(), "/b.pdf".into()],
            "/Users/x",
        );
        assert_eq!(cmd, "pi @'/a.pdf' @'/b.pdf' 'make ppt'");
    }

    /// v40 修复：Pi `@<dir>` 会 EISDIR 崩。render_command 检测目录，目录不加 @ 前缀。
    #[test]
    fn test_render_command_pi_directory_no_at_prefix() {
        // 用 tempdir 造一个真实目录（pi 检测 std::path::Path::is_dir()）
        let tmp = std::env::temp_dir().join(format!("octopus-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let dir_path = tmp.to_string_lossy().to_string();
        let file_path = tmp.join("a.pdf").to_string_lossy().to_string();
        std::fs::write(&file_path, "").unwrap();

        let cmd = render_command(
            "pi {files_at} {prompt}",
            "make ppt",
            &[dir_path.clone(), file_path.clone()],
            "/Users/x",
        );
        // 期望：目录不加 @，文件加 @
        let expected = format!("pi '{}' @'{}' 'make ppt'", dir_path, file_path);
        assert_eq!(cmd, expected);

        // 清理
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_shell_escape_single_with_quote() {
        let escaped = shell_escape_single("it's a test");
        assert_eq!(escaped, "'it'\"'\"'s a test'");
    }

    #[test]
    fn test_shell_escape_single_no_quotes() {
        // 无单引号——直接包裹
        let escaped = shell_escape_single("hello world");
        assert_eq!(escaped, "'hello world'");
    }

    #[test]
    fn test_shell_escape_single_empty() {
        assert_eq!(shell_escape_single(""), "''");
    }

    #[test]
    fn test_builtin_adapters_has_claude_and_pi() {
        let builtins = builtin_adapters();
        let keys: Vec<&str> = builtins.iter().map(|a| a.key.as_str()).collect();
        assert!(keys.contains(&"claude"));
        assert!(keys.contains(&"pi"));
    }

    #[test]
    fn test_builtin_adapters_are_marked_builtin() {
        let builtins = builtin_adapters();
        assert!(builtins.iter().all(|a| a.is_builtin));
    }

    #[test]
    fn test_render_command_empty_files() {
        // 空文件列表——{files} 和 {files_at} 渲染为空串
        let cmd = render_command("tool {files} {files_at} {prompt}", "do something", &[], "/tmp");
        assert_eq!(cmd, "tool   'do something'");
    }

    #[test]
    fn test_render_command_no_placeholders() {
        // 模板不含任何占位符——原样返回
        let cmd = render_command("echo hello", "ignored", &["/a".into()], "/tmp");
        assert_eq!(cmd, "echo hello");
    }

    #[test]
    fn test_render_command_prompt_with_special_chars() {
        // prompt 含 $ 和 ` ——shell_escape_single 用单引号包裹后不解释
        let cmd = render_command("tool {prompt}", "echo $HOME `whoami`", &[], "/tmp");
        assert_eq!(cmd, "tool 'echo $HOME `whoami`'");
    }

    #[test]
    fn test_render_command_single_file() {
        let cmd = render_command("tool {files}", "", &["/path/to/file.pdf".into()], "/tmp");
        assert_eq!(cmd, "tool '/path/to/file.pdf'");
    }

    #[test]
    fn test_render_command_multiple_files_at() {
        let cmd = render_command(
            "tool {files_at}",
            "",
            &["/a.pdf".into(), "/b.jpg".into(), "/c.docx".into()],
            "/tmp",
        );
        assert_eq!(cmd, "tool @'/a.pdf' @'/b.jpg' @'/c.docx'");
    }

    #[test]
    fn test_render_command_path_with_spaces() {
        // 含空格的路径——单引号包裹安全
        let cmd = render_command(
            "tool {files} {cwd}",
            "",
            &["/Users/John Doe/report.pdf".into()],
            "/Users/John Doe",
        );
        assert_eq!(cmd, "tool '/Users/John Doe/report.pdf' '/Users/John Doe'");
    }

    #[test]
    fn test_render_command_path_with_shell_metachar() {
        // 路径含 shell 元字符——单引号内全部字面，不解释
        let cmd = render_command("tool {files}", "", &["/tmp/a;echo b".into()], "/tmp");
        assert_eq!(cmd, "tool '/tmp/a;echo b'");
    }

    #[test]
    fn test_render_command_path_with_dollar() {
        // 路径含 $ ——单引号内不展开
        let cmd = render_command("tool {files}", "", &["/tmp/$HOME".into()], "/tmp");
        assert_eq!(cmd, "tool '/tmp/$HOME'");
    }

    #[test]
    fn test_render_command_path_with_backtick() {
        // 路径含 ` ——单引号内不执行命令替换
        let cmd = render_command("tool {files}", "", &["/tmp/`whoami`".into()], "/tmp");
        assert_eq!(cmd, "tool '/tmp/`whoami`'");
    }
}
