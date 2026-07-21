//! Vault 同步 Tauri 命令（2026-07-21 Phase 1）。
//!
//! 薄包装层——把 `octopus_vault::sync::engine` 的纯逻辑函数暴露为 Tauri 命令。
//! SyncError → String 转换用 Display（用户可读消息）。
//!
//! 所有命令在 `vault` feature gate 下编译。

use octopus_vault::sync::{self as vault_sync, SyncReport, SyncStatus};

/// SyncError → Tauri String（用 Display，用户可读）。
fn sync_err_to_string(e: octopus_vault::sync::SyncError) -> String {
    e.to_string()
}

/// 查询同步状态——UI 初始化时调用。
#[tauri::command]
pub fn vault_sync_status() -> Result<SyncStatus, String> {
    Ok(vault_sync::get_sync_status())
}

/// 测试远程连接——`git ls-remote --heads <url>`。
#[tauri::command]
pub fn vault_sync_test_connection(remote_url: String) -> Result<(), String> {
    vault_sync::test_connection(&remote_url).map_err(sync_err_to_string)
}

/// 启用同步——初始化本地 git repo + 导出 SQLite 数据 + 首次 commit。
/// 不 push（用户需先 add_remote）。
#[tauri::command]
pub fn vault_sync_enable() -> Result<(), String> {
    vault_sync::enable_sync().map_err(sync_err_to_string)
}

/// 手动触发同步——编排 pull + push 流程。
#[tauri::command]
pub fn vault_sync_now() -> Result<SyncReport, String> {
    vault_sync::sync_now().map_err(sync_err_to_string)
}

/// 禁用同步——删除 `~/.octopus/.vault/`（保留 SQLite 数据）。
#[tauri::command]
pub fn vault_sync_disable() -> Result<(), String> {
    vault_sync::disable_sync().map_err(sync_err_to_string)
}

/// 检测系统 git 是否可用——启动时调用，决定是否显示同步 UI。
#[tauri::command]
pub fn vault_is_git_available() -> bool {
    octopus_vault::sync::git::check_git_available()
}

/// 添加 remote——用户自由输入 URL + 名称，不限制 GitHub/Gitee/自建。
#[tauri::command]
pub fn vault_sync_add_remote(name: String, url: String) -> Result<(), String> {
    vault_sync::add_remote(&name, &url).map_err(sync_err_to_string)
}

/// 删除 remote。
#[tauri::command]
pub fn vault_sync_remove_remote(name: String) -> Result<(), String> {
    vault_sync::remove_remote(&name).map_err(sync_err_to_string)
}

/// 列出所有 remote（name → url）。
#[tauri::command]
pub fn vault_sync_list_remotes() -> Result<Vec<(String, String)>, String> {
    vault_sync::list_remotes().map_err(sync_err_to_string)
}

/// 从指定 remote URL clone 仓库（B 机首次同步）。
#[tauri::command]
pub fn vault_sync_clone(remote_url: String) -> Result<(), String> {
    vault_sync::clone_from(&remote_url).map_err(sync_err_to_string)
}
