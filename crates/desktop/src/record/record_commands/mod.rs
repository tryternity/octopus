//! 录屏 Tauri 命令（薄封装 octopus-record crate）。
//!
//! DB 访问模式：复用 octopus 既有 `octopus_infra::db::with_db(|conn| ...)` 全局函数，
//! 通过 ReentrantMutex 保护连接（参考 clipboard_commands.rs 模式）。
//! spawn_blocking 包裹避免长 DB 操作阻塞 tokio worker。
//!
//! 全模块 `#[cfg(target_os = "macos")]`：octopus-record 当前只实现了 macOS provider，
//! windows/linux provider 为占位（platform/windows.rs、platform/linux.rs）。
//! desktop crate 也仅在 macOS target 段引入 octopus-record 依赖，故此处整体 gate。

#![cfg(target_os = "macos")]

mod permission;
pub use permission::*;

mod library;
pub use library::*;

mod postprocess;
pub use postprocess::*;

mod control;
pub use control::*;

use octopus_record::platform::HelperProvider;
use octopus_record::{DisplayInfo, MicrophoneInfo, RecordError, WindowInfo};
use rusqlite::Connection;
use tauri::command;

// ── 辅助函数 ──────────────────────────────────────────────────

/// 把 RecordError 转 String 的统一出口——复用 error_util::e2s。
use crate::core::error_util::e2s;

/// 拿当前平台的 provider（零成本，MacOSProvider 是 ZST）。
///
/// trait async 化（2026-07-26，`#[async_trait]`）后，原 `platform_helper` 闭包 wrapper
/// 因 async_trait 的 `Box<dyn Future + Send + 'static>` 与 `&dyn HelperProvider` 的
/// 生命周期冲突编译不过。改为直接拿 provider 实例调用——ZST 无成本，调用点更直观。
fn provider() -> impl HelperProvider {
    octopus_record::platform::provider()
}

/// ISO8601 UTC 时间戳（DB 里 created_at 统一格式）。
fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// DB 操作 spawn_blocking 包裹（with_db 持全局 ReentrantMutex，长 DB 操作避免阻塞 tokio）。
///
/// 类型推导链：
/// 1. `f: F` 返回 `Result<T, RecordError>`
/// 2. 闭包内 `Ok(f(conn)?)` 把内层 RecordError 经 `?` 提升为 `anyhow::Error`
///    （RecordError: std::error::Error 满足 anyhow::From），外层包 Ok 变 anyhow::Result<T>
/// 3. `with_db` 直接返回该 anyhow::Result<T>
/// 4. spawn_blocking 再包一层 `Result<anyhow::Result<T>, JoinError>`
/// 5. `.await.map_err(join)` → `anyhow::Result<T>`
/// 6. `.map_err(e2s)` → `Result<T, String>`
async fn with_db_blocking<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, RecordError> + Send + 'static,
    T: Send + 'static,
{
    let result = tokio::task::spawn_blocking(move || {
        // 用 Ok(f(conn)?) 把闭包的 Result<T, RecordError> 收敛成 anyhow::Result<T>
        // （RecordError 实现了 std::error::Error，? 自动 From）。
        octopus_infra::db::with_db(|conn| Ok(f(conn)?))
    })
    .await
    .map_err(|e| format!("join error: {e}"))?; // anyhow::Result<T>

    result.map_err(e2s)
}

/// HelperEvent 已 derive Serialize（与 helper stdout 同一份 schema，双向用），
/// emit 时直接传给 Tauri，前端按 `event` 字段（kebab-case tag）match 分支。
/// 前端示例：
///   { "event": "recording-started", "timestamp_ms": 1773, "width": 1920, "height": 1080 }
///   { "event": "warning", "code": "...", "message": "..." }

// ── A. 源枚举（6 个，录制前调用）──────────────────────────────

#[command]
pub async fn list_record_displays() -> Result<Vec<DisplayInfo>, String> {
    // provider().list_displays() 已 async 化（2026-07-26），直接 .await helper 子进程，
    // 不再走 block_in_place——async Tauri 命令在 runtime worker 上不阻塞。
    provider().list_displays().await.map_err(e2s)
}

#[command]
pub async fn list_record_windows() -> Result<Vec<WindowInfo>, String> {
    provider().list_windows().await.map_err(e2s)
}

#[command]
pub async fn list_microphones() -> Result<Vec<MicrophoneInfo>, String> {
    provider().list_microphones().await.map_err(e2s)
}
