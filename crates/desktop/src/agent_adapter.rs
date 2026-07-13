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

/// 内置白名单（一期）
fn builtin_adapters() -> Vec<AgentAdapter> {
    vec![
        AgentAdapter {
            key: "claude".into(),
            display_name: "Claude Code".into(),
            detect_binary: "claude".into(),
            command_template: "claude --add-dir \"{cwd}\" \"{prompt}\"".into(),
            is_builtin: true,
            is_available: false,
        },
        AgentAdapter {
            key: "pi".into(),
            display_name: "Pi".into(),
            detect_binary: "pi".into(),
            command_template: "pi {files_at} \"{prompt}\"".into(),
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
pub fn render_command(template: &str, prompt: &str, files: &[String], cwd: &str) -> String {
    let files_str = files.join(" ");
    let files_at_str = files.iter().map(|f| format!("@{}", f)).collect::<Vec<_>>().join(" ");
    template
        .replace("{prompt}", &shell_escape_single(prompt))
        .replace("{files_at}", &files_at_str)
        .replace("{files}", &files_str)
        .replace("{cwd}", cwd)
}

/// shell 单引号转义：用单引号包裹，内部单引号用 '"'"' 转义。
fn shell_escape_single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_command_claude() {
        let cmd = render_command(
            "claude --add-dir \"{cwd}\" \"{prompt}\"",
            "整理这些文件",
            &["/a.pdf".into(), "/b.pdf".into()],
            "/Users/x",
        );
        assert_eq!(cmd, "claude --add-dir \"/Users/x\" \"'整理这些文件'\"");
    }

    #[test]
    fn test_render_command_pi() {
        let cmd = render_command(
            "pi {files_at} \"{prompt}\"",
            "make ppt",
            &["/a.pdf".into(), "/b.pdf".into()],
            "/Users/x",
        );
        assert_eq!(cmd, "pi @/a.pdf @/b.pdf \"'make ppt'\"");
    }

    #[test]
    fn test_shell_escape_single_with_quote() {
        let escaped = shell_escape_single("it's a test");
        assert_eq!(escaped, "'it'\"'\"'s a test'");
    }

    #[test]
    fn test_builtin_adapters_has_claude_and_pi() {
        let builtins = builtin_adapters();
        let keys: Vec<&str> = builtins.iter().map(|a| a.key.as_str()).collect();
        assert!(keys.contains(&"claude"));
        assert!(keys.contains(&"pi"));
    }
}
