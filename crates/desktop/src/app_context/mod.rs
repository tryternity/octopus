//! Action Bar 应用上下文获取——平台无关的类型定义、trait、纯函数辅助。
//!
//! 各 OS 的实现在子模块（macos_ax.rs 等），通过 `provider()` 工厂 + cfg 分发。

/// 应用语义类别。决定前端/LLM 如何利用上下文。
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    Editor,
    Terminal,
    Browser,
    Chat,
    Unknown,
}

/// 选中文本所在的应用。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    pub name: String,
    pub kind: AppKind,
}

/// 选中文本的周围文本。
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurroundingText {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
}

/// gather 采集到的额外上下文。
#[derive(Clone)]
pub struct ExtraContext {
    pub source: AppSource,
    pub surrounding: Option<SurroundingText>,
    /// AX 诊断信息（各步成功/失败 + range + full_text 预览），写入日志方便排查。
    pub diagnostics: Option<String>,
}

/// 平台无关的应用上下文获取接口。
pub trait ContextProvider {
    /// 至少返回 source（前台 app 信息）；surrounding 可能 None。
    /// selected_text 用于校验 AX 树是否包含真实编辑器内容。
    fn gather(&self, selected_text: &str) -> anyhow::Result<ExtraContext>;
}

/// 非 macOS/Windows/Linux 平台的空实现——永远返回 Err。
#[allow(dead_code)] // 仅非主流平台使用
pub struct NullProvider;

impl ContextProvider for NullProvider {
    fn gather(&self, _selected_text: &str) -> anyhow::Result<ExtraContext> {
        Err(anyhow::anyhow!("app context: platform not supported"))
    }
}

/// 工厂函数——cfg 分发到各平台实现。
pub fn provider() -> Box<dyn ContextProvider> {
    #[cfg(target_os = "macos")]
    {
        Box::new(self::macos_ax::AxProvider)
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(self::windows_uia::UiaProvider)
    }
    // Linux 暂不支持（AT-SPI2 需事件流，见 linux_atspi.rs 注释）
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Box::new(NullProvider)
    }
}

/// 工厂便捷方法：调 provider().gather()。
pub fn gather_context(selected_text: &str) -> anyhow::Result<ExtraContext> {
    provider().gather(selected_text)
}

#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod macos_ax;
#[cfg(target_os = "macos")]
pub mod sublime_plugin;
#[cfg(target_os = "windows")]
mod windows_uia;
// linux_atspi.rs 暂不编译——AT-SPI2 需事件流，见文件注释

// ── 纯函数辅助 ──

/// 带截止时间执行子进程——超时 kill 子进程，防永久挂起（权限对话框/无响应进程）。
///
/// gather_context 的 fallback 链（Pages osascript / lsof / pdftotext / officecli / mdfind / subl）
/// 原各自用 `.output()` 无超时同步阻塞，子进程挂起会永久卡死 trigger worker 线程，进而
/// 卡死 TRIGGER_IN_PROGRESS 守卫。本函数统一收口：spawn 后轮询到 deadline，超时 kill + wait
/// 回收，保证 gather 总耗时受 `AX_TIMEOUT`（500ms）约束。
///
/// 范式同 `action_bar_commands::wait_with_timeout_secs`，但接收 `Instant` deadline 而非固定秒数，
/// 以融入 gather 的整体预算语义。返回 `None` 表示超时或 spawn 失败。
pub(crate) fn run_command_with_deadline(
    mut cmd: std::process::Command,
    deadline: std::time::Instant,
) -> Option<std::process::Output> {
    use std::process::Stdio;

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return None,
    };

    // 轮询到 deadline——500ms 内每 50ms 检查一次
    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(), // 已退出，一次性拿 output
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
    // 超时仍未退出 → kill + wait 回收，防孤儿进程
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// bundle_id / 进程名 → AppKind 映射（三平台统一）。
/// 内部统一转小写比较——Windows 文件系统不区分大小写（WINWORD.EXE = winword.exe），
/// macOS bundle_id 本就小写，无副作用。
pub fn classify_app(id: &str) -> AppKind {
    let id = id.to_ascii_lowercase();
    match id.as_str() {
        // ── Terminal ──
        "com.apple.terminal" | "com.googlecode.iterm2" => AppKind::Terminal,
        #[cfg(target_os = "windows")]
        "cmd.exe" | "powershell.exe" | "pwsh.exe" | "windowsterminal.exe" | "conhost.exe" => AppKind::Terminal,
        "gnome-terminal" | "gnome-terminal-server" | "konsole" | "xterm" | "alacritty"
        | "kitty" | "terminator" | "tilix" | "foot" | "wezterm-gui" => AppKind::Terminal,
        // ── Editor ──
        "com.microsoft.word" | "com.apple.textedit"
        | "com.sublimetext.4" | "com.sublimetext.3"
        | "com.microsoft.vscode" | "com.todesktop.230313mzl4w4u92"
        | "com.github.atom" | "com.kingsoft.wpsoffice.mac"
        | "com.apple.iwork.pages" | "com.apple.iwork.keynote"
        | "com.apple.iwork.numbers" => AppKind::Editor,
        #[cfg(target_os = "windows")]
        "notepad.exe" | "winword.exe" | "excel.exe" | "powerpnt.exe"
        | "code.exe" | "sublime_text.exe" | "wps.exe" | "notepad++.exe" => AppKind::Editor,
        #[cfg(target_os = "linux")]
        "gedit" | "code" | "sublime_text" | "kate" | "nano"
        | "vim" | "gvim" | "emacs" | "wps" => AppKind::Editor,
        // ── Browser ──
        "com.apple.safari" | "com.google.chrome"
        | "org.mozilla.firefox" | "org.mozilla.firefoxdeveloperedition"
        | "org.mozilla.firefox.nightly" | "com.microsoft.edgemac" => AppKind::Browser,
        #[cfg(target_os = "windows")]
        "chrome.exe" | "msedge.exe" | "firefox.exe" => AppKind::Browser,
        #[cfg(target_os = "linux")]
        "firefox" | "chromium" | "google-chrome" | "brave" | "microsoft-edge" => AppKind::Browser,
        // ── Chat ──
        "com.tencent.xinweichat" | "com.tinyspeck.slackmacgap"
        | "com.hnc.discord" => AppKind::Chat,
        #[cfg(target_os = "windows")]
        "wechat.exe" | "slack.exe" | "discord.exe" => AppKind::Chat,
        #[cfg(target_os = "linux")]
        "slack" | "discord" => AppKind::Chat,
        _ => AppKind::Unknown,
    }
}

/// 选区范围（start..end，字符偏移）。
#[derive(Clone, Copy, Debug)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

/// 从全文和选区范围切出 before/after，各裁剪到 limit 字。
pub fn extract_surrounding(full_text: &str, range: TextRange, limit: usize) -> SurroundingText {
    let chars: Vec<char> = full_text.chars().collect();
    let total = chars.len();
    // 归一化：确保 start <= end（AX 可能返回反向选区或负 length）
    let start = range.start.min(total);
    let end = range.end.max(start).min(total);

    let before_start = start.saturating_sub(limit);
    let before: String = chars[before_start..start].iter().collect();
    let after_end = (end + limit).min(total);
    let after: String = chars[end..after_end].iter().collect();

    SurroundingText {
        before: if before.is_empty() { None } else { Some(before) },
        after: if after.is_empty() { None } else { Some(after) },
        window_title: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_terminal() {
        assert_eq!(classify_app("com.apple.Terminal"), AppKind::Terminal);
        assert_eq!(classify_app("com.googlecode.iterm2"), AppKind::Terminal);
    }

    #[test]
    fn test_classify_editor() {
        assert_eq!(classify_app("com.microsoft.Word"), AppKind::Editor);
        assert_eq!(classify_app("com.apple.TextEdit"), AppKind::Editor);
        assert_eq!(classify_app("com.microsoft.VSCode"), AppKind::Editor);
        assert_eq!(classify_app("com.kingsoft.wpsoffice.mac"), AppKind::Editor);
    }

    #[test]
    fn test_classify_unknown() {
        assert_eq!(classify_app("com.some.unknown.app"), AppKind::Unknown);
        assert_eq!(classify_app(""), AppKind::Unknown);
    }

    #[test]
    fn test_extract_surrounding_normal() {
        // "Hello world this is a test sentence"
        //  H=0 e=1 l=2 l=3 o=4 ' '=5 w=6 o=7 r=8 l=9 d=10 ' '=11
        // range start=6 end=11 → "world"
        // before: 5 chars before pos 6 → chars[1..6] = "ello "
        // after: 5 chars after pos 11 → chars[11..16] = " this"
        let full = "Hello world this is a test sentence";
        let range = TextRange { start: 6, end: 11 };
        let s = extract_surrounding(full, range, 5);
        assert_eq!(s.before.as_deref(), Some("ello "));
        assert_eq!(s.after.as_deref(), Some(" this"));
    }

    #[test]
    fn test_extract_surrounding_start_of_text() {
        let full = "Hello world";
        let range = TextRange { start: 0, end: 5 };
        let s = extract_surrounding(full, range, 100);
        assert_eq!(s.before, None);
        assert_eq!(s.after.as_deref(), Some(" world"));
    }

    #[test]
    fn test_extract_surrounding_cjk() {
        let full = "你好世界这是一段测试文字";
        let range = TextRange { start: 4, end: 6 };
        let s = extract_surrounding(full, range, 2);
        assert_eq!(s.before.as_deref(), Some("世界"));
        assert_eq!(s.after.as_deref(), Some("一段"));
    }

    #[test]
    fn test_null_provider_returns_err() {
        let p = NullProvider;
        assert!(p.gather("test").is_err());
    }

    /// 回归 M2：超时时 kill 子进程并返回 None，不永久阻塞。
    #[test]
    fn run_command_with_deadline_kills_on_timeout() {
        // sleep 10 命令远超 deadline——应在 deadline 内返回 None 且子进程被 kill
        let start = std::time::Instant::now();
        let deadline = start + std::time::Duration::from_millis(200);
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("10");
        let result = run_command_with_deadline(cmd, deadline);
        assert!(result.is_none(), "超时应返回 None");
        // 应在远小于 10s 内返回（实际 ~200ms + kill 开销）
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "不应阻塞到 sleep 自然结束");
    }

    /// 回归 M2：正常命令返回 output。
    #[test]
    fn run_command_with_deadline_returns_output_on_success() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut cmd = std::process::Command::new("echo");
        cmd.arg("hello");
        let result = run_command_with_deadline(cmd, deadline);
        assert!(result.is_some(), "echo 应成功返回 output");
        let output = result.unwrap();
        assert!(output.status.success(), "echo 退出码应为 0");
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"), "stdout 应含 hello");
    }
}
