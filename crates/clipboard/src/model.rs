use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    #[default]
    Text,
    Voice,
    Ocr,
    Image,
    File,
}

impl ItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemType::Text => "text",
            ItemType::Voice => "voice",
            ItemType::Ocr => "ocr",
            ItemType::Image => "image",
            ItemType::File => "file",
        }
    }

    #[allow(clippy::should_implement_trait)] // 返回 Self 而非 Result，语义不同于 FromStr
    pub fn from_str(s: &str) -> Self {
        match s {
            "voice" => ItemType::Voice,
            "ocr" => ItemType::Ocr,
            "image" => ItemType::Image,
            "file" => ItemType::File,
            _ => ItemType::Text,
        }
    }
}

/// JSON 元数据，按 item_type 不同 schema（见 spec §2.3）。
/// 存 DB 时序列化为 JSON 字符串存 meta_info 列；读 DB 时反序列化。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MetaInfo {
    // image
    #[serde(skip_serializing_if = "Option::is_none")]
    pub w: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    // voice / ocr
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub char_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asr_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polish_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polished: Option<bool>,
    // file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub size: Option<String>,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardItem {
    pub id: i64,
    pub item_type: ItemType,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta_info: Option<MetaInfo>,
    pub is_favorite: bool,
    pub is_rich: bool,
    pub created_at: String,
    pub has_thumbnail: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments: Option<String>,
    /// 软删标记（false=活跃；true=已进回收站）。图片始终 false（不软删）。
    /// 与 vault_ciphers/folders 的 is_deleted 字段一致（DB 列 INTEGER 0/1）。
    pub is_deleted: bool,
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
        for t in [ItemType::Text, ItemType::Voice, ItemType::Ocr, ItemType::Image, ItemType::File] {
            let s = t.as_str();
            assert_eq!(ItemType::from_str(s), t);
        }
    }

    #[test]
    fn test_item_type_default() {
        assert_eq!(ItemType::default(), ItemType::Text);
    }
}
