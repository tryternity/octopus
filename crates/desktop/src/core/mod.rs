//! 启动 + 基础设施功能域。

pub mod setup;
pub mod bootstrap;
pub mod config;
pub mod runtime_config;
pub mod db_queue;
#[macro_use]
pub mod invoke_handler;
pub mod error_util;
pub mod perf_log;
pub mod file_watcher;
pub mod extensions;
// shortcut.rs 已删除：register_shortcut（原 asr toggle 热重载用）被 PTT 热重载取代
// （platform::ptt::register_ptt / unregister_ptt）。
