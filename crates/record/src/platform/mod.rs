//! HelperProvider trait：跨平台 helper 二进制查找抽象。

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;

use crate::error::RecordResult;
use crate::protocol::{DisplayInfo, MicrophoneInfo, PermissionStatus, WindowInfo};
use std::path::PathBuf;

pub trait HelperProvider: Send + Sync {
    /// 返回 helper 二进制的绝对路径。
    /// 解析顺序：1) Tauri resource_dir（打包后）；2) 开发期 cargo target dir。
    fn resolve_helper_path(&self, app_resource_dir: Option<&std::path::Path>) -> RecordResult<PathBuf>;

    /// 列出可用显示器（走 helper --list-displays）。
    fn list_displays(&self) -> RecordResult<Vec<DisplayInfo>>;

    /// 列出可用窗口（走 helper --list-windows）。
    fn list_windows(&self) -> RecordResult<Vec<WindowInfo>>;

    /// 列出可用麦克风（走 helper --list-microphones）。
    fn list_microphones(&self) -> RecordResult<Vec<MicrophoneInfo>>;

    /// 检查屏幕录制权限（走 helper --check-permission）。
    fn check_permission(&self) -> RecordResult<PermissionStatus>;

    /// 申请屏幕录制权限（走 helper --request-permission）。
    fn request_screen_permission(&self) -> RecordResult<PermissionStatus>;
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
#[allow(dead_code)] // MVP 只 macos 用，windows/linux 占位时不调
pub(crate) async fn run_helper_subcommand(
    helper_path: &std::path::Path,
    subcmd: &str,
) -> RecordResult<serde_json::Value> {
    let output = tokio::process::Command::new(helper_path)
        .arg(subcmd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?
        .wait_with_output()
        .await?;
    if !output.status.success() {
        return Err(crate::error::RecordError::HelperError {
            code: "subcommand-failed".into(),
            message: format!("{} exited with {:?}", subcmd, output.status),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next().unwrap_or("");
    let value: serde_json::Value = serde_json::from_str(first_line)
        .map_err(|e| crate::error::RecordError::Json(e))?;
    Ok(value)
}
