//! HuggingFace 适配层。
pub mod api;
pub mod glob;
pub mod resolve;

pub use resolve::{HfRequest, resolve_tasks};
