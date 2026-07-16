//! AppProvider：应用搜索（来自 AppIndex 内存索引）。
//! Task 4 stub——search 返回空。Task 5 填真实实现（match_score + app 权重 +2000）。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::provider::{SearchContext, SearchProvider};

pub struct AppProvider;

#[async_trait]
impl SearchProvider for AppProvider {
    fn id(&self) -> &'static str {
        "app"
    }

    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "apps" | "quick")
    }

    async fn search(&self, _query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        vec![]
    }
}
