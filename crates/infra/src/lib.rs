//! octopus-infra: 基础设施 crate（无项目内依赖）。
//!
//! 承载跨 crate 共享的基础设施：固定常量、路径工具、时间工具等。
//! 任何项目 crate 都可依赖本 crate；本 crate 不依赖任何项目 crate。

pub mod consts;
