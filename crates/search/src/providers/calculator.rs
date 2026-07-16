//! CalculatorProvider：计算器（query 形如数学表达式时给出结果）。
//! Task 4 stub——search 返回空。Task 9 填真实实现。
//!
//! 注意：`matches_tab` 返回 false——calculator 仅在 `tab == "all"` 时参与，
//! 由 engine.rs search() 的 `tab == "all" || p.matches_tab(tab)` 兜底包含。

use async_trait::async_trait;

use crate::engine::SearchResult;
use crate::provider::{SearchContext, SearchProvider};

pub struct CalculatorProvider;

#[async_trait]
impl SearchProvider for CalculatorProvider {
    fn id(&self) -> &'static str {
        "calculator"
    }

    /// 仅由 search() 的 tab=="all" 包含。
    fn matches_tab(&self, _tab: &str) -> bool {
        false
    }

    async fn search(&self, _query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
        vec![]
    }
}
