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
    /// 综合搜索。tab = "all" | "apps" | "files" | "shell" | "bookmarks"。
    pub async fn search(&self, query: &str, tab: &str) -> Vec<SearchResult> {
        if query.is_empty() {
            return Vec::new();
        }

        let mut results = Vec::new();

        // Shell 模式：query 以 > 开头
        if query.starts_with('>') && (tab == "shell" || tab == "all") {
            let cmd = query[1..].trim();
            if !cmd.is_empty() {
                results.push(SearchResult {
                    source: "shell".into(),
                    title: format!("▶ {}", cmd),
                    subtitle: "运行 Shell 命令".into(),
                    action_type: "shell".into(),
                    action_data: serde_json::json!({ "command": cmd }).to_string(),
                    score: 10000,
                });
            }
        }

        // 即时搜索（内存索引）
        if tab == "all" || tab == "apps" {
            let mut apps = self.app_index.search(query);
            // 应用加权重（source 优先级）
            for r in &mut apps {
                r.score += 100;
            }
            results.extend(apps);
        }

        // 菜单项 + Quicklinks（从 DB 查）
        if tab == "all" {
            results.extend(search_menus_and_quicklinks(query));
        }

        // 延迟搜索（文件 + 书签）
        if tab == "all" || tab == "files" {
            results.extend(search_files(query).await);
        }
        if tab == "all" || tab == "bookmarks" {
            results.extend(search_bookmarks(query, &self.bookmarks));
        }

        // 排序：按 score 降序
        results.sort_by(|a, b| b.score.cmp(&a.score));

        // 限制总数
        results.truncate(10);
        results
    }
}

/// 从 DB 查菜单项 + Quicklinks。
fn search_menus_and_quicklinks(query: &str) -> Vec<SearchResult> {
    let rows = match octopus_infra::db::list_action_bar_items() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut results: Vec<(i32, SearchResult)> = rows
        .iter()
        .filter(|r| r.is_enabled && r.action_type != "submenu")
        .filter_map(|row| {
            let score = match_score(query, &row.title)?;
            let action_data = if row.action_type == "url" && !row.action_data.is_empty() {
                serde_json::json!({
                    "action_type": row.action_type,
                    "action_data": row.action_data,
                    "id": row.id,
                })
            } else {
                serde_json::json!({
                    "action_type": row.action_type,
                    "action_data": row.action_data,
                    "id": row.id,
                })
            };
            Some((score, SearchResult {
                source: "menu".into(),
                title: row.title.clone(),
                subtitle: row.action_type.clone(),
                action_type: "menu".into(),
                action_data: action_data.to_string(),
                score: 0,
            }))
        })
        .collect();

    results.sort_by(|a, b| b.0.cmp(&a.0));
    results.into_iter().take(5).map(|(s, mut r)| { r.score = s; r }).collect()
}

/// 获取全局搜索引擎（需先 init_search_engine）。
pub fn get_engine() -> Option<&'static SearchEngine> {
    SEARCH_ENGINE.get()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
