//! 应用索引：扫描 macOS /Applications/ 等目录。

use super::matcher::{match_score, Score};
use super::engine::SearchResult;

/// 应用索引入口。
pub struct AppEntry {
    pub name: String,
    pub path: String,
}

pub struct AppIndex {
    pub apps: Vec<AppEntry>,
}

impl AppIndex {
    /// 扫描 macOS 应用目录。
    pub fn scan() -> Self {
        let mut apps = Vec::new();
        for dir in &[
            "/Applications",
            "/System/Applications",
            "/Applications/Utilities",
        ] {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("app") {
                        let name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string();
                        if !name.is_empty() {
                            apps.push(AppEntry {
                                name,
                                path: path.to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
        }
        // 也扫 ~/Applications
        if let Some(home) = dirs::home_dir() {
            let user_apps = home.join("Applications");
            if let Ok(entries) = std::fs::read_dir(&user_apps) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("app") {
                        let name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string();
                        if !name.is_empty() {
                            apps.push(AppEntry {
                                name,
                                path: path.to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
        }
        log::info!("[search] 应用索引: {} 个应用", apps.len());
        Self { apps }
    }

    /// 搜索应用，返回匹配结果（已排序）。
    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        let mut scored: Vec<(Score, &AppEntry)> = self
            .apps
            .iter()
            .filter_map(|app| match_score(query, &app.name).map(|s| (s, app)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored
            .into_iter()
            .take(10)
            .map(|(score, app)| SearchResult {
                source: "app".into(),
                title: app.name.clone(),
                subtitle: app.path.clone(),
                action_type: "launch_app".into(),
                action_data: serde_json::json!({ "path": app.path }).to_string(),
                score,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_matching_apps() {
        let index = AppIndex { apps: vec![
            AppEntry { name: "Chrome".into(), path: "/Applications/Chrome.app".into() },
            AppEntry { name: "Safari".into(), path: "/Applications/Safari.app".into() },
        ]};
        let results = index.search("chr");
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "Chrome");
        assert_eq!(results[0].source, "app");
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let index = AppIndex { apps: vec![
            AppEntry { name: "Chrome".into(), path: "/Applications/Chrome.app".into() },
        ]};
        let results = index.search("");
        assert!(results.is_empty());
    }
}
