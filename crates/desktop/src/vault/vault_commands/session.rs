//! vault 会话管理：状态查询 / setup / unlock / lock / 心跳 / 自动锁定超时 / 改密。

use std::sync::Arc;
use zeroize::Zeroizing;

use tauri::State;

use crate::core::error_util::e2s;
use crate::core::runtime_config::SharedRuntimeConfig;
use crate::vault::vault_error::{self, VaultError};
use crate::vault::vault_state::SharedVaultSession;

/// `vault_status` 命令返回值（前端调用方唯一消费点，故就近定义而非堆在 DTO 区）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub initialized: bool,
    pub user_vault_unlocked: bool,
}

#[tauri::command]
pub fn vault_status(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
) -> Result<VaultStatus, String> {
    let timeout = config.read().vault_lock_timeout_secs;
    let initialized = octopus_vault::unlock::is_initialized().map_err(vault_error::to_tauri_error)?;
    // 用 write() 因为 is_user_vault_unlocked 超时会主动清零 key
    let user_vault_unlocked = state.write().is_user_vault_unlocked(timeout);
    Ok(VaultStatus {
        initialized,
        user_vault_unlocked,
    })
}

#[tauri::command]
pub fn vault_setup(state: State<'_, SharedVaultSession>, password: String) -> Result<(), String> {
    // H1 修复：主密码用 Zeroizing 包裹 move 进 vault 层，vault 层结束时清零 heap
    let keys = octopus_vault::unlock::setup_vault(Zeroizing::new(password))
        .map_err(vault_error::to_tauri_error)?;
    let mut session = state.write();
    session.set_user_vault_unlocked(Arc::new(keys.user_vault_key));
    session.app_key = Some(Arc::new(keys.app_key));
    Ok(())
}

#[tauri::command]
pub fn vault_unlock(state: State<'_, SharedVaultSession>, password: String) -> Result<(), String> {
    let keys = octopus_vault::unlock::unlock_with_master_password(Zeroizing::new(password))
        .map_err(vault_error::to_tauri_error)?;
    let mut session = state.write();
    session.set_user_vault_unlocked(Arc::new(keys.user_vault_key));
    session.app_key = Some(Arc::new(keys.app_key));
    Ok(())
}

#[tauri::command]
pub fn vault_lock(state: State<'_, SharedVaultSession>) -> Result<(), String> {
    state.write().lock_user_vault();
    Ok(())
}

/// 前端保险库 tab 处于前台时每 30s 调用一次，刷新 last_active_at 防止超时锁定。
///
/// 前端卸载（切 tab / 关窗口）后心跳停止，超过 `vault_lock_timeout_secs`（运行时配置，
/// 0=永不）后 is_user_vault_unlocked 自动返回 false 并清零 key。
#[tauri::command]
pub fn vault_heartbeat(state: State<'_, SharedVaultSession>) -> Result<(), String> {
    state.write().heartbeat();
    Ok(())
}

// === 自动锁定超时配置（vault 内联 UI，非通用 settings 表单）===
//
// 这些命令读写 `AppConfig.vault_lock_timeout_secs`，但 UI 控件挂在 VaultPanel
// 顶部（不是 General Settings）——vault 相关配置归属 vault，符合「就近原则」。
// 持久化走通用 `octopus_infra::db::save_app_config`（与 set_config 同路径）。

/// 读取当前自动锁定超时（秒）。
///   - `0`  = 永不锁定（UI 应展示警告）
///   - `>0` = 离开焦点后多少秒锁定
#[tauri::command]
pub fn vault_get_lock_timeout(config: State<'_, SharedRuntimeConfig>) -> u64 {
    config.read().vault_lock_timeout_secs
}

/// 设置自动锁定超时（秒）。校验：0=永不 或 30-3600。
///
/// 同步刷新 `SharedRuntimeConfig` 内存态并落库——下次 `is_user_vault_unlocked`
/// 取值即生效（心跳 / 状态轮询天然周期性读 config）。
#[tauri::command]
pub fn vault_set_lock_timeout(
    config: State<'_, SharedRuntimeConfig>,
    secs: u64,
) -> Result<(), String> {
    if secs != 0 && (secs < 30 || secs > 3600) {
        return Err(vault_error::serialize(&VaultError::InvalidInput(format!(
            "超时值无效：{}（应为 0=永不，或 30-3600）",
            secs
        ))));
    }
    let mut cfg = config.write();
    cfg.vault_lock_timeout_secs = secs;
    octopus_infra::db::save_app_config(&cfg).map_err(e2s)?;
    log::info!("vault 自动锁定超时已更新为 {}s", secs);
    Ok(())
}

#[tauri::command]
pub fn vault_change_password(
    state: State<'_, SharedVaultSession>,
    old_password: String,
    new_password: String,
) -> Result<(), String> {
    let keys = octopus_vault::unlock::change_master_password(
        Zeroizing::new(old_password),
        Zeroizing::new(new_password),
    )
    .map_err(vault_error::to_tauri_error)?;
    // 改密码成功后用返回的 keys 刷新 session（即使之前是 locked 也 re-unlock）。
    // user_vault_key / app_key 在改密码流程中不变（INV-7），但显式刷一下让
    // 「先 lock 再改密码」也能用——跟 setup_vault / vault_unlock 一致。
    // （follow-up #3）
    let mut session = state.write();
    session.set_user_vault_unlocked(Arc::new(keys.user_vault_key));
    session.app_key = Some(Arc::new(keys.app_key));
    Ok(())
}
