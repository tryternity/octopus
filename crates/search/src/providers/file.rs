//! 文件搜索 Provider。mdfind 实时搜文件名。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::file_search::search_files;
use crate::provider::{SearchContext, SearchProvider};

pub struct FileProvider;

#[async_trait]
impl SearchProvider for FileProvider {
    fn id(&self) -> &'static str {
        "file"
    }

    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "files" | "files_bookmarks")
    }

    async fn search(&self, query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        search_files(query).await
    }
}
