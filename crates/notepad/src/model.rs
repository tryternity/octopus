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

/// 一条笔记（DB notes 表一行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub title: Option<String>,
    pub content_html: String,
    pub content_text: String,
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
}
