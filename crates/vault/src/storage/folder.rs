//! vault_folders 表的薄包装（MVP UI 不暴露，但提供 API）。

use anyhow::Result;
use octopus_infra::db::{self, VaultFolder};

pub fn list_folders() -> Result<Vec<VaultFolder>> {
    Ok(db::list_vault_folders()?)
}

/// 注意：name 应由调用者先用 user_vault_key.encrypt() 加密后再传入。
/// MVP UI 不使用，故不在 storage 层做加密。
pub fn create_folder(name_encrypted: &str) -> Result<i64> {
    Ok(db::insert_vault_folder(name_encrypted)?)
}
