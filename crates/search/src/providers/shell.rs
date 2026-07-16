//! ShellProvider：Shell 模式（query 以 `>` 开头）。
//! Task 4 stub——search 返回空。Task 7 填真实实现（依赖 shell_commands.rs / shell_history.rs）。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::provider::{SearchContext, SearchProvider};

pub struct ShellProvider;

impl ShellProvider {
    pub fn new() -> Self {
        ShellProvider
    }
}

impl Default for ShellProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SearchProvider for ShellProvider {
    fn id(&self) -> &'static str {
        "shell"
    }

    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "shell" | "quick")
    }

    async fn search(&self, _query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        vec![]
    }
}
