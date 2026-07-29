//! vault 密码生成 / 评估 / TOTP / 健康报告 / 导入导出。

use tauri::State;

use octopus_vault::generator::GeneratorConfig;
use octopus_vault::health::HealthReport;
use octopus_vault::importer::ImportReport;
use octopus_vault::types::CipherData;

use crate::error_util::e2s;
use crate::runtime_config::SharedRuntimeConfig;
use crate::vault_error::{self, VaultError};
use crate::vault_state::SharedVaultSession;

use super::require_user_vault_key;

#[tauri::command]
pub fn vault_generate(cfg: GeneratorConfig) -> Result<String, String> {
    octopus_vault::generator::generate(&cfg).map_err(vault_error::to_tauri_error)
}

/// 评估密码强度（zxcvbn 评分 + 熵）。
///
/// 前端 CipherEditor 在密码字段下方实时展示强度条：debounce 300ms 后调用本命令，
/// 避免每键都跑 zxcvbn（前端不打包 zxcvbn，统一走后端单点）。
///
/// 不需要 user_vault_key——评估是纯计算，不接触 vault 数据。
///
/// 直接返回内部 `PasswordStrength`（已带 `rename_all = "camelCase"`，2026-07-27 DTO
/// 消除：原 `PasswordStrengthDto` 仅暴露 score + entropy 是历史前端能力受限的产物，
/// 现直接返回完整结构——多出的 `warning` / `suggestions` 字段对前端是可选增强，
/// JSON 多字段不破坏现有契约）。
#[cfg(feature = "vault")]
#[tauri::command]
pub fn vault_evaluate_password(password: String) -> octopus_vault::health::strength::PasswordStrength {
    octopus_vault::health::strength::evaluate(&password)
}

/// `vault_generate_totp` 命令返回值（前端调用方唯一消费点，就近定义）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotpResult {
    pub code: String,
    pub seconds_remaining: u64,
}

#[tauri::command]
pub fn vault_generate_totp(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    cipher_id: String,
) -> Result<TotpResult, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let cipher = octopus_vault::storage::load_cipher(&cipher_id, &key)
        .map_err(vault_error::to_tauri_error)?
        .ok_or_else(|| vault_error::serialize(&VaultError::CipherNotFound(cipher_id.clone())))?;
    // CipherData 当前仅 Login 单变体；预留 unreachable arm 以便未来扩展。
    #[allow(unreachable_patterns)]
    let login = match cipher.data {
        CipherData::Login(l) => l,
        _ => return Err(vault_error::serialize(&VaultError::InvalidInput("非 Login 类型".into()))),
    };
    let totp_secret = login
        .totp
        .ok_or_else(|| vault_error::serialize(&VaultError::InvalidInput("无 TOTP secret".into())))?;
    // from_input 智能分发（修复 #7）：
    // - otpauth:// 开头 → 解析完整 URL（SHA256/SHA512、digits=8、period=60 等）
    // - 否则 → 裸 Base32 secret，默认 SHA1/6/30
    // 两种输入都接受 RFC 6238 下限的 80bit secret（new_unchecked / from_url_unchecked）
    let gen = octopus_vault::totp::TotpGenerator::from_input(&totp_secret)
        .map_err(vault_error::to_tauri_error)?;
    // T1 修复（2026-07-24）：用 current_with_remaining 单次读时钟，避免 current()
    // + seconds_remaining() 各自读 SystemTime 在 step 边界不一致（陈旧 1 秒显示）。
    let (code, seconds_remaining) = gen
        .current_with_remaining()
        .map_err(vault_error::to_tauri_error)?;
    Ok(TotpResult {
        code,
        seconds_remaining,
    })
}

#[tauri::command]
pub fn vault_health_report(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
) -> Result<HealthReport, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let (ciphers, failures) =
        octopus_vault::storage::list_ciphers(&key).map_err(vault_error::to_tauri_error)?;
    if !failures.is_empty() {
        log::warn!("vault_health_report: {} 条记录解密失败已跳过", failures.len());
    }
    Ok(octopus_vault::health::generate_report(&ciphers))
}

#[tauri::command]
pub fn vault_import_bitwarden(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
    json: String,
) -> Result<ImportReport, String> {
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    // 内部 anyhow 错误可能含 JSON parse 详情 / SQL 片段——统一映射到 ImportFailed
    // 的稳定 message，不透传内部细节。
    octopus_vault::importer::import_bitwarden_json(&json, &key)
        .map_err(|_| vault_error::serialize(&VaultError::ImportFailed(String::new())))
}

#[tauri::command]
pub fn vault_export(
    state: State<'_, SharedVaultSession>,
    config: State<'_, SharedRuntimeConfig>,
) -> Result<String, String> {
    // L17 修复（2026-07-24）：加 SYNC_LOCK——list_ciphers + list_folders 两次 DB 读
    // 期间若 sync_now 并发写入会跨事务边界（快照不一致）。与 empty_trash 同模式（T2）。
    let _sync_guard = octopus_vault::sync::engine::try_sync_lock()
        .map_err(e2s)?;
    let key = require_user_vault_key(&state, &config).map_err(|e| vault_error::serialize(&e))?;
    let (ciphers, failures) =
        octopus_vault::storage::list_ciphers(&key).map_err(vault_error::to_tauri_error)?;
    if !failures.is_empty() {
        log::warn!("vault_export: {} 条记录解密失败已跳过（未导出）", failures.len());
    }
    // M6 修复：读 folders 一并导出（之前 folders 硬编码空 → 导入后丢失文件夹归属）
    let (folders, folder_failures) =
        octopus_vault::storage::list_folders(&key).map_err(vault_error::to_tauri_error)?;
    if !folder_failures.is_empty() {
        log::warn!(
            "vault_export: {} 个文件夹解密失败已跳过",
            folder_failures.len()
        );
    }
    octopus_vault::importer::export_vault_json(&ciphers, &folders)
        .map_err(vault_error::to_tauri_error)
}
