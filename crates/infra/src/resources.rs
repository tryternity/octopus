//! 编译期内联资源统一入口。
//!
//! 2026-08-04 集中化：DB schema / 字典 / 模型 / prompt 从各 crate 散落的
//! `include_bytes!`/`include_str!` 集中到 `infra/resources/`，消除跨 crate `../../`
//! 脆弱路径。
//!
//! crate 专有资源（desktop icon/i18n/tauri.conf、pty shell 脚本）保留原位，
//! 调用方用 `env!("CARGO_MANIFEST_DIR")` 消除 `../../`——本模块不为其提供 API。
