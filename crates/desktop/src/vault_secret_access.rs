//! secret_key 透明解密 chokepoint（follow-up #7）。
//!
//! Task 20 的迁移把 `models.secret_key`（仅 `is_local=0` 云端行）从明文 API Key
//! 改写为 `v1:<base64(...)>` 加密格式（用 `app_key` 加密）。但消费端（cloud ASR WS 鉴权、
//! LLM polish HTTP Bearer、云端翻译）原样把 DB 里的值当明文 API Key 用——vault 启用后
//! 每次云端推理都会把加密 blob 当 Key 发出去导致 401。
//!
//! 本模块提供统一 chokepoint：
//!
//! - [`read_model_secret_key`]：按 model_name 读 DB secret_key，自动解 `v1:` 前缀
//! - [`try_decrypt_secret`]：对 raw 字符串做条件解密（云端推理热路径用——
//!   `ResolvedEngine.entry.secret_key` 等场景只有字符串、没有 model_name 时调）
//!
//! 设计原则：
//! - 没有 vault session（未初始化 / 启动失败）→ 一律返回 raw 原值（向后兼容）
//! - raw 不以 `v1:` 开头（本地 manifest JSON / 未迁移的明文）→ 原样返回
//! - 仅 `v1:` 前缀的密文走 `app_key.decrypt(...)`；解密失败返回 Err
//!
//! 注意 `is_local=1` 的本地模型 secret_key 是 manifest JSON，永远不应解密——
//! 但本模块按「前缀判定」而非「is_local 判定」，因为：
//! 1. 本地 manifest JSON 不会以 `v1:` 开头（迁移 SQL 含 `is_local=0` 守卫）
//! 2. 调用方已知自己在处理云端 Key（`ResolvedEngine.entry.is_local == false`）

use std::sync::Arc;

use octopus_vault::crypto::symmetric::CIPHERTEXT_PREFIX;
use octopus_vault::crypto::DerivedKey;

use crate::vault_state::SharedVaultSession;

/// 按 model_name 读 DB secret_key，透明解密 `v1:` 前缀。
///
/// 流程：
/// 1. 调 `model_commands::current_secret_key_any(model_name)` 读 DB raw 值（cloud + local）
/// 2. raw 不以 `v1:` 开头（manifest / 未迁移明文）→ 原样返回
/// 3. raw 以 `v1:` 开头但 vault 未初始化 / app_key 不可用 → 返回 Err
///    （让调用方决定是否 fallback——通常显示「请先解锁 vault」）
/// 4. raw 以 `v1:` 开头且 app_key 可用 → `app_key.decrypt(raw)` 返回明文 API Key
pub fn read_model_secret_key(
    model_name: &str,
    session: &SharedVaultSession,
) -> Result<String, String> {
    let raw = crate::model_commands::current_secret_key_any(model_name)?;
    try_decrypt_secret(&raw, session)
}

/// 对 raw secret_key 字符串做条件解密。
///
/// 用于云端推理热路径（`ResolvedEngine.entry.secret_key`、`CompatibleLlmConfig.secret_key`）：
/// 这些场景只有字符串本身、没有 model_name，但已知是云端 API Key（非本地 manifest）。
///
/// 行为：
/// - raw 不以 `v1:` 开头 → 原样返回（未迁移明文 / pre-vault / 空）
/// - raw 以 `v1:` 开头但 vault 未初始化 / app_key 缺失 → 返回 Err
/// - 否则 `app_key.decrypt(raw)` → UTF-8 字符串
///
/// `session` 可通过 [`crate::vault_state::try_global_session`] 在非 Tauri-State
/// 调用点取（AliyunEngine / config::llm_config_ignore_mode 等）。
pub fn try_decrypt_secret(
    raw: &str,
    session: &SharedVaultSession,
) -> Result<String, String> {
    // 不以 v1: 开头 → 不是加密格式，原样返回
    if !raw.starts_with(CIPHERTEXT_PREFIX) {
        return Ok(raw.to_string());
    }

    // 加密格式但 app_key 不可用 → 报错（让调用方提示用户解锁）
    // 复用 vault_commands::require_app_key_from_session 保持单一 chokepoint（follow-up #7）。
    // 该函数现在返回 VaultError（user-safe message）；这里透传其 JSON 序列化形式
    // 给调用方（仍是 Result<_, String>，但内容是稳定的 `{ code, message }`）。
    let app_key: Arc<DerivedKey> = crate::vault_commands::require_app_key_from_session(session)
        .map_err(|e| crate::vault_error::serialize(&e))?;

    // decrypt 失败属内部细节（nonce mismatch / tag 等）——映射为 InternalError 的
    // user-safe message，不透传。
    let plaintext = app_key.decrypt(raw).map_err(|_| {
        crate::vault_error::serialize(&crate::vault_error::VaultError::InternalError)
    })?;
    String::from_utf8(plaintext.to_vec()).map_err(|_| {
        crate::vault_error::serialize(&crate::vault_error::VaultError::InternalError)
    })
}

/// 进程级便捷版：raw 字符串自动用全局 session 解密。
///
/// 用于云端推理热路径（拿不到 Tauri State、也没显式 session 参数）。
/// 全局 session 未注入（main.rs 未启动 / 测试环境）→ raw 原样返回（向后兼容）。
///
/// 注意：与 [`try_decrypt_secret`] 不同，本函数在 vault 已启用但 app_key 不可用时
/// 不返回 Err——而是返回 raw（让上层推理路径继续走，最终由云端 401 报错暴露）。
/// 这样在 vault 未启用 / 启动早期 / 测试环境都不会破坏现有行为。
pub fn try_decrypt_secret_global(raw: &str) -> String {
    match crate::vault_state::try_global_session() {
        Some(session) => match try_decrypt_secret(raw, &session) {
            Ok(plaintext) => plaintext,
            Err(e) => {
                // 解密失败不 panic——记录日志，返回 raw 让上层处理
                // （避免推理热路径因 vault 问题直接挂掉整段录音 / 翻译）
                log::warn!("secret_key 解密失败，回退 raw 值：{}", e);
                raw.to_string()
            }
        },
        // vault 未启用 / 全局 session 未注入 → raw 原样返回（向后兼容）
        None => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_state::{SharedVaultSession, VaultSession};
    use parking_lot::RwLock;
    use std::sync::Arc;

    /// 构造一份确定性的 32B DerivedKey（每个 byte 都为 `byte`）。
    fn make_key(byte: u8) -> Arc<DerivedKey> {
        use octopus_vault::Zeroizing;
        Arc::new(DerivedKey(Zeroizing::new([byte; 32])))
    }

    /// 构造一个空 session（app_key=None），用于测试「vault 已启用但 app_key 不可用」。
    fn empty_session() -> SharedVaultSession {
        Arc::new(RwLock::new(VaultSession::default()))
    }

    /// 构造一个 app_key 已注入的 session。
    fn session_with_app_key(key: Arc<DerivedKey>) -> SharedVaultSession {
        let mut s = VaultSession::default();
        s.app_key = Some(key);
        Arc::new(RwLock::new(s))
    }
    /// 非 v1: 前缀 → 原样返回（明文 API Key / 本地 manifest JSON / 空）。
    #[test]
    fn plaintext_passthrough() {
        let session = empty_session();

        assert_eq!(try_decrypt_secret("", &session).unwrap(), "");
        assert_eq!(
            try_decrypt_secret("sk-plain-api-key", &session).unwrap(),
            "sk-plain-api-key"
        );
        // 本地 manifest JSON 不应被解密
        let manifest = r#"{"version":"1.0","files":[]}"#;
        assert_eq!(try_decrypt_secret(manifest, &session).unwrap(), manifest);
    }

    /// v1: 前缀 + app_key 可用 → 解密返回明文。
    #[test]
    fn encrypted_with_app_key_decrypts() {
        let key = make_key(7);
        let session = session_with_app_key(key.clone());

        let plaintext = "sk-secret-cloud-api-key-42";
        let encrypted = key.encrypt(plaintext.as_bytes()).unwrap();
        assert!(encrypted.starts_with("v1:"));

        let decrypted = try_decrypt_secret(&encrypted, &session).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    /// v1: 前缀 + app_key=None（vault 未解锁 / app_key 缺失）→ 返回 Err。
    ///
    /// follow-up #9 起：错误信息是 VaultError::KeychainUnavailable 的稳定 JSON
    /// `{ code: "keychain_unavailable", message: ... }`——不再透传内部细节。
    #[test]
    fn encrypted_without_app_key_errors() {
        let key = make_key(7);
        let encrypted = key.encrypt(b"some-key").unwrap();
        let session = empty_session(); // app_key=None

        let result = try_decrypt_secret(&encrypted, &session);
        assert!(result.is_err(), "vault 已启用但 app_key 不可用应返回 Err");
        let err = result.unwrap_err();
        // 新契约：JSON 序列化的 VaultError，code=keychain_unavailable
        assert!(
            err.contains("keychain_unavailable") || err.contains("密钥串"),
            "错误信息应含 keychain_unavailable 稳定 code / 密钥串 message，got: {}",
            err
        );
    }

    /// v1: 前缀 + 错误的 app_key（key 不匹配）→ decrypt 失败 → 返回 Err。
    #[test]
    fn encrypted_with_wrong_app_key_errors() {
        let encryptor = make_key(1);
        let encrypted = encryptor.encrypt(b"plain").unwrap();

        let wrong_key = make_key(2);
        let session = session_with_app_key(wrong_key);

        let result = try_decrypt_secret(&encrypted, &session);
        assert!(result.is_err(), "key 不匹配应解密失败");
    }

    /// try_decrypt_secret_global：全局 session 未注入 → 原样返回（向后兼容）。
    #[test]
    fn global_without_session_passthrough() {
        // 注意：单测里 GLOBAL_SESSION 不一定注入，这里只验证「未注入时」的行为。
        // 由于 OnceLock::set 是 once 语义，如果其他测试已 set 了，本测试会拿到 session——
        // 因此这里只断言「raw 不以 v1: 开头时一定原样返回」（与是否注入无关）。
        assert_eq!(try_decrypt_secret_global("sk-plain"), "sk-plain");
        assert_eq!(try_decrypt_secret_global(""), "");
    }

    /// round-trip：用 migrate 的加密路径产出的密文，本模块能解出来。
    ///
    /// 与 crates/vault/src/migrate.rs 的 `migrate_encrypts_plaintext_keys` 对称——
    /// 那个测试验证「明文 → v1: 密文」，这里验证「v1: 密文 → 明文」。
    #[test]
    fn round_trip_with_migrate_path() {
        let key = make_key(42);
        let session = session_with_app_key(key.clone());

        // 模拟 Task 20 migrate 产出的密文
        let plaintext = "plaintext-api-key-for-round-trip";
        let migrated = key.encrypt(plaintext.as_bytes()).unwrap();
        assert!(migrated.starts_with("v1:"));

        // 模拟推理热路径消费
        let decrypted = try_decrypt_secret(&migrated, &session).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
