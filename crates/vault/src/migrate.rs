//! 一次性迁移：把 models.secret_key 的明文 API Key 用 app_key 加密回写。
//!
//! 触发时机：首次 setup_vault 之后。
//! 规则：
//!   - 仅处理 is_local=0（云端 API Key）的行
//!   - 跳过已 v1: 开头的行（避免重复加密）
//!   - 迁移后字段以 v1: 前缀存密文

use anyhow::Result;
use octopus_infra::db;

use crate::crypto::DerivedKey;

/// 迁移所有未加密的 secret_key。返回迁移的行数。
pub fn migrate_secret_keys_to_encrypted(app_key: &DerivedKey) -> Result<usize> {
    let models = db::list_models_for_secret_migration()?;
    let mut count = 0usize;
    for (model_id, plaintext) in models {
        let encrypted = app_key.encrypt(plaintext.as_bytes())?;
        db::update_model_secret_key(model_id, &encrypted)?;
        count += 1;
        log::info!("迁移 model {} 的 secret_key 为加密格式", model_id);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_compiles() {
        // 仅签名测试，不真正调用 DB
        let _ = std::any::TypeId::of::<fn(&DerivedKey) -> Result<usize>>();
    }
}
