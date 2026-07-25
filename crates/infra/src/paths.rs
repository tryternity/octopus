// crates/infra/src/paths.rs
// 路径工具：跨 crate 共享的根目录定位。
// asr / llm / dlp / desktop / cli / server 统一调用，不再各自定义。

use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};

/// $HOME/.octopus — 全局根目录，所有配置 / 模型 / 数据都基于此。
static OCTOPUS_HOME: Lazy<PathBuf> = Lazy::new(|| {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".octopus")
});

/// 获取 ~/.octopus 路径（Lazy 缓存，进程内首次调用后固定）。
pub fn octopus_config_home() -> &'static Path {
    OCTOPUS_HOME.as_path()
}

// ── 录屏 ───────────────────────────────────────────────────────────

/// 录屏输出目录：~/.octopus/recordings/
/// 不存在时由调用方在 start_recording 前创建。
pub fn recordings_dir() -> PathBuf {
    octopus_config_home().join("recordings")
}

/// 解析 recordings 表里的相对路径为绝对路径。
/// file_path 字段存 "recordings/xxx.mp4" 这种相对路径，
/// 运行时 join octopus_config_home() 得到绝对路径。
pub fn resolve_recording_path(relative: &str) -> PathBuf {
    octopus_config_home().join(relative)
}

/// 录屏 helper 子进程的 stdout/stderr 日志路径：~/.octopus/logs/record-helper.log
/// （logs 目录约定与 desktop/action_bar_commands.rs、desktop/perf_log.rs 一致。）
pub fn record_helper_log() -> PathBuf {
    octopus_config_home().join("logs").join("record-helper.log")
}
