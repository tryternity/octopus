//! 外置 seed 数据加载——长文本 seed 从仓库内 seeds/ 目录读取，运行期拼装 SQL 插入 DB。
//! 仅 schema 升级（v<39）时执行一次；失败时 log::error 跳过该项，绝不阻塞 schema 升级。
//!
//! 设计动机：db.sql 内联长 prompt / 多 provider JSON 让 schema 真相难读，
//! 改为本模块从 `seeds/` 目录读 markdown / JSON 运行期拼装。db.sql 只保留表结构 +
//! 短种子（参考 Task 2/3）。

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::PathBuf;

/// seeds 目录绝对路径。
/// dev（cargo run / cargo test）：$CARGO_MANIFEST_DIR/seeds
/// release（裸二进制）：通过 Cargo.toml `package.include` 打包到 exe 同级/seeds
pub fn seeds_dir() -> PathBuf {
    // dev 路径——编译期取 Cargo.toml 所在目录
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("seeds");
    if dev.exists() {
        return dev;
    }
    // release 路径——exe 同级/seeds
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let release = parent.join("seeds");
            if release.exists() {
                return release;
            }
        }
    }
    // fallback：dev 路径（即使不存在也返回，调用方处理 Err）
    dev
}

/// 给 desktop crate 复原按钮用——按 prompt 简称返回 seed 文件路径。
/// name 示例："default-polish" / "advanced-polish"
pub fn seed_prompt_path(name: &str) -> Option<PathBuf> {
    let path = seeds_dir().join("prompts").join(format!("{}.md", name));
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_dir_returns_existing_path_in_dev() {
        let dir = seeds_dir();
        // dev 模式必须存在（仓库内）
        assert!(dir.exists(), "seeds_dir() 在 dev 模式应存在: {:?}", dir);
        assert!(dir.join("prompts/default-polish.md").exists());
    }

    #[test]
    fn seed_prompt_path_returns_some_for_known_name() {
        let path = seed_prompt_path("default-polish");
        assert!(path.is_some());
        assert!(path.unwrap().exists());
    }

    #[test]
    fn seed_prompt_path_returns_none_for_unknown_name() {
        assert!(seed_prompt_path("nonexistent-prompt").is_none());
    }
}
