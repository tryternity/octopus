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

use anyhow::{bail, ensure, Context, Result};
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
    // 复审 #1 修复：后端主密码强度校验（INV-10 / §7.4 / F19），防前端绕过
    crate::validate::validate_master_password(password)?;

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
    //
    // #5 修复：旧实现 `Err(e) => log::warn!` 吞错——迁移半成功（事务化前会留下
    // 半迁移状态），用户看到的是"初始化成功"但实际 API Key 仍明文，setup_vault
    // 的 `ensure!(!is_initialized())` 又阻止重跑，无重试入口。
    // 现迁移已事务化（migrate.rs #5 修复），成功=全完成，失败=完全未动 DB——
    // 直接 bubble 让用户看到"初始化失败"而非静默半迁移。
    //
    // **A1 修复（第五轮审查）**：迁移失败时必须回滚 vault_meta——否则 vault_meta
    // 已落盘 + secret_key 仍全明文 + `ensure!(!is_initialized())` 阻止重跑 →
    // 不可恢复的「已初始化但全明文」状态。`save_vault_meta` 在迁移之前已独立
    // commit（不同表无法廉价合并到同一事务，强行合并需重构 meta 模块所有写路径），
    // 故采用「失败时显式 DELETE vault_meta」的对称回滚：让 is_initialized() 回到
    // false，用户可重新走 setup。即使 DELETE 本身失败也不掩盖迁移错误——
    // 返回的 Err 同时报告「迁移失败 + 回滚失败」，让用户/开发者知道需手动介入。
    match crate::migrate::migrate_secret_keys_to_encrypted(&app_key) {
        Ok(n) if n > 0 => log::info!("已迁移 {} 个 model 的 secret_key 为加密格式", n),
        Ok(_) => log::debug!("无明文 secret_key 需迁移"),
        Err(migrate_err) => {
            // 迁移失败 → 显式回滚 vault_meta，恢复 setup 可重试
            if let Err(rollback_err) = octopus_infra::db::delete_vault_meta_row() {
                return Err(migrate_err.context(format!(
                    "secret_key 迁移失败，且回滚 vault_meta 也失败（需手动清 DB）：{rollback_err}"
                )));
            }
            return Err(migrate_err.context("secret_key 迁移失败（已回滚 vault_meta，可重试 setup）"));
        }
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
///
/// **暴力破解防护**（复审 #3 修复，spec §7.3）：失败时调 attempt_guard 退避，
/// 在退避窗口内返 `Err`（"请等待 N 秒后重试"）。
pub fn unlock_with_master_password(password: &str) -> Result<UnlockedKeys> {
    // 退避检查（spec §7.3）——在退避窗口内直接拒绝
    if let Some(wait) = crate::attempt_guard::guard().remaining_wait() {
        bail!("尝试过于频繁，请等待 {} 秒后重试", wait.as_secs());
    }

    let meta = meta::read_vault_meta()
        .context("读取 vault_meta 失败")?
        .context("vault 未初始化")?;
    let params = meta_to_kdf_params(&meta);
    let master_root_key = derive_master_root_key(password.as_bytes(), &meta.kdf_salt, &params)?;

    // 解 user_vault_key + app_key（密码校验阶段）。
    //
    // B2 修复（第五轮审查）：原实现把「密码校验（decrypt）」与「refresh 副作用
    // （写 local_enc）」捆绑在闭包里，match result 在任意 Err 时 record_failure()。
    // 流程 C 写 local_enc 失败（save_vault_meta DB 错 / Keychain 错）会让正确密码
    // 被判失败 + 退避，且整个 unlock 返 Err（用户输对密码却解锁失败）。
    //
    // 新实现分两阶段：
    //   1. 密码校验阶段：仅做 decrypt，失败 = 密码错 → record_failure
    //   2. 副作用阶段（refresh）：失败不调 record_failure（密码是对的），但仍返 Err。
    //      unlock 本身确实失败（用户没拿到有效 session），但下一次立即重试不会被挡。
    let auth_result = (|| -> Result<UnlockedKeys> {
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

        Ok(UnlockedKeys {
            user_vault_key,
            app_key,
        })
    })();

    let keys = match auth_result {
        Ok(k) => {
            crate::attempt_guard::guard().reset(); // 密码校验成功 → 重置计数
            k
        }
        Err(e) => {
            crate::attempt_guard::guard().record_failure(); // 密码错 → 计数 + 退避
            // 加 context 让 desktop::vault_error::classify 识别为 InvalidMasterPassword。
            // 裸 decrypt 错误文案是 "AES-256-GCM 解密失败：密文可能已损坏或 key 不匹配"，
            // classify 兜底为 InternalError → 用户看到"内部错误"而非"主密码错误"。
            // 与 change_master_password:276 对称（那里用 .context("旧主密码错误")）。
            return Err(e.context("主密码错误"));
        }
    };

    // 流程 C 特有：用本机 K_machine 重新加密 app_key → 落盘
    // 这样下次本机启动就能用流程 B 无感。
    //
    // 密码已校验通过，此处失败属"副作用失败"——不调 record_failure（密码是对的），
    // 但仍返 Err（unlock 未完成）。这样用户可立即重试不被退避挡。
    if let Err(e) = refresh_app_key_local_enc(&keys.app_key) {
        // 已 reset 过 guard，不再因副作用失败而污染退避计数
        return Err(e.context("密码正确但刷新 app_key_local_enc 失败"));
    }

    // refresh 后 keys 不变（refresh 仅改 vault_meta，不改 in-memory 派生 key）
    Ok(keys)
}

/// 流程 E：改主密码。
///
/// 副作用：重写 3 个密文 + 刷新 security_stamp。
/// 不重加密 vault_ciphers（因为 user_vault_key 不变）。
///
/// 返回：解出来的 `UnlockedKeys`（user_vault_key + app_key，均不变），
/// 让调用方（desktop）能直接刷 session，避免「先 lock 再改密码」后无法继续用。
/// （follow-up #3）
pub fn change_master_password(old_password: &str, new_password: &str) -> Result<UnlockedKeys> {
    // 复审 #1 修复：新主密码必须强度达标（INV-10），防前端绕过
    crate::validate::validate_master_password(new_password)?;
    // 退避检查（复审 #3）——改密旧密码校验也受 guard 保护
    if let Some(wait) = crate::attempt_guard::guard().remaining_wait() {
        bail!("尝试过于频繁，请等待 {} 秒后重试", wait.as_secs());
    }
    // 修复 #4：加 meta 写锁，串行化 read-modify-write 整段。
    // 防止并发调用（双 modal / Tauri 同步命令并发）导致整行覆盖丢失其他字段。
    let _guard = crate::meta_lock::acquire_meta_write_lock();

    let meta = meta::read_vault_meta()
        .context("读取 vault_meta 失败")?
        .context("vault 未初始化")?;
    let old_params = meta_to_kdf_params(&meta);
    let old_master = derive_master_root_key(old_password.as_bytes(), &meta.kdf_salt, &old_params)?;

    // 验证旧密码（用 protected_user_vault_key 解密试一下）——失败时 record_failure
    let user_vault_bytes = match old_master.decrypt(&meta.protected_user_vault_key) {
        Ok(b) => b,
        Err(e) => {
            crate::attempt_guard::guard().record_failure();
            return Err(e.context("旧主密码错误"));
        }
    };
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

    // B1 修复（第五轮审查）：成功改密后必须 reset guard——与 unlock 成功路径
    // (`:201`) 对称。否则用户连续输错旧密码几次后改密成功，退避计数仍累计，
    // 下次 vault_unlock 被 remaining_wait() 挡（"尝试过于频繁"）。
    crate::attempt_guard::guard().reset();

    // user_vault_key / app_key 在改密码流程中不变（INV-7），原样返回让 caller 刷 session。
    Ok(UnlockedKeys {
        user_vault_key,
        app_key,
    })
}

/// 用本机 K_machine 重新加密 app_key → 写入 app_key_local_enc。
/// 用于流程 C 末尾，让本机下次启动可走流程 B。
fn refresh_app_key_local_enc(app_key: &DerivedKey) -> Result<()> {
    // 修复 #4：与 change_master_password 共享 meta 写锁（两者都改 vault_meta）。
    // 串行化防止 read-modify-write 整行覆盖交错导致数据损坏。
    let _guard = crate::meta_lock::acquire_meta_write_lock();

    let k_machine = match keychain::load_machine_key()? {
        Some(k) => k,
        None => keychain::load_or_create_machine_key()?,
    };
    let k_machine_derived = DerivedKey::from_raw(*k_machine);

    let meta = meta::read_vault_meta()
        .context("读取 vault_meta 失败")?
        .context("vault 未初始化")?;

    // #8 修复：流程 D 无条件重写 app_key_local_enc 是冗余写。
    //
    // 此函数被 `unlock_with_master_password` 调用，而后者同时供流程 C（换机/
    // K_machine 丢失）和流程 D（超时重解）共用。spec §2.6 流程 D 触发条件是
    // 「超时后用户主动重新输主密码」——K_machine / app_key 都没变，刷新是多余的，
    // 还会让每次超时多一次 SQLite UPDATE + WAL fsync。
    //
    // 锁内"解密比较"（spec 中"先比对再决定"原案的语义实现）：
    //   - 用本机 K_machine 解 meta.app_key_local_enc
    //   - 解出来的字节 == 当前 app_key 字节 → K_machine 没变 + app_key 没变
    //     → 跳过 save（流程 D 常见情况）
    //   - 否则 → 流程 C（K_machine 换了 / app_key 换了 / 旧 local_enc 损坏），
    //     走原路径重写
    //
    // 不采用"加密后 == meta.app_key_local_enc"的字符串比较：AES-GCM 加密含随机
    // nonce（见 crypto/symmetric.rs），同样的 (K_machine, app_key) 每次加密得到
    // 不同密文 → 该比较永不为真。必须解密后比较明文字节。
    let skip_save = matches!(&k_machine_derived.decrypt(&meta.app_key_local_enc), Ok(b)
        if b.as_slice() == app_key.as_bytes());
    if skip_save {
        log::debug!(
            "refresh_app_key_local_enc: app_key_local_enc 已能用 K_machine 解出当前 app_key（流程 D 常见情况），跳过 save"
        );
        return Ok(());
    }

    let new_local_enc = k_machine_derived.encrypt(app_key.as_bytes())?;
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

/// 仅校验主密码是否正确，不做解锁副作用。
///
/// 用于二次验证场景（如 reprompt 保护的高敏感 cipher 自动填充）：调用方已经
/// 解锁过 vault（有 user_vault_key 在 session 里），但需要再次确认用户是
/// 真正的主人才能执行高敏感操作。
///
/// 实现与 [`unlock_with_master_password`] 共享前半段——派生 master_root_key +
/// 尝试解密 protected_user_vault_key；解密成功（AES-GCM tag 校验通过）即密码正确。
///
/// 返回 `Ok(())` 表示密码正确，`Err(...)` 表示密码错或 vault 异常。
pub fn verify_master_password(password: &str) -> Result<()> {
    // 退避检查（复审 #3）——reprompt 场景的二次密码验证同样受 guard 保护
    if let Some(wait) = crate::attempt_guard::guard().remaining_wait() {
        bail!("尝试过于频繁，请等待 {} 秒后重试", wait.as_secs());
    }
    let meta = meta::read_vault_meta()
        .context("读取 vault_meta 失败")?
        .context("vault 未初始化")?;
    let params = meta_to_kdf_params(&meta);
    let master_root_key = derive_master_root_key(password.as_bytes(), &meta.kdf_salt, &params)?;
    // 尝试解密——AES-GCM tag 校验失败即密码错
    match master_root_key.decrypt(&meta.protected_user_vault_key) {
        Ok(_) => {
            // verify 不调 reset()——它只是二次确认，不该清掉 unlock 路径的计数
            Ok(())
        }
        Err(e) => {
            crate::attempt_guard::guard().record_failure();
            // 加 context 让 classify 识别为 InvalidMasterPassword（见 unlock_with_master_password 同款修复）
            Err(e.context("主密码错误"))
        }
    }
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
        // reset attempt guard 防跨测试串扰（一个测试先试错密码触发退避，下个测试被挡）
        crate::attempt_guard::guard().reset();
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

        // 注：密码需满足后端 validate_master_password 强度要求（≥12 + 4 类）
        let keys = setup_vault("Test-password-123!").expect("setup_vault");
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

        let pw = "Correct-horse-battery-staple-1!";
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

        let _ = setup_vault("Right-password-1!").expect("setup");
        let _ = keychain::delete_machine_key();

        let result = unlock_with_master_password("wrong-password");
        assert!(result.is_err(), "错误主密码应解密失败");

        let _ = keychain::delete_machine_key();
    }

    // === Follow-up #3: change_master_password 返回 UnlockedKeys ===

    /// setup → unlock → change_master_password → 返回的 user_vault_key
    /// 应与 setup 派生的同一把（INV-7：改密码不改 user_vault_key）。
    #[test]
    fn test_change_master_password_returns_same_keys() {
        setup_clean_db();
        let _ = keychain::delete_machine_key();

        let old_pw = "Old-master-password-1!";
        let new_pw = "New-master-password-2!";

        let setup_keys = setup_vault(old_pw).expect("setup");
        let _ = keychain::delete_machine_key();

        let keys = change_master_password(old_pw, new_pw).expect("change password");
        // user_vault_key / app_key 不应随 master 变化
        assert_eq!(
            keys.user_vault_key.as_bytes(),
            setup_keys.user_vault_key.as_bytes(),
            "改密码后 user_vault_key 应不变"
        );
        assert_eq!(
            keys.app_key.as_bytes(),
            setup_keys.app_key.as_bytes(),
            "改密码后 app_key 应不变"
        );

        let _ = keychain::delete_machine_key();
    }

    /// 改密码后旧密码应解不开，新密码应能解开 → 验证 master_root_key 确实换了。
    #[test]
    fn test_change_master_password_swaps_master_key() {
        setup_clean_db();
        let _ = keychain::delete_machine_key();

        let setup_keys = setup_vault("Pw-old-pw-1!").expect("setup");
        let _ = keychain::delete_machine_key();

        change_master_password("Pw-old-pw-1!", "Pw-new-pw-2!").expect("change");

        // 旧密码应失败
        let _ = keychain::delete_machine_key();
        assert!(
            unlock_with_master_password("Pw-old-pw-1!").is_err(),
            "旧主密码改密后应解不开"
        );
        // reset guard——测试环境无真实退避需求，避免挡住下面的成功路径
        crate::attempt_guard::guard().reset();
        // 新密码应成功，且拿到同一把 user_vault_key
        let unlocked = unlock_with_master_password("Pw-new-pw-2!").expect("new pw unlocks");
        assert_eq!(
            unlocked.user_vault_key.as_bytes(),
            setup_keys.user_vault_key.as_bytes(),
            "新主密码解出的 user_vault_key 应与 setup 一致"
        );

        let _ = keychain::delete_machine_key();
    }

    /// change_master_password 用错误旧密码应失败。
    #[test]
    fn test_change_master_password_wrong_old_fails() {
        setup_clean_db();
        let _ = keychain::delete_machine_key();

        let _ = setup_vault("Correct-old-1!").expect("setup");
        let _ = keychain::delete_machine_key();

        let result = change_master_password("wrong-old", "Anything-new-2!");
        assert!(result.is_err(), "错误旧密码应导致 change 失败");

        let _ = keychain::delete_machine_key();
    }

    // === 修复 #8: 流程 D 不应冗余重写 app_key_local_enc ===

    /// 流程 D（超时重解，K_machine 没变）连续调用 unlock_with_master_password，
    /// 第二次应短路跳过 save——app_key_local_enc 字段保持不变。
    ///
    /// 思路：
    ///   1. setup_vault 初始化（写入第一份 app_key_local_enc）
    ///   2. 流程 C：delete_machine_key 模拟换机 → unlock 一次 → refresh 写入
    ///      用「流程 C 后的 K_machine」加密的 app_key_local_enc
    ///   3. 流程 D：K_machine 不变 → unlock 第二次 → 此时应短路不写
    ///
    /// 断言：步骤 2 末尾读到的 app_key_local_enc 字符串 == 步骤 3 末尾读到的字符串
    /// （若 #8 未修复，每次 refresh 都会用随机 nonce 生成新密文 → 两次必然不同）。
    #[test]
    fn test_flow_d_skips_redundant_app_key_local_enc_write() {
        setup_clean_db();
        let _ = keychain::delete_machine_key();

        let pw = "Test-password-1!";
        setup_vault(pw).expect("setup");

        // 流程 C：清 K_machine 模拟换机 → unlock 后 refresh 会重写 local_enc
        //   （旧 local_enc 用 setup 时的 K_machine 加密；现在 K_machine 全新，
        //    解开解不出当前 app_key → 不短路 → 重写）
        let _ = keychain::delete_machine_key();
        unlock_with_master_password(pw).expect("flow C unlock");
        let local_enc_after_flow_c = meta::read_vault_meta()
            .expect("read meta")
            .expect("vault_meta should exist")
            .app_key_local_enc;

        // 流程 D：K_machine 不变（沿用流程 C 末尾 keychain 里的 key）→ unlock 第二次
        //   → refresh 应短路跳过 save
        unlock_with_master_password(pw).expect("flow D unlock");
        let local_enc_after_flow_d = meta::read_vault_meta()
            .expect("read meta")
            .expect("vault_meta should exist")
            .app_key_local_enc;

        assert_eq!(
            local_enc_after_flow_c, local_enc_after_flow_d,
            "#8 修复：流程 D 不应重写 app_key_local_enc——两值应字节一致"
        );

        let _ = keychain::delete_machine_key();
    }

    // === 第五轮审查修复测试 ===

    /// B1（第五轮审查）：连续输错旧密码 → 等退避窗口过后改密成功，guard 必须 reset。
    ///
    /// 旧实现：失败路径 `record_failure()` 累计计数，但成功路径无 `reset()` →
    /// 连续输错几次后改密成功，下次 vault_unlock 被 `remaining_wait()` 挡。
    /// 修复：`change_master_password` 成功路径也调 `reset()`。
    ///
    /// 测试时序：
    ///   1. 错旧密码 3 次 → 累计失败计数（delay=0/1/2s）
    ///   2. 等待退避窗口过去（最多 2s）
    ///   3. 用正确旧密码改密成功 → 应 reset
    ///   4. 立即再试 unlock 不会被退避挡
    #[test]
    fn test_change_password_resets_guard_on_success() {
        setup_clean_db();
        let _ = keychain::delete_machine_key();

        let old_pw = "Old-password-1!";
        let new_pw = "New-password-2!";
        let _ = setup_vault(old_pw).expect("setup");
        let _ = keychain::delete_machine_key();

        // 连续输错旧密码触发退避（第 3 次失败 delay=2s）
        for _ in 0..3 {
            // 错旧密码会立即 record_failure（前 2 次 delay=0/1，remaining_wait 暂不挡）
            // 第 3+ 次调用一开始就被 remaining_wait 挡 bail，但 record_failure 已累计
            let _ = change_master_password("wrong-old-pw-WITH-STRENGTH-1!", new_pw).err();
        }
        // 等待最长退避窗口过去（BACKOFF_SECS[2]=2s，留足余量到 3s）
        std::thread::sleep(std::time::Duration::from_secs(3));

        // 用正确旧密码改密成功——B1 修复后成功路径 reset guard
        change_master_password(old_pw, new_pw).expect("change with correct old pw");

        assert!(
            crate::attempt_guard::guard().remaining_wait().is_none(),
            "B1 修复：改密成功后 guard 应被 reset，不再有退避窗口"
        );

        // 立即用新密码 unlock 应能成功（验证 reset 后不被挡）
        let _ = unlock_with_master_password(new_pw).expect("unlock with new pw should work");

        let _ = keychain::delete_machine_key();
    }

    /// B2（第五轮审查）：refresh_app_key_local_enc 失败时**不**应污染退避计数。
    ///
    /// 旧实现：闭包把「密码校验」与「副作用」捆绑，任意 Err 都 record_failure()。
    /// 修复：密码校验通过后 reset，副作用失败仅返 Err 不调 record_failure。
    ///
    /// 难以在当前 in-memory 测试基础设施下注入 refresh 失败（Keychain / save_vault_meta
    /// 都不会失败），故此处做"正向可达"覆盖：unlock 成功后 guard 应是已 reset 状态。
    /// 退避污染的回归断言靠"连续失败 + 成功解锁后 remaining_wait 应为 None"——
    /// 与 B1 测试结构对称。
    #[test]
    fn test_unlock_success_clears_guard_after_wrong_attempts() {
        setup_clean_db();
        let _ = keychain::delete_machine_key();

        let pw = "Correct-password-1!";
        let _ = setup_vault(pw).expect("setup");
        let _ = keychain::delete_machine_key();

        // 连续输错密码触发退避
        for _ in 0..3 {
            let _ = unlock_with_master_password("wrong-pw").err();
        }
        // 等待退避窗口过去（delay 序列第 3 次 = 2s，sleep 等 3s 保证已过）
        std::thread::sleep(std::time::Duration::from_secs(3));

        // 用正确密码解锁——应成功且 reset
        let _ = unlock_with_master_password(pw).expect("unlock with correct pw");
        assert!(
            crate::attempt_guard::guard().remaining_wait().is_none(),
            "B2 修复：正确密码解锁后 guard 应被 reset（副作用失败不污染）"
        );

        let _ = keychain::delete_machine_key();
    }

    /// A1（第五轮审查）：`delete_vault_meta_row` 显式回滚函数的单元测试。
    ///
    /// 验证 setup 失败路径的回滚能力——若迁移失败，setup_vault 调用此函数
    /// 让 is_initialized() 回到 false，用户可重新走 setup。
    /// 此处直接测 `delete_vault_meta_row` 的语义：save → is_initialized=true →
    /// delete → is_initialized=false。
    #[test]
    fn test_delete_vault_meta_row_resets_is_initialized() {
        setup_clean_db();

        // 全新库 → 未初始化
        assert!(!is_initialized().expect("fresh DB"));

        // 写一行 vault_meta → 已初始化
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
        meta::save_vault_meta(&input).expect("save");
        assert!(is_initialized().expect("after save"));

        // delete → 回到未初始化（A1 修复的核心语义）
        octopus_infra::db::delete_vault_meta_row().expect("delete vault_meta");
        assert!(
            !is_initialized().expect("after delete"),
            "A1 修复：delete_vault_meta_row 应让 is_initialized 回到 false"
        );

        // 再次 save（模拟用户重新走 setup）应能成功——无残留
        meta::save_vault_meta(&input).expect("save again");
        assert!(is_initialized().expect("after re-save"));
    }
}
