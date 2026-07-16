//! 应用搜索 Provider。从内存 app_index 搜索，+2000 权重。

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

    async fn search(&self, query: &str, ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        let mut apps = ctx.app_index.read().search(query);
        // 应用加权重——launcher 核心场景，应排在文件/书签前
        for r in &mut apps {
            r.score += 2000;
        }
        apps
    }
}
