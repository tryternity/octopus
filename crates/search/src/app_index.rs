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
    /// 扫描 macOS 应用目录（递归子目录，覆盖 Adobe / JetBrains Toolbox 等嵌套 .app）。
    pub fn scan() -> Self {
        let mut apps = Vec::new();
        for dir in &[
            "/Applications",
            "/System/Applications",
            "/Applications/Utilities",
        ] {
            Self::scan_apps_dir(std::path::Path::new(dir), &mut apps, 0);
        }
        // 也扫 ~/Applications
        if let Some(home) = dirs::home_dir() {
            Self::scan_apps_dir(&home.join("Applications"), &mut apps, 0);
        }
        log::info!("[search] 应用索引: {} 个应用", apps.len());
        // 去重：跨目录同名 app 只保留第一个（/Applications 优先于 ~/Applications）
        let mut seen = std::collections::HashSet::new();
        apps.retain(|a| seen.insert(a.name.clone()));
        Self { apps }
    }

    /// 递归扫描目录下的 .app（深度受限，不进入 .app 包内部）。
    fn scan_apps_dir(dir: &std::path::Path, apps: &mut Vec<AppEntry>, depth: u32) {
        const MAX_DEPTH: u32 = 2;
        if depth > MAX_DEPTH {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("app") {
                // .app 是 bundle（叶子），加入但不递归进包内部
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    if !name.is_empty() {
                        apps.push(AppEntry {
                            name: name.to_string(),
                            path: path.to_string_lossy().to_string(),
                        });
                    }
                }
            } else if path.is_dir() {
                // 普通子目录：递归（覆盖 /Applications/Adobe/Adobe Photoshop.app 等嵌套）
                Self::scan_apps_dir(&path, apps, depth + 1);
            }
        }
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
