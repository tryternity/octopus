//! Argon2id 派生 master_root_key。
//!
//! 参数：t=3, m=65536 KiB (64 MiB), p=4（OWASP 2024 推荐）。
//! salt：32B 随机（首次 init 生成，存 vault_meta.kdf_salt）。

use anyhow::{ensure, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use super::DerivedKey;

/// Argon2id 参数。默认 t=3, m=64 MiB, p=4。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Argon2Params {
    pub iterations: u32,    // t，默认 3
    pub memory_kib: u32,    // m，默认 65536 = 64 MiB
    pub parallelism: u32,   // p，默认 4
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            iterations: 3,
            memory_kib: 65_536,
            parallelism: 4,
        }
    }
}

impl Argon2Params {
    /// 用 Params::new 构造 argon2 crate 用的参数对象
    fn to_params(&self) -> Result<Params> {
        Params::new(self.memory_kib, self.iterations, self.parallelism, Some(32))
            .context("Argon2id 参数无效")
    }

    /// 从 DB 的 i64 字段构造 Argon2Params，带范围 + 最小值校验（#14 修复）。
    ///
    /// 之前 `as u32` 截断发生在 Params::new 之前——负值/超大值静默回绕成弱参数，
    /// 无任何完整性校验。现在显式检查：
    /// - iterations/memory_kib/parallelism 必须在 u32 正范围内（i64 负值或 > u32::MAX 拒绝）
    /// - 最小值：iterations ≥ 1, memory_kib ≥ 8（argon2 crate 要求），parallelism ≥ 1
    ///
    /// 威胁模型：DB 被直接改（kdf_iterations=1）可悄悄削弱 KDF。虽然单机威胁模型
    /// 假设 DB 不被直接改，但加这层校验成本低、能至少在 Params::new 失败时报错
    /// 而非用弱参数继续。
    pub fn from_i64(
        iterations: i64,
        memory_kib: i64,
        parallelism: i64,
    ) -> Result<Self> {
        ensure!(
            (1..=u32::MAX as i64).contains(&iterations),
            "kdf_iterations 非法（{}，应在 1..=u32::MAX）",
            iterations
        );
        ensure!(
            (8..=u32::MAX as i64).contains(&memory_kib),
            "kdf_memory_kib 非法（{}，应 ≥ 8——argon2 crate 要求）",
            memory_kib
        );
        ensure!(
            (1..=u32::MAX as i64).contains(&parallelism),
            "kdf_parallelism 非法（{}，应 ≥ 1）",
            parallelism
        );
        Ok(Self {
            iterations: iterations as u32,
            memory_kib: memory_kib as u32,
            parallelism: parallelism as u32,
        })
    }

    /// 从 VaultMeta 行构造 Argon2Params（from_i64 的便利封装）。
    pub fn from_meta(meta: &octopus_infra::db::VaultMeta) -> Result<Self> {
        Self::from_i64(
            meta.kdf_iterations,
            meta.kdf_memory_kib,
            meta.kdf_parallelism,
        )
    }
}

/// 从 master_password + 32B salt 派生 master_root_key。
///
/// **调用者必须在调用后立即 zeroize password**（本函数不接管 password 引用）。
pub fn derive_master_root_key(password: &[u8], salt: &[u8], params: &Argon2Params) -> Result<DerivedKey> {
    ensure!(salt.len() == 32, "salt 必须为 32 字节，当前 {}", salt.len());

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.to_params()?);
    let mut out = [0u8; 32];
    argon2
        .hash_password_into(password, salt, &mut out)
        .context("Argon2id 派生失败")?;
    Ok(DerivedKey(Zeroizing::new(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_params_match_spec() {
        let p = Argon2Params::default();
        assert_eq!(p.iterations, 3);
        assert_eq!(p.memory_kib, 65_536);
        assert_eq!(p.parallelism, 4);
    }

    #[test]
    fn test_kdf_deterministic() {
        // 同 password + salt + params → 同 master_root_key
        let salt = [42u8; 32];
        let p = Argon2Params::default();
        let k1 = derive_master_root_key(b"my-password", &salt, &p).unwrap();
        let k2 = derive_master_root_key(b"my-password", &salt, &p).unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_different_password_different_key() {
        let salt = [42u8; 32];
        let p = Argon2Params::default();
        let k1 = derive_master_root_key(b"password1", &salt, &p).unwrap();
        let k2 = derive_master_root_key(b"password2", &salt, &p).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_different_salt_different_key() {
        let s1 = [1u8; 32];
        let s2 = [2u8; 32];
        let p = Argon2Params::default();
        let k1 = derive_master_root_key(b"same-pwd", &s1, &p).unwrap();
        let k2 = derive_master_root_key(b"same-pwd", &s2, &p).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_invalid_salt_length() {
        let p = Argon2Params::default();
        let result = derive_master_root_key(b"pwd", &[0u8; 16], &p);
        assert!(result.is_err());
    }
}
