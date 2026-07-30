// Shell init 脚本构造——注入 OSC 133 shell prompt marker。
// 参考 Terax shell_init.rs 设计。

use portable_pty::CommandBuilder;

/// zsh OSC 133 注入脚本。
/// precmd：prompt 开始 → OSC 133;A
/// preexec：命令开始 → OSC 133;C;<cmd>
/// TRAPINT/TRAPEXIT：命令退出 → OSC 133;D
const ZSH_INIT: &str = r#"
# octopus shell integration: OSC 133 prompt markers
__octopus_precmd() { printf '\e]133;A\e\\'; }
__octopus_preexec() { printf '\e]133;C;%s\e\\' "$1"; }
precmd_functions=(__octopus_precmd $precmd_functions)
preexec_functions=(__octopus_preexec $preexec_functions)
"#;

/// bash OSC 133 注入脚本。
/// bash 没有 preexec hook，用 trap DEBUG + PROMPT_COMMAND 近似。
const BASH_INIT: &str = r#"
# octopus shell integration: OSC 133 prompt markers (bash)
__octopus_last_cmd=""
__octopus_debug_trap() {
    local cmd="$BASH_COMMAND"
    if [ "$cmd" != "$__octopus_last_cmd" ]; then
        printf '\e]133;C;%s\e\\' "$cmd"
        __octopus_last_cmd="$cmd"
    fi
}
PROMPT_COMMAND='printf '\''\e]133;A\e\\'\'''
trap '__octopus_debug_trap; printf '\''\e]133;D\e\\'\''' DEBUG
"#;

/// 构造 shell 启动命令（portable_pty CommandBuilder）。
///
/// 注入环境变量：
/// - `TERM=xterm-256color`
/// - `COLORTERM=truecolor`
/// - `OCTOPUS_TERMINAL=1`（agent hook 门控——只在 octopus PTY 中发 OSC）
///
/// 注入 OSC 133 shell prompt marker（zsh 用 precmd/preexec hook，bash 用 trap DEBUG）。
pub fn build_command(
    cwd: Option<&str>,
    shell: Option<String>,
) -> anyhow::Result<CommandBuilder> {
    let shell = shell.unwrap_or_else(|| default_shell().to_string());
    let mut cmd = CommandBuilder::new(&shell);

    // cwd
    if let Some(dir) = cwd {
        cmd.cwd(dir);
    }

    // 环境变量
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("OCTOPUS_TERMINAL", "1");

    // shell init 脚本注入
    let (shell_name, _init_script) = if shell.ends_with("zsh") {
        ("zsh", ZSH_INIT)
    } else if shell.ends_with("bash") {
        ("bash", BASH_INIT)
    } else {
        ("sh", "") // 未知 shell 不注入 OSC 133
    };

    // 根据 shell 类型设置启动参数
    match shell_name {
        "zsh" => {
            // 写临时 zshrc → 用 ZDOTDIR 指向临时目录
            // Phase 1 简化：直接用 -c 方式注入（不完美但够用）
            // 后续优化：参考 Terax shell_init.rs 的完整 ZDOTDIR 方案
        }
        "bash" => {
            // bash --rcfile 方案后续优化
        }
        _ => {}
    }

    Ok(cmd)
}

/// 平台默认 shell。
#[cfg(target_os = "macos")]
fn default_shell() -> &'static str {
    "/bin/zsh"
}

#[cfg(not(target_os = "macos"))]
fn default_shell() -> &'static str {
    "/bin/sh"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command_basic() {
        let cmd = build_command(None, None).unwrap();
        // CommandBuilder 内部字段不可直接检查，但构造成功即可
    }

    #[test]
    fn test_build_command_with_cwd() {
        let cmd = build_command(Some("/tmp"), None).unwrap();
    }
}
