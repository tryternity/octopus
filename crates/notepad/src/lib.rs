//! octopus-notepad：内容收集箱式记事本业务逻辑。
//! 仅依赖 octopus-infra（DB 访问）；序列化用 scraper，文件 I/O 用 std + dirs。
//!
//! 各业务模块（model/serialize/store/export）在后续 task 逐个落地并在此 `pub mod` 注册。
//! 当前为 crate 骨架，仅保证 `octopus-notepad` 作为 workspace member + desktop 依赖可编译通过。
