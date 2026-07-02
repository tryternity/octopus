//! octopus-notepad：内容收集箱式记事本业务逻辑。
//! 仅依赖 octopus-infra（DB 访问）；文件 I/O 用 std + dirs。
//!
//! 各业务模块（model/store/export）落地。富文本（serialize/TipTap）已移除。

pub mod export;
pub mod model;
pub mod store;

pub use model::{Note, NoteFilter, NoteSource, NoteType};
