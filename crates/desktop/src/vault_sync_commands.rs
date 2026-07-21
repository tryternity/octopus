//! Vault 同步 Tauri 命令（2026-07-21 Phase 1）。
//!
//! 薄包装层——把 `octopus_vault::sync::engine` 的纯逻辑函数暴露为 Tauri 命令。
//! SyncError → String 转换用 Display（用户可读消息）。
//!
//! 所有命令在 `vault` feature gate 下编译。

use octopus_vault::sync::{self as vault_sync, SyncReport, SyncStatus};
use tauri::{AppHandle, Emitter};
use serde::Serialize;

/// SyncError → Tauri String（用 Display，用户可读）。
fn sync_err_to_string(e: octopus_vault::sync::SyncError) -> String {
    e.to_string()
}

/// `vault-sync-done` 事件 payload——sync_now 完成后 emit 给前端。
#[derive(Debug, Clone, Serialize)]
struct SyncDonePayload {
    /// 成功时含报告，失败时 None
    report: Option<SyncReport>,
    /// 失败时含错误消息，成功时 None
    error: Option<String>,
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

/// 手动触发同步——**异步**：spawn 后台线程跑 sync_now，命令立即返回。
///
/// UI 不阻塞——sync_now 可能跑 10-30 秒（fetch + push），同步执行会让整个
/// SyncPanel 看起来像没响应（用户反馈：「感觉像应用没响应了一样」）。
///
/// 完成（成功/失败）后 emit `vault-sync-done` 事件，payload 含 report 或 error。
/// 前端 listen 该事件刷新状态 + 显示结果 toast。
///
/// UI 实时进度：通过 `vault_sync_status.syncing` 字段查询（true 时显进度条）。
#[tauri::command]
pub async fn vault_sync_now(app: AppHandle) -> Result<(), String> {
    // spawn_blocking 起独立线程跑 sync_now（阻塞操作：git shell out）
    // 不等结果——命令立即返回，让前端 UI 切到进度条状态。
    // 结果通过 vault-sync-done 事件 emit，前端 listen 处理。
    tauri::async_runtime::spawn_blocking(move || {
        let result = vault_sync::sync_now();
        let payload = match result {
            Ok(report) => SyncDonePayload {
                report: Some(report),
                error: None,
            },
            Err(e) => {
                let msg = sync_err_to_string(e);
                SyncDonePayload {
                    report: None,
                    error: Some(msg),
                }
            }
        };
        // emit 给所有窗口——用户可能已切走或关 SyncPanel
        let _ = app.emit("vault-sync-done", payload);
    });
    Ok(())
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
