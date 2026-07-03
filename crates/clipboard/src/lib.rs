pub mod cleanup;
pub mod handle;
pub mod image;
pub mod model;
pub mod store;
pub mod watcher;

pub use handle::ClipboardHandle;
// 重导出 clipboard-rs 的剪贴板格式枚举与图片数据类型：desktop（paste.rs 备份/还原、
// clipboard_commands.rs）需命名 read_image 的返回类型，避免每个下游 crate 单独依赖 clipboard-rs。
pub use clipboard_rs::common::{ContentFormat, RustImageData};
pub use model::{AsrMeta, ClipboardItem, FileMeta, ImageMeta, ItemType, OcrMeta, QueryFilter, Source};
pub use watcher::ClipboardWatcher;
