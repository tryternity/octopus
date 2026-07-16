//! 书签搜索 Provider。

use async_trait::async_trait;

use crate::bookmark::search_bookmarks;
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

    async fn search(&self, query: &str, ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        let bookmarks = ctx.bookmarks.read();
        search_bookmarks(query, &bookmarks)
    }
}
