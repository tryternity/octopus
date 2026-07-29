//! vault Tauri 命令层。
//!
//! 命令返回类型用 DTO（避免直接暴露 vault crate 内部类型）。
//! 错误统一映射为 `String`（前端用 `err` 分支即可）。
//!
//! 2026-07-29 起拆分为子模块。mod.rs 保留共享 helper/DTO struct + 各子模块 glob re-export
//! （`pub use submodule::*`）保持 `crate::vault::vault_commands::xxx` 路径不变。

mod window;
pub use window::*;

mod session;
pub use session::*;

mod generate;
pub use generate::*;

mod autotype;
pub use autotype::*;

mod cipher;
pub use cipher::*;

use std::sync::Arc;

use tauri::State;

use octopus_vault::crypto::DerivedKey;
use octopus_vault::types::{Cipher, CipherData, CipherInput, CipherType, Field, LoginData, RepromptType};

use crate::runtime_config::SharedRuntimeConfig;
use crate::vault::vault_error::VaultError;
use crate::vault::vault_state::SharedVaultSession;

// === DTO ===
//
// 仅保留 Cipher 相关 DTO（CipherDto / CipherInputDto / LoginDataDto / LoginUriDto /
// FieldDto）——它们是 Phase 2/3 的真实 transformation（Cipher 内部用强类型 enum，
// 前端用裸 i64），不在本次 DTO 消除范围。
//
// VaultStatusDto / TotpResultDto / PasswordStrengthDto / AutoTypeResultDto 已于
// 2026-07-27 移除：要么返回内部类型（PasswordStrength），要么改为命令同文件内
// 的局部 wire-format struct（VaultStatus / TotpResult / AutoTypeResult）。

// Phase 2（2026-07-28）：LoginUriDto/LoginDataDto/FieldDto 已删除——内部 struct
// （LoginUri/LoginData/Field）已有 rename_all + alias，直接用于 CipherDto 字段类型。
// MatchType 枚举有 #[serde(into = "i64", from = "i64")]，序列化为 i64，wire format
// 与原 DTO（Option<i64>）一致。LoginData 多出的 passwordRevisionDate 字段对前端
// 是 extra field（harmless）。


#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CipherDto {
    pub id: String, // UUID v4 字符串（2026-07-21 v44：支持 git 同步）
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub atype: i64,
    pub name: String,
    pub notes: Option<String>,
    pub login: Option<LoginData>,
    pub fields: Vec<Field>,
    pub reprompt: i64,
    pub is_deleted: bool,
    pub created_at: String,
    pub updated_at: String,
}


#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CipherInputDto {
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub name: String,
    pub notes: Option<String>,
    pub login: Option<LoginData>,
    pub fields: Vec<Field>,
    pub reprompt: Option<i64>,
}


// === AppState key 取用辅助 ===

/// 从 AppState 取 user_vault_key（必须解锁），否则返回 [`VaultError::Locked`]。
///
/// 直接返回精确的 `VaultError`（而非走字符串启发式 `classify`）——这是命令层
/// 已知语义的最精确分类点。调用方用 `.map_err(|e| vault_error::serialize(&e))?`
/// 转 Tauri 的 `Result<_, String>`。
fn require_user_vault_key(
    state: &State<'_, SharedVaultSession>,
    config: &State<'_, SharedRuntimeConfig>,
) -> Result<Arc<DerivedKey>, VaultError> {
    let timeout = config.read().vault_lock_timeout_secs;
    let mut session = state.write();
    if !session.is_user_vault_unlocked(timeout) {
        return Err(VaultError::Locked);
    }
    session.user_vault_key.clone().ok_or(VaultError::Locked)
}

/// 从 AppState 取 app_key（启动时已 bootstrap，不应为空）。
///
/// follow-up #7 起被 `vault_secret_access::try_decrypt_secret` 复用——cloud 推理热路径
/// 需要用 app_key 解 `v1:` 前缀的 secret_key。
///
/// 提供裸 `SharedVaultSession` 版本（非命令层调用点：`vault_secret_access`、
/// 未来其它内部消费方）。Tauri 命令层若需用，从 `state.inner()` 取 `&SharedVaultSession`
/// 传入即可。
///
/// 返回 [`VaultError::KeychainUnavailable`] 表达「app_key 不可用」——通常意味着
/// bootstrap 失败（keychain 拒访）或 vault 未初始化。
pub(crate) fn require_app_key_from_session(
    session: &SharedVaultSession,
) -> Result<Arc<DerivedKey>, VaultError> {
    let s = session.read();
    s.app_key.clone().ok_or(VaultError::KeychainUnavailable)
}


// === DTO ↔ Domain 转换 ===
//
// Phase 2（2026-07-28）：LoginDataDto/LoginUriDto/FieldDto 已删除。CipherDto 直接
// 使用内部 LoginData/Field 类型（已有 rename_all + alias）。转换函数仍保留——
// CipherDto 做真转换：CipherData enum 展平为 Option<LoginData> + enum→i64 + 丢字段。
// 这个转换层有架构价值（wire shape 简化），不是 casing 冗余。

fn cipher_to_dto(c: Cipher) -> CipherDto {
    // CipherData 当前仅 Login 单变体；保留 match 以便未来扩展 SecureNote/Card/Identity。
    #[allow(irrefutable_let_patterns)]
    let (login, atype) = match &c.data {
        CipherData::Login(l) => (Some(l.clone()), 1),
    };
    CipherDto {
        id: c.id.clone(),
        folder_id: c.folder_id.clone(),
        favorite: c.favorite,
        atype,
        name: c.name,
        notes: c.notes,
        login,
        fields: c.fields.clone(),
        reprompt: c.reprompt.into(),
        is_deleted: c.is_deleted,
        created_at: c.created_at,
        updated_at: c.updated_at,
    }
}


fn dto_to_input(dto: CipherInputDto) -> Result<CipherInput, VaultError> {
    let login = dto.login.ok_or_else(|| VaultError::InvalidInput("login 必填".into()))?;
    Ok(CipherInput {
        folder_id: dto.folder_id,
        favorite: dto.favorite,
        atype: CipherType::Login,
        name: dto.name,
        notes: dto.notes,
        data: CipherData::Login(login),
        fields: dto.fields,
        password_history: vec![],
        reprompt: dto
            .reprompt
            .map(RepromptType::from)
            .unwrap_or(RepromptType::None),
    })
}


/// password_history 上限（避免无界增长）。FIFO 截断：丢最老的。
pub const PASSWORD_HISTORY_MAX: usize = 20;


/// 在保存前合并现有 history 并按需追加新条目。
///
/// 行为：
///   - 始终以 `existing.password_history` 为起点（前端 CipherInputDto 不管理 history，
///     避免每次保存都清成 `[]` —— final-review I3）
///   - 当 password 字段变化（且旧值非空）时追加一条 `PasswordHistoryEntry`，
///     `last_used_at` 用 cipher 的 `updated_at`（代表它最后一次作为活动密码的时间）
///     —— follow-up #2
///   - FIFO 截断到 `PASSWORD_HISTORY_MAX` 条（丢最老的）
///
/// 抽出为自由函数便于单元测试（不需要 Tauri `State`）。
fn merge_password_history(
    mut domain: CipherInput,
    existing: &Cipher,
) -> CipherInput {
    // CipherData 当前仅 Login 单变体；保留 irrefutable_let_patterns 以便未来扩展。
    #[allow(irrefutable_let_patterns)]
    let old_password: Option<&str> = match &existing.data {
        CipherData::Login(l) => l.password.as_deref(),
    };
    #[allow(irrefutable_let_patterns)]
    let new_password: Option<&str> = match &domain.data {
        CipherData::Login(l) => l.password.as_deref(),
    };

    // 1. 保留现有 history（不动）
    domain.password_history = existing.password_history.clone();

    // 2. password 变化（且旧值非空）→ 追加
    if let (Some(old_pwd), Some(new_pwd)) = (old_password, new_password) {
        if old_pwd != new_pwd && !old_pwd.is_empty() {
            use octopus_vault::types::PasswordHistoryEntry;
            domain.password_history.push(PasswordHistoryEntry {
                password: old_pwd.to_string(),
                last_used_at: existing.updated_at.clone(),
            });
            // 3. FIFO 截断
            if domain.password_history.len() > PASSWORD_HISTORY_MAX {
                let excess = domain.password_history.len() - PASSWORD_HISTORY_MAX;
                domain.password_history.drain(0..excess);
            }
        }
    }

    domain
}

