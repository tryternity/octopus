//! HelperProvider trait：跨平台 helper 二进制查找抽象。
//!
//! **async 化（2026-07-26）**：5 个调 helper 子进程的方法标 `async`（`#[async_trait]`）。
//! 原本 trait 是 sync 签名，macOS impl 内部用 `futures_block_on` + `block_in_place`
//! 桥接——但这会让 async Tauri 命令在 runtime worker 上阻塞，多 display 枚举并发时
//! 拖慢调度。改 async 后直接 `.await` 子进程，无阻塞。
//! `resolve_helper_path` 保留 sync（纯文件探测，无 `.await`）。

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;

use crate::error::RecordResult;
use crate::protocol::{DisplayInfo, MicrophoneInfo, PermissionStatus, WindowInfo};
use async_trait::async_trait;
use std::path::PathBuf;

/// 跨平台 helper 抽象。
///
/// `#[async_trait]` 让 trait 方法可以 `async fn` 同时保留 `dyn HelperProvider` 兼容
/// （原生 async-fn-in-trait 不支持 dyn dispatch）。`resolve_helper_path` 是纯文件探测，
/// 保留 sync——async_trait 允许同一 trait 内 sync/async 方法混用。
#[async_trait]
pub trait HelperProvider: Send + Sync {
    /// 返回 helper 二进制的绝对路径（纯文件探测，保留 sync——不走子进程）。
    /// 解析顺序：1) Tauri resource_dir（打包后）；2) 开发期 cargo target dir。
    fn resolve_helper_path(&self, app_resource_dir: Option<&std::path::Path>) -> RecordResult<PathBuf>;

    /// 列出可用显示器（走 helper --list-displays）。
    async fn list_displays(&self) -> RecordResult<Vec<DisplayInfo>>;

    /// 列出可用窗口（走 helper --list-windows）。
    async fn list_windows(&self) -> RecordResult<Vec<WindowInfo>>;

    /// 列出可用麦克风（走 helper --list-microphones）。
    async fn list_microphones(&self) -> RecordResult<Vec<MicrophoneInfo>>;

    /// 检查屏幕录制权限（走 helper --check-permission）。
    async fn check_permission(&self) -> RecordResult<PermissionStatus>;

    /// 申请屏幕录制权限（走 helper --request-permission）。
    async fn request_screen_permission(&self) -> RecordResult<PermissionStatus>;
}

#[cfg(target_os = "macos")]
pub fn provider() -> impl HelperProvider {
    crate::platform::macos::MacOSProvider
}

#[cfg(target_os = "windows")]
pub fn provider() -> impl HelperProvider {
    crate::platform::windows::WindowsProvider
}

#[cfg(target_os = "linux")]
pub fn provider() -> impl HelperProvider {
    crate::platform::linux::LinuxProvider
}

/// 跑 helper 子命令模式（--check-permission / --list-displays / ...）。
/// 通用工具：spawn helper 传一个子命令参数，等 stdout 第一行 JSON 解析。
///
/// 第三十一轮 P1-1：原 wait_with_output().await 无 timeout + 无 kill_on_drop——
/// macOS 权限弹窗等用户确认时 helper 阻塞 → wait 永不返回 → 前端 invoke 永久 await
/// （UI loading 永转）。现加 30s timeout（覆盖正常操作 + 给权限弹窗留时间，但防永久卡）
/// + kill_on_drop（timeout/helper hang 时杀子进程，不残留孤儿）。对齐 session.rs:174
/// 的 kill_on_drop + :314 timeout 范式。
#[allow(dead_code)] // MVP 只 macos 用，windows/linux 占位时不调
pub(crate) async fn run_helper_subcommand(
    helper_path: &std::path::Path,
    subcmd: &str,
) -> RecordResult<serde_json::Value> {
    let child = tokio::process::Command::new(helper_path)
        .arg(subcmd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    // 30s timeout——覆盖 list-displays/windows/microphones（<2s）+ check-permission（<5s）
    // + request-permission（用户确认弹窗，30s 充裕；超时返错让前端提示重试）。
    // 用 tokio::select 因 wait_with_output 消费 child（move），timeout 后拿不到 child kill。
    let output = tokio::select! {
        result = child.wait_with_output() => {
            result?
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
            // timeout——kill_on_drop 在 child drop 时杀进程，但 child 已 move 进
            // wait_with_output 的 future。select 的 cancel drop 该 future → drop child → kill。
            return Err(crate::error::RecordError::HelperError {
                code: "subcommand-timeout".into(),
                message: format!("{} timed out after 30s", subcmd),
            });
        }
    };
    if !output.status.success() {
        return Err(crate::error::RecordError::HelperError {
            code: "subcommand-failed".into(),
            message: format!("{} exited with {:?}", subcmd, output.status),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next().unwrap_or("");
    let value: serde_json::Value = serde_json::from_str(first_line)
        .map_err(crate::error::RecordError::Json)?;
    Ok(value)
}
