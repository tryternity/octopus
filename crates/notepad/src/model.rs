use serde::{Deserialize, Serialize};

/// 笔记来源（决定徽标 + 溯源回溯目标）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoteSource {
    Asr,
    Ocr,
    Clipboard,
    #[default]
    Manual,
}

impl NoteSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteSource::Asr => "asr",
            NoteSource::Ocr => "ocr",
            NoteSource::Clipboard => "clipboard",
            NoteSource::Manual => "manual",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "asr" => NoteSource::Asr,
            "ocr" => NoteSource::Ocr,
            "clipboard" => NoteSource::Clipboard,
            _ => NoteSource::Manual,
        }
    }
}

/// 笔记内容格式（DB `notes.type` 列）。
/// - `Html`：TipTap 富文本（content_html 存原始，content_text 存抽取纯文本）。
/// - `Text`：纯文本（content_text 存原文，content_html 空）。
/// - `Markdown`：md 源码（content_text 存源码，content_html 空，预览端渲染）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoteType {
    #[default]
    Html,
    Text,
    Markdown,
}

impl NoteType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteType::Html => "html",
            NoteType::Text => "text",
            NoteType::Markdown => "markdown",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "text" => NoteType::Text,
            "markdown" => NoteType::Markdown,
            // "html" 及未知值 → Html（历史数据 DEFAULT 'html'，容错偏富文本）
            _ => NoteType::Html,
        }
    }
}

/// 一条笔记（DB notes 表一行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub title: Option<String>,
    pub content_html: String,
    pub content_text: String,
    pub note_type: NoteType,
    pub source: NoteSource,
    pub source_ref_id: Option<i64>,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// 列表查询过滤 + 分页。
#[derive(Debug, Clone, Default)]
pub struct NoteFilter {
    pub source: Option<NoteSource>,
    pub favorite: bool,
    pub pinned: bool,
    /// None 或 <3 字符 → LIKE 子串；≥3 字符 → FTS5 phrase MATCH
    pub search: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_source_roundtrip() {
        for s in [NoteSource::Asr, NoteSource::Ocr, NoteSource::Clipboard, NoteSource::Manual] {
            assert_eq!(NoteSource::from_str(s.as_str()), s);
        }
    }

    #[test]
    fn note_source_from_unknown_defaults_manual() {
        assert_eq!(NoteSource::from_str("???"), NoteSource::Manual);
    }

    #[test]
    fn note_source_default_is_manual() {
        assert_eq!(NoteSource::default(), NoteSource::Manual);
    }

    #[test]
    fn note_type_roundtrip() {
        for t in [NoteType::Html, NoteType::Text, NoteType::Markdown] {
            assert_eq!(NoteType::from_str(t.as_str()), t);
        }
    }

    #[test]
    fn note_type_from_unknown_defaults_html() {
        // 未知值 → Html（保守：历史/异常值保持富文本不丢格式）
        assert_eq!(NoteType::from_str("???"), NoteType::Html);
        assert_eq!(NoteType::from_str(""), NoteType::Html);
    }

    #[test]
    fn note_type_default_is_html() {
        assert_eq!(NoteType::default(), NoteType::Html);
    }
}
