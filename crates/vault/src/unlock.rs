//! vault 解锁态管理。
//!
//! 5 大流程（spec 第 2.5 节）：
//!   A. setup_vault              - 首次初始化（设主密码）
//!   B. unlock_app_key_local     - 本机启动（K_machine 解 app_key_local_enc）
//!   C. unlock_with_master_password - 换机器首次 / K_machine 缺失（输主密码）
//!   D. unlock_with_master_password - 超时锁定后重新解锁
//!   E. change_master_password   - 改主密码
//!
//! 流程 C 和 D 共享 unlock_with_master_password 函数（区别在调用 context）。
//!
//! 双密文 Y+ 方案：
//!   - app_key 被 AES-GCM 加密两次，落盘两条密文
//!     * app_key_local_enc：用 K_machine（OS Keychain）加密 → 本机无感启动
//!     * app_key_sync_enc ：用 master_root_key 加密 → 跨机器同步用
//!   - 换机器场景：K_machine 不一致 → app_key_local_enc 解不开 → 用户输主密码
//!     走流程 C 解 app_key_sync_enc，成功后用本机 K_machine 重写 local 密文

use anyhow::{ensure, Context, Result};
use uuid::Uuid;

use octopus_infra::db::{VaultMeta, VaultMetaInput};

use crate::crypto::hierarchy::{LABEL_APP_SECRETS, LABEL_USER_VAULT};
use crate::crypto::kdf::{derive_master_root_key, Argon2Params};
use crate::crypto::util::random_32;
use crate::crypto::DerivedKey;
use crate::keychain;
use crate::storage::meta;

/// 解锁态：持有派生的 user_vault_key 和 app_key（均 32B Zeroizing）。
pub struct UnlockedKeys {
    pub user_vault_key: DerivedKey,
    pub app_key: DerivedKey,
}

/// vault 状态摘要（用于 UI 显示）。
pub struct VaultStatus {
    pub initialized: bool,
    pub user_vault_unlocked: bool, // 由调用方（desktop）维护，此处恒为 false
}

pub fn is_initialized() -> Result<bool> {
    Ok(meta::read_vault_meta()?.is_some())
}

fn meta_to_kdf_params(meta: &VaultMeta) -> Argon2Params {
    Argon2Params {
        iterations: meta.kdf_iterations as u32,
        memory_kib: meta.kdf_memory_kib as u32,
        parallelism: meta.kdf_parallelism as u32,
    }
}

/// 流程 A：首次初始化 vault。
///
/// 输入：用户设的主密码（明文，调用后立即 zeroize 由调用者负责）。
/// 副作用：
///   - 生成 32B kdf_salt
///   - 派生 master_root_key，进一步派生 user_vault_key 和 app_key
///   - 生成 K_machine（OS Keychain）
///   - 双密文 app_key 落盘
///   - 落盘 vault_meta
///   - 不迁移 models.secret_key（迁移由 Task 20 单独负责）
pub fn setup_vault(password: &str) -> Result<UnlockedKeys> {
    ensure!(!is_initialized()?, "vault 已初始化");

    let kdf_salt = random_32();
    let params = Argon2Params::default();
    let master_root_key = derive_master_root_key(password.as_bytes(), &kdf_salt, &params)?;

    // 派生 user_vault_key / app_key
    let user_vault_key = master_root_key.child(LABEL_USER_VAULT);
    let app_key = master_root_key.child(LABEL_APP_SECRETS);

    // 加密 user_vault_key（用 master_root_key）
    let protected_user_vault_key = master_root_key.encrypt(user_vault_key.as_bytes())?;
    // 加密 app_key（双密文）
    let k_machine = keychain::load_or_create_machine_key()?;
    let app_key_local_enc = {
        let k_machine_derived = DerivedKey::from_raw(*k_machine);
        k_machine_derived.encrypt(app_key.as_bytes())?
    };
    let app_key_sync_enc = master_root_key.encrypt(app_key.as_bytes())?;

    let stamp = Uuid::new_v4().to_string();

    let input = VaultMetaInput {
        kdf_type: 0,
        kdf_salt: kdf_salt.to_vec(),
        kdf_iterations: params.iterations as i64,
        kdf_memory_kib: params.memory_kib as i64,
        kdf_parallelism: params.parallelism as i64,
        protected_user_vault_key,
        app_key_local_enc,
        app_key_sync_enc,
        security_stamp: stamp,
        equivalent_domains: "[]".into(),
        public_key: None,
        protected_private_key: None,
    };
    meta::save_vault_meta(&input)?;

    // 注意：Task 20 将在此处插入 migrate_secret_keys_to_encrypted(&app_key) 调用，
    // 用于把 models.secret_key 从明文迁移到 app_key 加密。当前 Task 9 暂不实现。

    Ok(UnlockedKeys {
        user_vault_key,
        app_key,
    })
}

/// 流程 B：本机启动时尝试用 K_machine 解 app_key（无感）。
///
/// 返回：
///   - Ok(Some(app_key))：成功解出 app_key，应用可用 ASR
///   - Ok(None)：vault 未初始化 / K_machine 不存在 / 解密失败 → 调用方应走流程 C
pub fn unlock_app_key_local() -> Result<Option<DerivedKey>> {
    let meta = match meta::read_vault_meta()? {
        Some(m) => m,
        None => return Ok(None),
    };
    let k_machine = match keychain::load_machine_key()? {
        Some(k) => k,
        None => return Ok(None),
    };
    let k_machine_derived = DerivedKey::from_raw(*k_machine);
    match k_machine_derived.decrypt(&meta.app_key_local_enc) {
        Ok(bytes) => {
            ensure!(
                bytes.len() == 32,
                "app_key 解密后长度异常：{}",
                bytes.len()
            );
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Ok(Some(DerivedKey::from_raw(arr)))
        }
        Err(_) => Ok(None), // 解密失败 → 降级到流程 C
    }
}

/// 流程 C/D：用主密码解锁（换机器 / 超时锁定后）。
///
/// 同时解开 user_vault_key 和 app_key。
/// 成功后调用方可选择用本机 K_machine 重新加密 app_key 落盘（流程 C 末尾）。
pub fn unlock_with_master_password(password: &str) -> Result<UnlockedKeys> {
    let meta = meta::read_vault_meta()
        .context("读取 vault_meta 失败")?
        .context("vault 未初始化")?;
    let params = meta_to_kdf_params(&meta);
    let master_root_key = derive_master_root_key(password.as_bytes(), &meta.kdf_salt, &params)?;

    // 解 user_vault_key
    let user_vault_bytes = master_root_key.decrypt(&meta.protected_user_vault_key)?;
    ensure!(user_vault_bytes.len() == 32, "user_vault_key 长度异常");
    let mut uv_arr = [0u8; 32];
    uv_arr.copy_from_slice(&user_vault_bytes);
    let user_vault_key = DerivedKey::from_raw(uv_arr);

    // 解 app_key（用 sync 密文）
    let app_key_bytes = master_root_key.decrypt(&meta.app_key_sync_enc)?;
    ensure!(app_key_bytes.len() == 32, "app_key 长度异常");
    let mut ak_arr = [0u8; 32];
    ak_arr.copy_from_slice(&app_key_bytes);
    let app_key = DerivedKey::from_raw(ak_arr);

    // 流程 C 特有：用本机 K_machine 重新加密 app_key → 落盘
    // 这样下次本机启动就能用流程 B 无感
    refresh_app_key_local_enc(&app_key)?;

    Ok(UnlockedKeys {
        user_vault_key,
        app_key,
    })
}

/// 流程 E：改主密码。
///
/// 副作用：重写 3 个密文 + 刷新 security_stamp。
/// 不重加密 vault_ciphers（因为 user_vault_key 不变）。
pub fn change_master_password(old_password: &str, new_password: &str) -> Result<()> {
    let meta = meta::read_vault_meta()
        .context("读取 vault_meta 失败")?
        .context("vault 未初始化")?;
    let old_params = meta_to_kdf_params(&meta);
    let old_master = derive_master_root_key(old_password.as_bytes(), &meta.kdf_salt, &old_params)?;

    // 验证旧密码（用 protected_user_vault_key 解密试一下）
    let user_vault_bytes = old_master
        .decrypt(&meta.protected_user_vault_key)
        .context("旧主密码错误")?;
    ensure!(user_vault_bytes.len() == 32, "user_vault_key 长度异常");
    let mut uv_arr = [0u8; 32];
    uv_arr.copy_from_slice(&user_vault_bytes);
    let user_vault_key = DerivedKey::from_raw(uv_arr);

    // 用旧 master 解出 app_key
    let app_key_bytes = old_master.decrypt(&meta.app_key_sync_enc)?;
    ensure!(app_key_bytes.len() == 32, "app_key 长度异常");
    let mut ak_arr = [0u8; 32];
    ak_arr.copy_from_slice(&app_key_bytes);
    let app_key = DerivedKey::from_raw(ak_arr);

    // 用新密码派生新 master_root_key
    let new_master = derive_master_root_key(new_password.as_bytes(), &meta.kdf_salt, &old_params)?;

    // 重加密 3 个密文
    let new_protected_user_vault_key = new_master.encrypt(user_vault_key.as_bytes())?;
    let new_app_key_sync_enc = new_master.encrypt(app_key.as_bytes())?;
    let new_app_key_local_enc = {
        let k_machine = keychain::load_or_create_machine_key()?;
        let k_machine_derived = DerivedKey::from_raw(*k_machine);
        k_machine_derived.encrypt(app_key.as_bytes())?
    };

    // 刷新 security_stamp（让其他机器同步后强制重新输主密码）
    let new_stamp = Uuid::new_v4().to_string();

    let input = VaultMetaInput {
        kdf_type: meta.kdf_type,
        kdf_salt: meta.kdf_salt.clone(),
        kdf_iterations: meta.kdf_iterations,
        kdf_memory_kib: meta.kdf_memory_kib,
        kdf_parallelism: meta.kdf_parallelism,
        protected_user_vault_key: new_protected_user_vault_key,
        app_key_local_enc: new_app_key_local_enc,
        app_key_sync_enc: new_app_key_sync_enc,
        security_stamp: new_stamp,
        equivalent_domains: meta.equivalent_domains,
        public_key: meta.public_key,
        protected_private_key: meta.protected_private_key,
    };
    meta::save_vault_meta(&input)?;
    Ok(())
}

/// 用本机 K_machine 重新加密 app_key → 写入 app_key_local_enc。
/// 用于流程 C 末尾，让本机下次启动可走流程 B。
fn refresh_app_key_local_enc(app_key: &DerivedKey) -> Result<()> {
    let k_machine = match keychain::load_machine_key()? {
        Some(k) => k,
        None => keychain::load_or_create_machine_key()?,
    };
    let k_machine_derived = DerivedKey::from_raw(*k_machine);
    let new_local_enc = k_machine_derived.encrypt(app_key.as_bytes())?;

    let meta = meta::read_vault_meta()
        .context("读取 vault_meta 失败")?
        .context("vault 未初始化")?;
    let input = VaultMetaInput {
        kdf_type: meta.kdf_type,
        kdf_salt: meta.kdf_salt.clone(),
        kdf_iterations: meta.kdf_iterations,
        kdf_memory_kib: meta.kdf_memory_kib,
        kdf_parallelism: meta.kdf_parallelism,
        protected_user_vault_key: meta.protected_user_vault_key,
        app_key_local_enc: new_local_enc,
        app_key_sync_enc: meta.app_key_sync_enc,
        security_stamp: meta.security_stamp,
        equivalent_domains: meta.equivalent_domains,
        public_key: meta.public_key,
        protected_private_key: meta.protected_private_key,
    };
    meta::save_vault_meta(&input)?;
    Ok(())
}

/// 仅刷新 security_stamp 并返回新值。
pub fn regenerate_security_stamp() -> Result<String> {
    let new_stamp = Uuid::new_v4().to_string();
    meta::update_security_stamp(&new_stamp)?;
    Ok(new_stamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uninitialized_vault_status() {
        // 注意：依赖 ~/.octopus/octopus.db 实际状态
        // 这里仅测函数签名和错误处理，不真正调用 setup_vault
        let _ = is_initialized();
    }
}
