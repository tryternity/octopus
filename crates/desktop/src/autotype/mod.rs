//! Auto-Type：跨平台键盘模拟 + URL 检测。
//!
//! MVP 仅 macOS。Windows/Linux 编译通过但运行时返回 Err("not implemented")。

pub mod clipboard;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub mod url_detect;

#[cfg(target_os = "macos")]
pub use macos::{activate_app, autotype_login};
#[cfg(target_os = "macos")]
pub use url_detect::current_browser_url;
#[cfg(target_os = "macos")]
pub use clipboard::{copy_concealed, copy_concealed_with_ttl};

#[cfg(not(target_os = "macos"))]
pub fn activate_app(_bundle_id: &str) -> anyhow::Result<()> {
    anyhow::bail!("Auto-Type 尚未实现此平台")
}
#[cfg(not(target_os = "macos"))]
pub fn autotype_login(_u: &str, _p: &str, _enter: bool) -> anyhow::Result<()> {
    anyhow::bail!("Auto-Type 尚未实现此平台")
}
#[cfg(not(target_os = "macos"))]
pub fn current_browser_url() -> anyhow::Result<Option<String>> {
    Ok(None)
}
#[cfg(not(target_os = "macos"))]
pub fn copy_concealed(_t: &str) -> anyhow::Result<()> {
    anyhow::bail!("concealed 剪贴板尚未实现此平台")
}
#[cfg(not(target_os = "macos"))]
pub fn copy_concealed_with_ttl(_t: &str, _ttl: std::time::Duration) -> anyhow::Result<()> {
    anyhow::bail!("concealed 剪贴板尚未实现此平台")
}
