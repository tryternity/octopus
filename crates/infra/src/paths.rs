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

/// 录屏输出目录：读 DB `record_output_dir` 配置（绝对路径，支持 `~` 展开）。
/// 空/未配置时 fallback `~/download/octopus/recordings/`。
/// 不存在时由调用方在 start_recording 前创建。
pub fn recordings_dir() -> PathBuf {
    let configured = crate::db::load_config_key("record_output_dir")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    match configured {
        Some(dir) => expand_tilde(&dir),
        None => expand_tilde("~/download/octopus/recordings"),
    }
}

/// 展开 `~` 为 $HOME（macOS/Linux）。已是绝对路径则原样返回。
/// 不引入 shellexpand 依赖——手动展开足够（录屏 macOS-only）。
fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(&path[2..])
    } else if path == "~" {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
    } else {
        PathBuf::from(path)
    }
}

/// 解析 recordings 表里的 file_path 为绝对路径。
/// 2026-07-27 起 file_path 直接存**绝对路径**（用户可配置保存目录），
/// 此函数对绝对路径原样返回；防御性 fallback：相对路径 join octopus_config_home()。
pub fn resolve_recording_path(file_path: &str) -> PathBuf {
    let p = PathBuf::from(file_path);
    if p.is_absolute() {
        p
    } else {
        octopus_config_home().join(p)
    }
}

/// 录屏 helper 子进程的 stdout/stderr 日志路径：~/.octopus/logs/record-helper.log
/// （logs 目录约定与 desktop/action_bar_commands.rs、desktop/perf_log.rs 一致。）
pub fn record_helper_log() -> PathBuf {
    octopus_config_home().join("logs").join("record-helper.log")
}
