//! 应用索引：扫描 macOS /Applications/ 等目录。

use super::matcher::{match_score, Score};
use super::engine::SearchResult;

/// 应用索引入口。
pub struct AppEntry {
    pub name: String,
    pub path: String,
    /// 本地化别名（如 WeChat 的中文名"微信"），用于拼音/模糊匹配
    pub aliases: Vec<String>,
}

pub struct AppIndex {
    pub apps: Vec<AppEntry>,
}

/// 读取 .app 的本地化名称（如 WeChat 的中文名"微信"）。
/// 优先用 mdls（Spotlight 元数据，已缓存），回退到 plist。
fn read_localized_names(app_path: &std::path::Path) -> Vec<String> {
    let mut names = Vec::new();

    // 方案 1：mdls kMDItemDisplayName（Spotlight 元数据，系统已缓存，快速）
    let output = std::process::Command::new("mdls")
        .args(["-name", "kMDItemDisplayName", "-raw"])
        .arg(app_path)
        .output();
    if let Ok(o) = output {
        if o.status.success() {
            let name = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // mdls 对无元数据的 app 返回 "(null)" 或空
            if !name.is_empty() && name != "(null)" {
                let stem = app_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                // 本地化名与 file_stem 不同才有价值
                if name != stem {
                    names.push(name);
                }
            }
        }
    }

    names
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
                        let aliases = read_localized_names(&path);
                        apps.push(AppEntry {
                            name: name.to_string(),
                            path: path.to_string_lossy().to_string(),
                            aliases,
                        });
                    }
                }
            } else if path.is_dir() {
                // 普通子目录：递归（覆盖 /Applications/Adobe/Adobe Photoshop.app 等嵌套）
                Self::scan_apps_dir(&path, apps, depth + 1);
            }
        }
    }

    /// 搜索应用，返回匹配结果（已排序）。对 name + aliases 都匹配，取最高分。
    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        let mut scored: Vec<(Score, &AppEntry)> = self
            .apps
            .iter()
            .filter_map(|app| {
                // 主名匹配
                let mut best = match_score(query, &app.name);
                // 别名匹配（中文名等），取最高分
                for alias in &app.aliases {
                    if let Some(s) = match_score(query, alias) {
                        best = Some(best.map_or(s, |b| s.max(b)));
                    }
                }
                best.map(|s| (s, app))
            })
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
            AppEntry { name: "Chrome".into(), path: "/Applications/Chrome.app".into(), aliases: vec![] },
            AppEntry { name: "Safari".into(), path: "/Applications/Safari.app".into(), aliases: vec![] },
        ]};
        let results = index.search("chr");
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "Chrome");
        assert_eq!(results[0].source, "app");
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let index = AppIndex { apps: vec![
            AppEntry { name: "Chrome".into(), path: "/Applications/Chrome.app".into(), aliases: vec![] },
        ]};
        let results = index.search("");
        assert!(results.is_empty());
    }

    #[test]
    fn search_matches_alias() {
        // WeChat 英文名不匹配 wx，但别名“微信”的拼音首字母 wx 能匹配
        let index = AppIndex { apps: vec![
            AppEntry { name: "WeChat".into(), path: "/Applications/WeChat.app".into(), aliases: vec!["微信".into()] },
        ]};
        let results = index.search("wx");
        assert!(!results.is_empty(), "wx should match WeChat via alias 微信");
        assert_eq!(results[0].title, "WeChat");
    }

    #[test]
    fn search_matches_alias_by_name() {
        // 直接搜别名也能匹配
        let index = AppIndex { apps: vec![
            AppEntry { name: "WeChat".into(), path: "/Applications/WeChat.app".into(), aliases: vec!["微信".into()] },
        ]};
        let results = index.search("微信");
        assert!(!results.is_empty());
        assert_eq!(results[0].title, "WeChat");
    }
}
