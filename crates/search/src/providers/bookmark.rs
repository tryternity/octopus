//! BookmarkProvider：书签搜索（来自内存 bookmarks 列表）。
//! Task 4 stub——search 返回空。Task 5 填真实实现。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::provider::{SearchContext, SearchProvider};

pub struct BookmarkProvider;

#[async_trait]
impl SearchProvider for BookmarkProvider {
    fn id(&self) -> &'static str {
        "bookmark"
    }

    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "bookmarks" | "files_bookmarks")
    }

    async fn search(&self, _query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        vec![]
    }
}
