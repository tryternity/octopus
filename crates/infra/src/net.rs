//! 网络超时常量（全项目统一引用，避免散落不一致）。

/// WebSocket 连接建立超时（秒）
pub const WS_CONNECT_TIMEOUT_SECS: u64 = 10;
/// WebSocket 流式读取超时（秒）——语音流间隙，超过此无数据视为断连
pub const WS_READ_TIMEOUT_SECS: u64 = 30;
/// HTTP 请求超时（秒）——LLM 推理较慢，给足时间
pub const HTTP_TIMEOUT_SECS: u64 = 120;
/// gRPC 连接建立超时（秒）
pub const GRPC_CONNECT_TIMEOUT_SECS: u64 = 8;
/// gRPC 请求超时（秒）
pub const GRPC_REQUEST_TIMEOUT_SECS: u64 = 30;
/// 文件下载超时（秒）——大模型文件
pub const FILE_DOWNLOAD_TIMEOUT_SECS: u64 = 300;
