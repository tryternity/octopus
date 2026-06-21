//! octopus-download：通用文件下载 crate（分块并发 + 断点续传 + 校验 + 镜像）。
//!
//! 两模块：`core`（通用，零 HF 知识）+ `hf`（HuggingFace 适配层）。
//! 详见 `docs/superpowers/specs/2026-06-21-model-download-design.md`。

pub mod core;
pub mod hf;
