//! Shell 命令 Provider：裸命令透传 + 内置补全 + 历史匹配。
//! 修复核心：query 不再强制 > 前缀，shell tab 裸命令也出结果。

use async_trait::async_trait;

use crate::provider::{SearchContext, SearchProvider};
use crate::engine::SearchResult;
use crate::providers::shell_commands::BUILTIN_COMMANDS;
use crate::providers::shell_history::ShellHistoryCache;

pub struct ShellProvider {
    history: ShellHistoryCache,
}

impl ShellProvider {
    pub fn new() -> Self {
        ShellProvider { history: ShellHistoryCache::new() }
    }
}

impl Default for ShellProvider {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl SearchProvider for ShellProvider {
    fn id(&self) -> &'static str { "shell" }

    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "shell" | "quick")
    }

    fn uses_frequency(&self) -> bool { false }

    async fn search(&self, query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        // 修复核心：剥离可选 > 前缀（兼容旧习惯），裸命令也处理
        let cmd = query.trim_start_matches('>').trim();
        if cmd.is_empty() {
            return vec![];
        }

        let mut results = vec![];

        // (1) 透传执行项（原行为，最高分）
        results.push(SearchResult {
            source: "shell".into(),
            title: format!("▶ {}", cmd),
            subtitle: "Shell".into(),
            icon: None,
            action_type: "shell".into(),
            action_data: serde_json::json!({ "command": cmd }).to_string(),
            score: 10000,
        });

        // (2) 内置命令补全：cmd 是某 builtin 前缀时，列出补全（不含完全等于的）
        let mut completions = 0;
        for cmd_def in BUILTIN_COMMANDS.iter() {
            if completions >= 5 { break; }
            if cmd_def.name.starts_with(cmd) && cmd_def.name != cmd {
                results.push(SearchResult {
                    source: "shell".into(),
                    title: format!("▶ {}", cmd_def.name),
                    subtitle: cmd_def.desc.to_string(),
                    icon: None,
                    action_type: "shell".into(),
                    action_data: serde_json::json!({ "command": cmd_def.name }).to_string(),
                    score: 8000,
                });
                completions += 1;
            }
        }

        // (3) 历史匹配
        for hist_cmd in self.history.search(cmd).into_iter().take(5) {
            // 跳过与透传/补全重复的
            if results.iter().any(|r| r.action_data.contains(&format!("\"command\":\"{}\"", hist_cmd))) {
                continue;
            }
            results.push(SearchResult {
                source: "shell".into(),
                title: format!("▶ {}", hist_cmd),
                subtitle: "历史".into(),
                icon: None,
                action_type: "shell".into(),
                action_data: serde_json::json!({ "command": hist_cmd }).to_string(),
                score: 6000,
            });
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frequency::FrequencyScorer;
    use parking_lot::RwLock;
    use crate::app_index::AppIndex;
    use crate::bookmark::BookmarkEntry;

    fn test_ctx<'a>(freq: &'a FrequencyScorer, apps: &'a RwLock<AppIndex>, bms: &'a RwLock<Vec<BookmarkEntry>>) -> SearchContext<'a> {
        SearchContext { app_index: apps, bookmarks: bms, frequency: freq }
    }

    #[tokio::test]
    async fn naked_command_returns_transparent_result() {
        let p = ShellProvider::new();
        let freq = FrequencyScorer::with_test_data(Default::default());
        let apps = RwLock::new(AppIndex { apps: vec![] });
        let bms = RwLock::new(vec![]);
        let ctx = test_ctx(&freq, &apps, &bms);
        let r = p.search("ls", &ctx).await;
        assert!(r.iter().any(|x| x.title == "▶ ls" && x.score == 10000), "裸命令应出透传项");
    }

    #[tokio::test]
    async fn prefix_gt_is_stripped() {
        let p = ShellProvider::new();
        let freq = FrequencyScorer::with_test_data(Default::default());
        let apps = RwLock::new(AppIndex { apps: vec![] });
        let bms = RwLock::new(vec![]);
        let ctx = test_ctx(&freq, &apps, &bms);
        let r_gt = p.search("> ls", &ctx).await;
        let r_naked = p.search("ls", &ctx).await;
        // 两者透传项的 command 应一致
        let cmd_gt = r_gt.iter().find(|x| x.score == 10000).map(|x| x.action_data.clone());
        let cmd_naked = r_naked.iter().find(|x| x.score == 10000).map(|x| x.action_data.clone());
        assert_eq!(cmd_gt, cmd_naked, "> 前缀应被剥离");
    }

    #[tokio::test]
    async fn completion_for_partial() {
        let p = ShellProvider::new();
        let freq = FrequencyScorer::with_test_data(Default::default());
        let apps = RwLock::new(AppIndex { apps: vec![] });
        let bms = RwLock::new(vec![]);
        let ctx = test_ctx(&freq, &apps, &bms);
        let r = p.search("git", &ctx).await;
        // 应有 git status / git diff 等补全（score 8000）
        assert!(r.iter().any(|x| x.score == 8000 && x.title.contains("git status")), "应有 git status 补全");
    }

    #[tokio::test]
    async fn empty_after_strip_returns_empty() {
        let p = ShellProvider::new();
        let freq = FrequencyScorer::with_test_data(Default::default());
        let apps = RwLock::new(AppIndex { apps: vec![] });
        let bms = RwLock::new(vec![]);
        let ctx = test_ctx(&freq, &apps, &bms);
        let r = p.search(">", &ctx).await;
        assert!(r.is_empty(), "> 后空应返回空");
    }
}
