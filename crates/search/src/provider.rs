//! Provider trait + 共享搜索上下文。

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::app_index::AppIndex;
use crate::bookmark::BookmarkEntry;
use crate::engine::SearchResult;
use crate::frequency::FrequencyScorer;

/// 各 Provider 共享的只读上下文。
/// 注意：含 `RwLock` 引用——生命周期内嵌于单次 search_streaming 调用，
/// 不跨 tokio::spawn（用 FuturesUnordered 在单 task 内并发，无需 Arc）。
pub struct SearchContext<'a> {
    pub app_index: &'a RwLock<AppIndex>,
    pub bookmarks: &'a RwLock<Vec<BookmarkEntry>>,
    pub frequency: &'a FrequencyScorer,
}

/// 搜索 Provider 契约。
///
/// **关键不变量**：`search` 绝不返回 Err——失败时返回空 Vec。
/// 这样 FuturesUnordered 并发不会因单个 Provider 提前返回而拖垮整体。
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Provider 唯一标识，对应 SearchResult.source。
    fn id(&self) -> &'static str;

    /// 该 Provider 响应哪些 tab。"all" 由调用方保证包含，无需在此判断。
    fn matches_tab(&self, tab: &str) -> bool;

    /// 执行搜索。绝不 panic / 绝不返回 Err。
    async fn search(&self, query: &str, ctx: &SearchContext<'_>) -> Vec<SearchResult>;

    /// 是否参与频次加权。shell 等命令序/时间序的返回 false。
    fn uses_frequency(&self) -> bool {
        true
    }

    /// 是否作为 fallback（无结果时兜底）。本期预留，无 Provider 启用。
    fn is_fallback(&self) -> bool {
        false
    }
}
