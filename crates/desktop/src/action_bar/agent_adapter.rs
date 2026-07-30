//! Agent 适配器注册表——纯 DB 驱动 + PATH 检测 + 命令模板渲染 + 三层 fallback。
//!
//! 2026-07-19 v42 改：Pi / Claude 不再在 Rust 常量里硬编码，由 db.sql seed
//! 进 agent_adapters 表（is_system=1）。本模块仅负责读 DB + which 检测 + 渲染。

/// Agent 适配器——描述一个 CLI agent 的检测与启动方式。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAdapter {
    pub id: i64,
    pub key: String,
    pub display_name: String,
    pub detect_binary: String,
    pub command_template: String,
    pub is_system: bool,
    pub is_default: bool,
    pub is_available: bool,
}

/// 列出全部 adapter（内置 + 用户自定义），逐个 which 检测安装状态。
pub fn list_adapters() -> Vec<AgentAdapter> {
    let records = octopus_infra::db::list_agent_adapter_records().unwrap_or_default();
    records.into_iter().map(|r| {
        let is_available = which(&r.detect_binary);
        AgentAdapter {
            id: r.id,
            key: r.key,
            display_name: r.display_name,
            detect_binary: r.detect_binary,
            command_template: r.command_template,
            is_system: r.is_system,
            is_default: r.is_default,
            is_available,
        }
    }).collect()
}

/// 重新检测所有 adapter（设置页「刷新检测」按钮用）。
pub fn refresh_detection() -> Vec<AgentAdapter> {
    list_adapters()
}

/// 解析菜单项 agent_key 到最终使用的 adapter——三层 fallback：
///
/// 1. **菜单指定**：传入的 key 非空 → 查 DB 该 key 存在 + which(detect_binary) 可用 → 用之
/// 2. **系统默认**：菜单 key 不可用 / 空 → 查 agent_adapters WHERE is_default=1 → 该 adapter 可用 → 用之
/// 3. **第一个可用**：默认也不可用 → 取 list 中第一个 is_available=true 的 → 用之
/// 4. 都不可用 → 返回 Err，调用方应报错给用户
///
/// 返回 (adapter, effective_source)：effective_source 用于日志/调试，
/// 标识走了哪一层（"menu" / "default" / "fallback_first"）。
pub fn resolve_effective_adapter(menu_agent_key: &str) -> Result<(AgentAdapter, &'static str), String> {
    let adapters = list_adapters();

    // 1. 菜单指定
    if !menu_agent_key.is_empty() {
        if let Some(a) = adapters.iter().find(|a| a.key == menu_agent_key) {
            if a.is_available {
                return Ok((a.clone(), "menu"));
            }
            log::warn!(
                "[agent-adapter] 菜单指定 agent '{}' 不可用（PATH 找不到 `{}`），fallback 到默认",
                a.key, a.detect_binary
            );
        } else {
            log::warn!(
                "[agent-adapter] 菜单指定 agent '{}' 不存在，fallback 到默认",
                menu_agent_key
            );
        }
    }

    // 2. 系统默认
    if let Some(a) = adapters.iter().find(|a| a.is_default) {
        if a.is_available {
            return Ok((a.clone(), "default"));
        }
        log::warn!(
            "[agent-adapter] 默认 agent '{}' 不可用（PATH 找不到 `{}`），fallback 到第一个可用",
            a.key, a.detect_binary
        );
    }

    // 3. 第一个可用
    if let Some(a) = adapters.iter().find(|a| a.is_available) {
        return Ok((a.clone(), "fallback_first"));
    }

    // 4. 都不可用
    Err(format!(
        "没有可用的 agent（菜单指定='{}'；默认不可用；列表全部未安装）",
        menu_agent_key
    ))
}

/// 检测二进制是否可找到——三层 fallback（仿 tolaria cli_agent_runtime）。
///
/// 打包版 .app 从 Finder 启动时 PATH 只有 /usr/bin:/bin，`which` 找不到
/// homebrew/fnm/nvm 装的工具。三层策略：
/// 1. 直接 `which`（进程 PATH 能找到就用，cargo run 不受影响）
/// 2. 用户 login shell 的 `command -v`（`$SHELL -lc`，加载 ~/.zshrc / ~/.bash_profile，
///    含 homebrew/fnm/nvm PATH 设置）——macOS GUI app 的关键修复
/// 3. 硬编码候选路径探测（homebrew / .local/bin / cargo / fnm glob / nvm glob）
fn which(binary: &str) -> bool {
    // 层 1：进程 PATH
    if std::process::Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return true;
    }

    // 层 2：用户 login shell（不硬编码 zsh——用 $SHELL 拿用户实际 shell）
    if let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        let found = std::process::Command::new(&shell)
            .arg("-lc")
            .arg(format!("command -v {}", binary))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if found {
            return true;
        }
    }

    // 层 3：硬编码候选路径（shell 失败/未装时的兜底）
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/Shared".into());
    let candidates = [
        format!("/opt/homebrew/bin/{}", binary),
        format!("/usr/local/bin/{}", binary),
        format!("{}/.local/bin/{}", home, binary),
        format!("{}/.cargo/bin/{}", home, binary),
        format!("{}/.bun/bin/{}", home, binary),
    ];
    for path in &candidates {
        if std::path::Path::new(path).exists() {
            return true;
        }
    }

    // 层 3b：fnm / nvm 动态版本号路径（glob 最新版本）
    for (base, suffix) in [
        (format!("{}/.local/share/fnm/node-versions", home), "installation/bin"),
        (format!("{}/.nvm/versions/node", home), "bin"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&base) {
            if let Some(latest) = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect::<Vec<_>>()
                .into_iter()
                .max_by_key(|e| e.file_name())
            {
                let bin_path = latest.path().join(suffix).join(binary);
                if bin_path.exists() {
                    return true;
                }
            }
        }
    }

    false
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

    // builtin_adapters + is_builtin 已删除（v42 改为纯 DB 驱动）。
    // Pi/Claude 内置由 db.sql seed 保证，测试覆盖在 infra crate 的 db.rs。

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
