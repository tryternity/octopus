//! 浏览器书签解析：Safari / Chrome / Edge。

use serde::Serialize;
use super::matcher::{match_score, Score};
use super::engine::SearchResult;

pub struct BookmarkEntry {
    pub title: String,
    pub url: String,
    pub browser: String,
}

/// 加载所有浏览器的书签。
pub fn load_all_bookmarks() -> Vec<BookmarkEntry> {
    let mut bookmarks = Vec::new();
    // Chrome / Edge（JSON 格式，无需 Full Disk Access）
    for (browser, path) in &[
        ("Chrome", "Library/Application Support/Google/Chrome/Default/Bookmarks"),
        ("Edge", "Library/Application Support/Microsoft Edge/Default/Bookmarks"),
    ] {
        if let Some(home) = dirs::home_dir() {
            let full_path = home.join(path);
            if full_path.exists() {
                bookmarks.extend(load_chromium_bookmarks(browser, &full_path));
            }
        }
    }
    // Safari（plist，需 Full Disk Access）—— 尝试读，失败跳过
    if let Some(home) = dirs::home_dir() {
        let safari_path = home.join("Library/Safari/Bookmarks.plist");
        if safari_path.exists() {
            bookmarks.extend(load_safari_bookmarks(&safari_path));
        }
    }
    log::info!("[search] 书签索引: {} 条", bookmarks.len());
    bookmarks
}

/// 解析 Chrome/Edge 书签 JSON。
fn load_chromium_bookmarks(browser: &str, path: &std::path::Path) -> Vec<BookmarkEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let root: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut result = Vec::new();
    // 递归遍历 children 数组
    fn walk(node: &serde_json::Value, browser: &str, out: &mut Vec<BookmarkEntry>) {
        if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
            for child in children {
                if child.get("type").and_then(|t| t.as_str()) == Some("url") {
                    let title = child.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let url = child.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
                    if !title.is_empty() && !url.is_empty() {
                        out.push(BookmarkEntry { title, url, browser: browser.into() });
                    }
                }
                walk(child, browser, out);
            }
        }
    }
    let roots = root.get("roots");
    if let Some(roots) = roots {
        walk(roots, browser, &mut result);
    }
    result
}

/// 解析 Safari 书签 plist（简化：用 plist crate 或跳过）。
fn load_safari_bookmarks(_path: &std::path::Path) -> Vec<BookmarkEntry> {
    // Safari plist 解析需要额外依赖（plist crate），暂跳过
    Vec::new()
}

/// 搜索书签。
pub fn search_bookmarks(query: &str, bookmarks: &[BookmarkEntry]) -> Vec<SearchResult> {
    let mut scored: Vec<(Score, SearchResult)> = bookmarks
        .iter()
        .filter_map(|bm| {
            let score = match_score(query, &bm.title)
                .or_else(|| match_score(query, &bm.url))?;
            Some((score, SearchResult {
                source: "bookmark".into(),
                title: bm.title.clone(),
                subtitle: format!("[{}] {}", bm.browser, bm.url),
                action_type: "url".into(),
                action_data: serde_json::json!({ "url": bm.url }).to_string(),
                score: 0,
            }))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(10).map(|(s, mut r)| { r.score = s; r }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_bookmarks_matches_title() {
        let bookmarks = vec![
            BookmarkEntry { title: "GitHub".into(), url: "https://github.com".into(), browser: "Chrome".into() },
            BookmarkEntry { title: "Google".into(), url: "https://google.com".into(), browser: "Chrome".into() },
        ];
        let results = search_bookmarks("git", &bookmarks);
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "GitHub");
    }

    #[test]
    fn search_bookmarks_empty_query_returns_empty() {
        let bookmarks = vec![
            BookmarkEntry { title: "GitHub".into(), url: "https://github.com".into(), browser: "Chrome".into() },
        ];
        let results = search_bookmarks("", &bookmarks);
        assert!(results.is_empty());
    }
}
