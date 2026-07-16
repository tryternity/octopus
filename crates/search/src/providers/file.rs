//! FileProvider：文件搜索（walk 文件系统）。
//! Task 4 stub——search 返回空。Task 8 填真实实现。

use async_trait::async_trait;

use crate::engine::SearchResult;
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

    async fn search(&self, _query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        vec![]
    }
}
