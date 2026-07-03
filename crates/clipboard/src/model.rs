use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    #[default]
    Text,
    Image,
    File,
}

impl ItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemType::Text => "text",
            ItemType::Image => "image",
            ItemType::File => "file",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "image" => ItemType::Image,
            "file" => ItemType::File,
            _ => ItemType::Text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Clipboard,
    Asr,
    Ocr,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Clipboard => "clipboard",
            Source::Asr => "asr",
            Source::Ocr => "ocr",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "asr" => Source::Asr,
            "ocr" => Source::Ocr,
            _ => Source::Clipboard,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMeta {
    pub blob_hash: String,
    pub width: u32,
    pub height: u32,
    pub has_thumbnail: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub file_count: usize,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrMeta {
    pub transcription_id: i64,
    pub polish_status: String,
    pub engine: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OcrMeta {
    pub engine: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub id: i64,
    pub item_type: ItemType,
    pub source: Source,
    pub content: String,
    pub is_favorite: bool,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_meta: Option<ImageMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_meta: Option<FileMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asr_meta: Option<AsrMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_meta: Option<OcrMeta>,
    pub is_rich: bool,
}

/// 查询过滤条件
#[derive(Debug, Clone, Default)]
pub struct QueryFilter {
    pub filter: String,
    pub search: Option<String>,
    pub page: u32,
    pub size: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_item_type_roundtrip() {
        for t in [ItemType::Text, ItemType::Image, ItemType::File] {
            let s = t.as_str();
            assert_eq!(ItemType::from_str(s), t);
        }
    }

    #[test]
    fn test_source_roundtrip() {
        for s in [Source::Clipboard, Source::Asr, Source::Ocr] {
            let str = s.as_str();
            assert_eq!(Source::from_str(str), s);
        }
    }

    #[test]
    fn test_source_ocr_unknown_fallback() {
        assert_eq!(Source::from_str("xxx"), Source::Clipboard);
    }

    #[test]
    fn test_item_type_default() {
        assert_eq!(ItemType::default(), ItemType::Text);
    }

    #[test]
    fn test_source_default() {
        assert_eq!(Source::default(), Source::Clipboard);
    }
}
