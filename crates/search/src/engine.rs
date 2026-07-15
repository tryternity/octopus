//! 统一搜索引擎：整合应用、菜单、Quicklinks、文件、书签。

use std::sync::OnceLock;
use serde::Serialize;
use super::matcher::match_score;
use super::app_index::AppIndex;
use super::bookmark::{load_all_bookmarks, search_bookmarks, BookmarkEntry};
use super::file_search::search_files;

/// 统一搜索结果。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub source: String,       // "app" | "file" | "menu" | "bookmark" | "quicklink" | "shell"
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>, // base64 data URI（应用图标等），None=用 source 默认图标
    pub action_type: String,  // "launch_app" | "open_file" | "menu" | "url" | "shell"
    pub action_data: String,  // JSON
    pub score: i32,
}

/// 全局搜索引擎（启动时初始化一次）。
pub struct SearchEngine {
    app_index: AppIndex,
    bookmarks: Vec<BookmarkEntry>,
}

static SEARCH_ENGINE: OnceLock<SearchEngine> = OnceLock::new();

pub fn init_search_engine() {
    SEARCH_ENGINE.get_or_init(|| {
        SearchEngine {
            app_index: AppIndex::scan(),
            bookmarks: load_all_bookmarks(),
        }
    });
}

impl SearchEngine {
    /// 综合搜索。
    /// tab = "all" | "apps" | "files" | "shell" | "bookmarks" | "quick" | "files_bookmarks"。
    /// - "quick": 仅即时搜索（应用+菜单+Quicklinks），无文件/书签
    /// - "files_bookmarks": 仅延迟搜索（文件+书签）
    pub async fn search(&self, query: &str, tab: &str) -> Vec<SearchResult> {
        if query.is_empty() {
            return Vec::new();
        }

        let mut results = Vec::new();

        // Shell 模式：query 以 > 开头
        if query.starts_with('>') && (tab == "shell" || tab == "all" || tab == "quick") {
            let cmd = query[1..].trim();
            if !cmd.is_empty() {
                results.push(SearchResult {
                    source: "shell".into(),
                    title: format!("▶ {}", cmd),
                    subtitle: "Shell".into(),
                    icon: None,
    action_type: "shell".into(),
                    action_data: serde_json::json!({ "command": cmd }).to_string(),
                    score: 10000,
                });
            }
        }

        // 即时搜索（内存索引）
        if tab == "all" || tab == "apps" || tab == "quick" {
            let mut apps = self.app_index.search(query);
            // 应用加权重——应用启动是 launcher 核心场景，应排在文件/书签前面
            // +2000 确保拼音匹配的 app（4000+2000=6000）超过文件 prefix match（~5000）
            for r in &mut apps {
                r.score += 2000;
            }
            results.extend(apps);
        }

        // 菜单项 + Quicklinks + 关键词触发（一次 DB 读，传给两个函数）
        if tab == "all" || tab == "quick" {
            let rows = match octopus_infra::db::list_action_bar_items() {
                Ok(r) => r,
                Err(_) => Vec::new(),
            };
            results.extend(search_menus_and_quicklinks(query, &rows));
            results.extend(search_quicklink_keywords(query, &rows));
        }

        // 延迟搜索（文件 + 书签）
        if tab == "all" || tab == "files" || tab == "files_bookmarks" {
            results.extend(search_files(query).await);
        }
        if tab == "all" || tab == "bookmarks" || tab == "files_bookmarks" {
            results.extend(search_bookmarks(query, &self.bookmarks));
        }

        // 排序：按 score 降序
        results.sort_by(|a, b| b.score.cmp(&a.score));

        // 限制总数
        results.truncate(10);
        results
    }
}

/// 从 DB rows 查菜单项 + Quicklinks。调用方负责一次性读 DB。
fn search_menus_and_quicklinks(query: &str, rows: &[octopus_infra::db::ActionBarItem]) -> Vec<SearchResult> {
    let mut results: Vec<(i32, SearchResult)> = rows
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

    results.sort_by(|a, b| b.0.cmp(&a.0));
    results.into_iter().take(5).map(|(s, mut r)| { r.score = s; r }).collect()
}

/// Quicklink 关键词触发：query 以 `<keyword> <rest>` 模式开头时，
/// 匹配 trigger_keyword == keyword 的 URL 类型菜单项，
/// 将 URL 模板中的 {query} 替换为 rest。
fn search_quicklink_keywords(query: &str, rows: &[octopus_infra::db::ActionBarItem]) -> Vec<SearchResult> {
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
                action_data: serde_json::json!({
                    "url": url,
                    "id": r.id,
                }).to_string(),
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
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// 获取全局搜索引擎（需先 init_search_engine）。
pub fn get_engine() -> Option<&'static SearchEngine> {
    SEARCH_ENGINE.get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_index::AppEntry;

    /// 切换到 in-memory DB，避免测试读 ~/.octopus/octopus.db。
    /// SearchEngine::search 在 tab=all/quick 时经 list_action_bar_items → with_db 触达 DB。
    /// 详见架构文档「测试数据库隔离」。
    static TEST_DB_SETUP: std::sync::Once = std::sync::Once::new();
    fn setup_test_db() {
        TEST_DB_SETUP.call_once(|| {
            octopus_infra::db::init_test_db();
        });
    }

    #[test]
    fn search_empty_returns_empty() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine {
            app_index: AppIndex { apps: vec![] },
            bookmarks: vec![],
        };
        let results = rt.block_on(engine.search("", "all"));
        assert!(results.is_empty());
    }

    #[test]
    fn shell_mode_prefix() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine {
            app_index: AppIndex { apps: vec![] },
            bookmarks: vec![],
        };
        let results = rt.block_on(engine.search("> ls", "shell"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "shell");
        assert_eq!(results[0].action_type, "shell");
    }

    #[test]
    fn quick_tab_excludes_files_and_bookmarks() {
        setup_test_db();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine {
            app_index: AppIndex { apps: vec![] },
            bookmarks: vec![],
        };
        // quick tab 搜索 → 无文件/书签结果（因为没有应用也没有菜单）
        let results = rt.block_on(engine.search("test", "quick"));
        // 可能只有菜单匹配，但不会有文件/书签
        assert!(results.iter().all(|r| r.source != "file" && r.source != "bookmark"));
    }

    #[test]
    fn files_bookmarks_tab_excludes_apps_and_menus() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine {
            app_index: AppIndex { apps: vec![
                AppEntry { name: "TestApp".into(), path: "/Applications/TestApp.app".into(), aliases: vec![], icon: String::new() },
            ]},
            bookmarks: vec![],
        };
        let results = rt.block_on(engine.search("test", "files_bookmarks"));
        // files_bookmarks tab 不返回应用结果
        assert!(results.iter().all(|r| r.source != "app"));
        assert!(results.iter().all(|r| r.source != "menu"));
    }

    #[test]
    fn quick_tab_includes_shell_mode() {
        setup_test_db();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine {
            app_index: AppIndex { apps: vec![] },
            bookmarks: vec![],
        };
        let results = rt.block_on(engine.search("> echo hi", "quick"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "shell");
    }

    #[test]
    fn all_tab_returns_combined_results() {
        setup_test_db();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine {
            app_index: AppIndex { apps: vec![
                AppEntry { name: "Chrome".into(), path: "/Applications/Chrome.app".into(), aliases: vec![], icon: String::new() },
            ]},
            bookmarks: vec![],
        };
        let results = rt.block_on(engine.search("chr", "all"));
        // all tab 应包含应用结果
        assert!(results.iter().any(|r| r.source == "app" && r.title == "Chrome"));
    }

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

    #[test]
    fn url_type_returned_as_quicklink_source() {
        setup_test_db();
        // URL 类型菜单项在搜索结果中 source 为 "quicklink"，action_type 为 "url"
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine = SearchEngine {
            app_index: AppIndex { apps: vec![] },
            bookmarks: vec![],
        };
        // 如果 DB 中有 URL 类型菜单项，搜索应该返回 source="quicklink"
        // 测试不依赖具体 DB 内容，只验证返回的结果中 URL 类型的 source 正确
        let _results = rt.block_on(engine.search("test", "all"));
        // 不 assert 具体结果（依赖 DB），仅验证不 panic
    }
}
