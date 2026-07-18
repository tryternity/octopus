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
///   - 一次性迁移现有明文 models.secret_key 为 app_key 加密格式（Task 20）
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

    // 一次性迁移现有明文 secret_key（仅首次 init vault 时触发）
    match crate::migrate::migrate_secret_keys_to_encrypted(&app_key) {
        Ok(n) if n > 0 => log::info!("已迁移 {} 个 model 的 secret_key 为加密格式", n),
        Ok(_) => log::debug!("无明文 secret_key 需迁移"),
        Err(e) => log::warn!("secret_key 迁移失败（不阻塞 setup）: {}", e),
    }

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

    /// 注入干净 in-memory DB（含 vault_meta 表，无数据）+ 干净 in-memory Keychain。
    ///
    /// 同时挂上 thread-local 的 DB 和 Keychain 覆盖，让 setup_vault /
    /// unlock_with_master_password 等原本依赖 OS Keychain 的流程可在 CI / 无
    /// Keychain 环境跑。多个测试互不影响（thread_local 隔离 + 各自空 store）。
    fn setup_clean_db() {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory DB");
        octopus_infra::db::set_test_db(conn);
        keychain::set_test_keychain();
    }

    /// 纯函数测试：meta_to_kdf_params 把 VaultMeta 的 i64 字段映射为 Argon2Params 的 u32 字段。
    #[test]
    fn test_meta_to_kdf_params_conversion() {
        let meta = VaultMeta {
            id: 1,
            kdf_type: 0,
            kdf_salt: vec![0u8; 32],
            kdf_iterations: 3,
            kdf_memory_kib: 65_536,
            kdf_parallelism: 4,
            protected_user_vault_key: String::new(),
            app_key_local_enc: String::new(),
            app_key_sync_enc: String::new(),
            security_stamp: String::new(),
            equivalent_domains: "[]".into(),
            public_key: None,
            protected_private_key: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let params = meta_to_kdf_params(&meta);
        assert_eq!(params.iterations, 3);
        assert_eq!(params.memory_kib, 65_536);
        assert_eq!(params.parallelism, 4);
    }

    /// DB-only 测试：未初始化时 is_initialized 返回 false。
    /// 不触发 Keychain（无 vault_meta 行 → 早返回 None）。
    #[test]
    fn test_is_initialized_false_initially() {
        setup_clean_db();
        assert!(!is_initialized().expect("is_initialized should succeed on empty DB"));
    }

    /// 直接写 vault_meta（不经 setup_vault / Keychain），验证 is_initialized 变为 true。
    /// 这样 DB-only 测试就覆盖了 is_initialized 的「true」分支，无需 OS Keychain。
    #[test]
    fn test_is_initialized_true_after_meta_written() {
        setup_clean_db();
        let input = VaultMetaInput {
            kdf_type: 0,
            kdf_salt: vec![1u8; 32],
            kdf_iterations: 3,
            kdf_memory_kib: 65_536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:dummy".into(),
            app_key_local_enc: "v1:dummy".into(),
            app_key_sync_enc: "v1:dummy".into(),
            security_stamp: "stamp".into(),
            equivalent_domains: "[]".into(),
            public_key: None,
            protected_private_key: None,
        };
        meta::save_vault_meta(&input).expect("save meta");
        assert!(is_initialized().expect("is_initialized should be true after meta saved"));
    }

    /// setup_vault 完整流程：会调 keychain::load_or_create_machine_key，
    /// 走 thread-local in-memory Keychain 覆盖（setup_clean_db 已挂上）。
    #[test]
    fn test_setup_vault_creates_meta_and_keys() {
        setup_clean_db();
        // 清掉可能残留的 Keychain entry（防止之前测试遗留）——
        // 已是 in-memory 覆盖，no-op；保留以防有人误改 setup。
        let _ = keychain::delete_machine_key();

        let keys = setup_vault("test-password-123").expect("setup_vault");
        // 32B 派生 key
        assert_eq!(keys.user_vault_key.as_bytes().len(), 32);
        assert_eq!(keys.app_key.as_bytes().len(), 32);
        // 应不同于对方（不同 label 派生）
        assert_ne!(
            keys.user_vault_key.as_bytes(),
            keys.app_key.as_bytes(),
            "user_vault_key 与 app_key 应是不同 label 派生"
        );
        // vault_meta 已写入 → is_initialized=true
        assert!(is_initialized().expect("is_initialized after setup"));

        // 清理 Keychain（避免污染下次测试或真实用户）
        let _ = keychain::delete_machine_key();
    }

    /// setup_vault + unlock_with_master_password 往返。
    #[test]
    fn test_unlock_with_master_after_setup() {
        setup_clean_db();
        let _ = keychain::delete_machine_key();

        let pw = "correct-horse-battery-staple";
        let setup_keys = setup_vault(pw).expect("setup");
        // 清掉 K_machine 强制走流程 C（输主密码）
        let _ = keychain::delete_machine_key();

        let unlocked = unlock_with_master_password(pw).expect("unlock");
        // 同样派生 → 应拿到同一把 user_vault_key
        assert_eq!(setup_keys.user_vault_key.as_bytes(), unlocked.user_vault_key.as_bytes());
        assert_eq!(setup_keys.app_key.as_bytes(), unlocked.app_key.as_bytes());

        let _ = keychain::delete_machine_key();
    }

    /// unlock_with_master_password 用错误密码应失败（需 setup 后才有 vault_meta）。
    #[test]
    fn test_unlock_wrong_password_fails() {
        setup_clean_db();
        let _ = keychain::delete_machine_key();

        let _ = setup_vault("right-password").expect("setup");
        let _ = keychain::delete_machine_key();

        let result = unlock_with_master_password("wrong-password");
        assert!(result.is_err(), "错误主密码应解密失败");

        let _ = keychain::delete_machine_key();
    }
}
