//! Shell 命令 Provider：裸命令透传 + 内置补全 + 历史匹配。
//! 修复核心：query 不再强制 > 前缀，shell/quick tab 裸命令也出结果。
//! Critical #2（最终 review）：all tab 裸命令不出透传（避免污染应用结果），
//! 见 `search` 里 `ctx.tab != "all" || has_gt` 分支。

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

    async fn search(&self, query: &str, ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        // 修复核心：剥离可选 > 前缀（兼容旧习惯），裸命令也处理
        let has_gt = query.starts_with('>');
        let cmd = query.trim_start_matches('>').trim();
        if cmd.is_empty() {
            return vec![];
        }

        let mut results = vec![];

        // (1) 透传执行项（最高分，score 10000）。
        // Critical #2 修复：all tab 且无 `>` 前缀时**不出透传**——避免每个查询都出现
        // `▶ <query>` 把应用结果（score 6000）压下去（spec §0.1：用户反馈是"切到 shell
        // tab 输入裸命令"，意图是 shell/quick tab 才出命令；all tab 不该出 shell 透传项）。
        // 显式 `>` 前缀在所有 tab 都出（用户明确要执行 shell）。
        // 裸命令的补全（score 8000）+ 历史（score 6000）在所有 tab 仍保留——
        // 它们与 query 是真匹配关系，不会污染（all tab 搜 "chr" 不会命中任何 builtin/历史）。
        let show_transparent = ctx.tab != "all" || has_gt;
        if show_transparent {
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

    fn test_ctx<'a>(
        freq: &'a FrequencyScorer,
        apps: &'a RwLock<AppIndex>,
        bms: &'a RwLock<Vec<BookmarkEntry>>,
        tab: &'a str,
    ) -> SearchContext<'a> {
        SearchContext { app_index: apps, bookmarks: bms, frequency: freq, tab }
    }

    #[tokio::test]
    async fn naked_command_returns_transparent_result() {
        let p = ShellProvider::new();
        let freq = FrequencyScorer::with_test_data(Default::default());
        let apps = RwLock::new(AppIndex { apps: vec![] });
        let bms = RwLock::new(vec![]);
        // shell tab：裸命令应出透传项（spec §0.1：shell tab 输入裸命令也出结果）
        let ctx = test_ctx(&freq, &apps, &bms, "shell");
        let r = p.search("ls", &ctx).await;
        assert!(r.iter().any(|x| x.title == "▶ ls" && x.score == 10000), "shell tab 裸命令应出透传项");
    }

    #[tokio::test]
    async fn prefix_gt_is_stripped() {
        let p = ShellProvider::new();
        let freq = FrequencyScorer::with_test_data(Default::default());
        let apps = RwLock::new(AppIndex { apps: vec![] });
        let bms = RwLock::new(vec![]);
        let ctx = test_ctx(&freq, &apps, &bms, "shell");
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
        let ctx = test_ctx(&freq, &apps, &bms, "shell");
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
        let ctx = test_ctx(&freq, &apps, &bms, "shell");
        let r = p.search(">", &ctx).await;
        assert!(r.is_empty(), "> 后空应返回空");
    }

    /// Critical #2（最终 review fix）：all tab 裸命令**不出透传项**（避免 `▶ <query>`
    /// score 10000 压下应用结果 6000）。补全/历史仍可出（真匹配关系，不会污染 all tab）。
    #[tokio::test]
    async fn all_tab_naked_command_no_transparent() {
        let p = ShellProvider::new();
        let freq = FrequencyScorer::with_test_data(Default::default());
        let apps = RwLock::new(AppIndex { apps: vec![] });
        let bms = RwLock::new(vec![]);
        let ctx = test_ctx(&freq, &apps, &bms, "all");
        let r = p.search("chr", &ctx).await;
        // all tab + 裸命令（无 >）→ 不应出 score 10000 透传项
        assert!(
            !r.iter().any(|x| x.score == 10000 && x.title == "▶ chr"),
            "all tab 裸命令不应出透传项（会污染应用结果）"
        );
    }

    /// Critical #2 配套：all tab **显式 >** 仍出透传（用户明确要执行 shell，不算污染）。
    #[tokio::test]
    async fn all_tab_prefix_gt_still_transparent() {
        let p = ShellProvider::new();
        let freq = FrequencyScorer::with_test_data(Default::default());
        let apps = RwLock::new(AppIndex { apps: vec![] });
        let bms = RwLock::new(vec![]);
        let ctx = test_ctx(&freq, &apps, &bms, "all");
        let r = p.search("> ls", &ctx).await;
        assert!(
            r.iter().any(|x| x.score == 10000 && x.title == "▶ ls"),
            "all tab 显式 > 前缀仍应出透传项（用户明确要执行 shell）"
        );
    }

    /// Critical #2 配套：quick tab 裸命令仍出透传（quick 是综合快速预览，含 shell 透传合理）。
    #[tokio::test]
    async fn quick_tab_naked_command_still_transparent() {
        let p = ShellProvider::new();
        let freq = FrequencyScorer::with_test_data(Default::default());
        let apps = RwLock::new(AppIndex { apps: vec![] });
        let bms = RwLock::new(vec![]);
        let ctx = test_ctx(&freq, &apps, &bms, "quick");
        let r = p.search("ls", &ctx).await;
        assert!(
            r.iter().any(|x| x.score == 10000 && x.title == "▶ ls"),
            "quick tab 裸命令仍应出透传项"
        );
    }
}
