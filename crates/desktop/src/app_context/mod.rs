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

/// 非 macOS 平台的空实现——永远返回 Err。
#[allow(dead_code)] // 仅非 macOS 平台使用
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
    #[cfg(not(target_os = "macos"))]
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

// ── 纯函数辅助 ──

/// bundle id → AppKind 映射。
pub fn classify_app(bundle_id: &str) -> AppKind {
    match bundle_id {
        "com.apple.Terminal" | "com.googlecode.iterm2" => AppKind::Terminal,
        "com.microsoft.Word"
        | "com.apple.TextEdit"
        | "com.sublimetext.4"
        | "com.sublimetext.3"
        | "com.microsoft.VSCode"
        | "com.todesktop.230313mzl4w4u92"
        | "com.github.atom" => AppKind::Editor,
        "com.apple.Safari"
        | "com.google.Chrome"
        | "org.mozilla.firefox"
        | "com.microsoft.edgemac" => AppKind::Browser,
        "com.tencent.xinWeChat"
        | "com.tinyspeck.slackmacgap"
        | "com.hnc.Discord" => AppKind::Chat,
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
    let start = range.start.min(total);
    let end = range.end.min(total);

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

/// Terminal scrollback 截断：从选区起点向前取，以 max_lines 或 max_chars 先达到者为准。
pub fn truncate_terminal_scrollback(
    scrollback: &str,
    selection_start: usize,
    max_lines: usize,
    max_chars: usize,
) -> String {
    let chars: Vec<char> = scrollback.chars().collect();
    let start = selection_start.min(chars.len());
    let before_part: String = chars[..start].iter().collect();

    let lines: Vec<&str> = before_part.lines().collect();
    let start_line = lines.len().saturating_sub(max_lines);
    let by_lines: String = lines[start_line..].join("\n");

    if by_lines.chars().count() > max_chars {
        let char_start = by_lines.chars().count().saturating_sub(max_chars);
        by_lines.chars().skip(char_start).collect()
    } else {
        by_lines
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
    fn test_truncate_terminal_by_lines() {
        // "line1\nline2\nline3\nline4\nline5\nselected"
        //  l=0...5='\n' 6-10...11='\n' ... 29='\n' 30='s' (start of "selected")
        // selection_start=30 → before_part = "line1\n...line5\n" → 5 lines → take last 2
        let scrollback = "line1\nline2\nline3\nline4\nline5\nselected";
        let result = truncate_terminal_scrollback(scrollback, 30, 2, 10000);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "line4");
        assert_eq!(lines[1], "line5");
    }

    #[test]
    fn test_truncate_terminal_by_chars() {
        let scrollback = "abcdefghijklmnopqrstuvwxyz selected";
        let result = truncate_terminal_scrollback(scrollback, 26, 10000, 5);
        assert_eq!(result.chars().count(), 5);
        assert_eq!(result, "vwxyz");
    }

    #[test]
    fn test_null_provider_returns_err() {
        let p = NullProvider;
        assert!(p.gather("test").is_err());
    }
}
