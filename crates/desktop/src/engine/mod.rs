//! ASR 全栈功能域：引擎 trait + 实现 + 云端流水线 + pipeline + transcript + audio + coordinator。

pub mod engine;
pub mod engine_embedded;
#[cfg(feature = "remote-ws")] pub mod engine_ws;
#[cfg(feature = "remote-grpc")] pub mod engine_grpc;
pub mod engine_dispatch;
#[cfg(feature = "cloud")] pub mod engine_aliyun;
#[cfg(feature = "cloud")] pub mod cloud_pipeline;
pub mod pipeline;
pub mod transcript;
pub mod audio;
pub mod coordinator;
