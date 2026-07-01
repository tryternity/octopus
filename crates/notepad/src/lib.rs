//! octopus-notepad：内容收集箱式记事本业务逻辑。
//! 仅依赖 octopus-infra（DB 访问）+ rusqlite；文件 I/O 用 std + dirs。
//! 正文存 content_text（纯文本或 md 源码）+ type，无富文本 HTML。

pub mod export;
pub mod model;
pub mod store;

pub use model::{Note, NoteFilter, NoteSource, NoteType};
