//! octopus-notepad：内容收集箱式记事本业务逻辑。
//! 仅依赖 octopus-infra（DB 访问）；序列化用 scraper，文件 I/O 用 std + dirs。
//!
//! 各业务模块（model/serialize/store/export）在后续 task 逐个落地。
//! 当前已完成 model；serialize/store/export 为占位，后续 task 覆写。

pub mod export;
pub mod model;
pub mod serialize;
pub mod store;

pub use model::{Note, NoteFilter, NoteSource};
