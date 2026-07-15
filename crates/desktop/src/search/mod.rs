//! ActionBar 搜索引擎：应用索引 + 菜单搜索 + Quicklinks + 文件搜索 + 书签搜索。
//!
//! 统一 SearchResult 结构，nucleo-matcher 模糊匹配 + 拼音首字母。

pub mod matcher;
pub mod app_index;
pub mod file_search;
pub mod bookmark;
pub mod engine;

pub use engine::{SearchEngine, SearchResult, init_search_engine, get_engine};
