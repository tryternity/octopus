//! 菜单 + Quicklink 搜索 Provider。一次 DB 读，产出 menu/quicklink 两类 source。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::matcher::match_score;
use crate::provider::{SearchContext, SearchProvider};

pub struct MenuProvider;

#[async_trait]
impl SearchProvider for MenuProvider {
    fn id(&self) -> &'static str {
        "menu"
    }

    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "quick" | "actions")
    }

    async fn search(&self, query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        let rows = match octopus_infra::db::list_action_bar_items() {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        let mut results = search_menus(query, &rows);
        results.extend(search_quicklink_keywords(query, &rows));
        results
    }
}

fn search_menus(query: &str, rows: &[octopus_infra::db::ActionBarItem]) -> Vec<SearchResult> {
    let mut scored: Vec<(i32, SearchResult)> = rows
        .iter()
        .filter(|r| r.is_enabled && r.action_type != "submenu")
        .filter_map(|row| {
            let score = match_score(query, &row.title)?;
            let action_data = serde_json::json!({
                "action_type": row.action_type,
                "action_data": row.action_data,
                "id": row.id,
            });
            Some((score, SearchResult {
                source: if row.action_type == "url" { "quicklink" } else { "menu" }.into(),
                title: row.title.clone(),
                subtitle: row.action_type.clone(),
                icon: None,
                action_type: if row.action_type == "url" { "url" } else { "menu" }.into(),
                action_data: action_data.to_string(),
                score: 0,
            }))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .into_iter()
        .take(5)
        .map(|(s, mut r)| {
            r.score = s;
            r
        })
        .collect()
}

/// Quicklink 关键词触发：query 以 `<keyword> <rest>` 模式开头时，
/// 匹配 trigger_keyword == keyword 的 URL 类型菜单项，
/// 将 URL 模板中的 {query} / {text} 替换为 rest。
fn search_quicklink_keywords(
    query: &str,
    rows: &[octopus_infra::db::ActionBarItem],
) -> Vec<SearchResult> {
    let parts: Vec<&str> = query.splitn(2, char::is_whitespace).collect();
    if parts.len() < 2 || parts[1].trim().is_empty() {
        return Vec::new();
    }
    let keyword = parts[0];
    let rest = parts[1].trim();

    rows.iter()
        .filter(|r| r.is_enabled && r.action_type == "url" && !r.trigger_keyword.is_empty())
        .filter(|r| r.trigger_keyword == keyword)
        .map(|r| {
            let url = if r.action_data.contains("{query}") {
                r.action_data.replace("{query}", &url_encode_param(rest))
            } else if r.action_data.contains("{text}") {
                r.action_data.replace("{text}", &url_encode_param(rest))
            } else {
                r.action_data.clone()
            };
            SearchResult {
                source: "quicklink".into(),
                title: format!("{} «{}»", r.trigger_keyword, rest),
                subtitle: format!("{} → {}", r.title, url),
                icon: None,
                action_type: "url".into(),
                action_data: serde_json::json!({ "url": url, "id": r.id }).to_string(),
                score: 15000,
            }
        })
        .collect()
}

/// URL 参数编码（百分比编码），用于 Quicklink URL 模板替换。
fn url_encode_param(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => result.push_str(&format!("%{:02X}", byte)),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_param_basic() {
        assert_eq!(url_encode_param("hello"), "hello");
        assert_eq!(url_encode_param("hello world"), "hello%20world");
        assert_eq!(url_encode_param("a+b=c"), "a%2Bb%3Dc");
        assert_eq!(url_encode_param("中文"), "%E4%B8%AD%E6%96%87");
        assert_eq!(url_encode_param(""), "");
    }

    #[test]
    fn url_encode_param_safe_chars() {
        assert_eq!(url_encode_param("A-Z0-9-_.~"), "A-Z0-9-_.~");
    }

    #[test]
    fn quicklink_keyword_no_keyword_returns_empty() {
        // 单词查询（无空格）不触发关键词模式
        assert!(search_quicklink_keywords("translate", &[]).is_empty());
        assert!(search_quicklink_keywords("hello", &[]).is_empty());
    }

    #[test]
    fn quicklink_keyword_only_space_returns_empty() {
        // keyword 后只有空格不算
        assert!(search_quicklink_keywords("tr   ", &[]).is_empty());
    }
}
