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
