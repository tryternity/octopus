//! AI 命令面板后端命令——模拟 Cmd+C / LLM 调用 / 模拟 Cmd+V / 打开 URL。
//!
//! 2026-07-29 起拆分为子模块。mod.rs 保留共享状态 + 各子模块 glob re-export
//! （`pub use submodule::*`）保持 `crate::action_bar::action_bar_commands::xxx` 路径不变。

mod agent;
pub use agent::*;

mod prompt_files;
pub use prompt_files::*;

mod window;
pub use window::*;

mod items;
pub use items::*;

mod translate;
pub use translate::*;

mod script;
pub use script::*;

mod context;
pub use context::*;

use parking_lot::Mutex;

/// 暂存选中对象 + 上下文（trigger 时写入，前端 mount 时读）。
/// context.rs 定义 `ActionBarContext` 类型本体；这里只保留跨子模块共享的实例。
static PENDING_CONTEXT: Mutex<Option<crate::action_bar::action_bar_commands::ActionBarContext>> = Mutex::new(None);

/// 重入 guard——防止热键连按导致 trigger 重叠执行（window.rs 的 trigger_* 直接读写）。
static TRIGGER_IN_PROGRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 触发时间戳——用于超时保护，防 webview 崩溃后 guard 永久卡死。
static TRIGGER_TIMESTAMP: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
