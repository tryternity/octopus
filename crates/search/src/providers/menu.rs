//! MenuProvider：菜单项 + Quicklinks + 关键词触发。
//! Task 4 stub——search 返回空。Task 6 填真实实现（搬入 search_menus_and_quicklinks /
//! search_quicklink_keywords / url_encode_param）。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::provider::{SearchContext, SearchProvider};

pub struct MenuProvider;

#[async_trait]
impl SearchProvider for MenuProvider {
    fn id(&self) -> &'static str {
        "menu"
    }

    fn matches_tab(&self, tab: &str) -> bool {
        matches!(tab, "quick")
    }

    async fn search(&self, _query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        vec![]
    }
}
