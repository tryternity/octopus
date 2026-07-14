#![warn(clippy::all)]
//! octopus-infra: 基础设施 crate（无项目内依赖）。
//!
//! 承载跨 crate 共享的基础设施：固定常量、路径工具、时间工具等。
//! 任何项目 crate 都可依赖本 crate；本 crate 不依赖任何项目 crate。

pub mod config;
pub mod consts;
pub mod net;
pub mod paths;
pub mod db;
pub mod model_probe;
pub mod hotword_text;
pub mod model_manifests;

// 高频路径函数提至 root，调用点用 octopus_infra::octopus_config_home（无需 paths:: 前缀）
pub use paths::octopus_config_home;
