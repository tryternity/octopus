//! octopus-search: 独立搜索引擎 crate。
//!
//! 应用索引 + 菜单搜索 + Quicklinks + 文件搜索 + 书签搜索。
//! 统一 SearchResult 结构，nucleo-matcher 模糊匹配 + 拼音首字母。
//! 不依赖 Tauri，可独立测试和复用。

pub mod matcher;
pub mod app_index;
pub mod file_search;
pub mod bookmark;
pub mod engine;
pub mod frequency;
pub mod provider;
pub mod providers;
pub mod command_index;

pub use engine::{SearchEngine, SearchResult, SearchBatch, init_search_engine, get_engine};
