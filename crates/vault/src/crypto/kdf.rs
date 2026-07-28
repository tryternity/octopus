//! Argon2id 派生 master_root_key。
//!
//! 参数：t=3, m=65536 KiB (64 MiB), p=4（OWASP 2024 推荐）。
//! salt：32B 随机（首次 init 生成，存 vault_meta.kdf_salt）。

use anyhow::{ensure, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use super::DerivedKey;

/// Argon2id 参数。默认 t=3, m=64 MiB, p=4。
///
/// K3 修复（2026-07-24）：移除 `Deserialize` derive。之前 `#[derive(Deserialize)]`
/// 暴露了"可构造 iterations=0 / 弱参数 Argon2Params 绕过 from_i64 校验"的能力。
/// 当前无任何反序列化调用方（所有构造走 from_i64 / from_i64_strict / Default），
/// 但若未来加配置加载/API 接收 JSON 的路径，Deserialize 会静默绕过校验。
/// 保留 Serialize（未来可能用于配置导出，且不暴露构造能力）。
/// 若未来确需反序列化，必须构造后立即用 from_i64_strict 复校验。
#[derive(Debug, Clone, serde::Serialize)]
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
    ///
    /// **注意（K1, 2026-07-24）**：本函数的下限是**崩溃级**（防 argon2 crate panic），
    /// 非安全级。memory_kib=8 (8KB) 仍可通过——这废掉了 Argon2id 的内存硬度（GPU
    /// 可全放寄存器高度并行爆破）。处理**远程不可信** KDF 参数（同步仓库的 meta.json）
    /// 必须用 [`from_i64_strict`]，它有安全下限。本地 DB 可信，继续用本函数。
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

    /// 从**远程不可信** i64 字段构造 Argon2Params，带安全下限校验（K1 修复，2026-07-24）。
    ///
    /// 与 [`from_i64`] 的区别：下限是**安全级**而非崩溃级，防止攻击者污染同步仓库的
    /// vault_meta 为弱 KDF 参数（如 memory_kib=8）废掉 Argon2id 内存硬度。
    ///
    /// 安全下限依据：
    /// - `memory_kib ≥ 16384`（16 MiB）：保留 GPU 抗并行的内存硬度。8KB 可全放 GPU
    ///   寄存器/共享内存 → 高度并行爆破，内存硬度几乎归零。OWASP 推荐值 65536 (64MiB)，
    ///   16384 是兼顾安全与兼容旧配置的宽松下限。
    /// - `iterations ≥ 2`：OWASP 推荐 3，允许 2 兼容合理配置，挡住 iterations=1。
    /// - `parallelism ≥ 1`：与 from_i64 一致。
    ///
    /// 使用场景：`sync/engine.rs::resolve_with_remote` 处理远程 meta.json 的 KDF 参数。
    /// 本地 DB 走 [`from_i64`]（本地可信假设）。
    pub fn from_i64_strict(
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
            memory_kib >= 16384 && memory_kib <= u32::MAX as i64,
            "远程 kdf_memory_kib 过弱（{}，安全下限 ≥ 16384 KiB / 16MiB——\
             低于此值 Argon2id 内存硬度归零，GPU 可高度并行爆破；\
             若见此错检查同步仓库 vault_meta 是否被篡改）",
            memory_kib
        );
        ensure!(
            iterations >= 2,
            "远程 kdf_iterations 过弱（{}，安全下限 ≥ 2）",
            iterations
        );
        // K-KDF-STRICT-MISSING-CEILING 修复（2026-07-26）：K1 守了机密性下限（防弱 KDF
        // 爆破），但漏了可用性上限（防资源耗尽）。攻击者污染远程 meta.json 为
        // memory_kib=2GiB / iterations=u32::MAX → from_i64_strict 通过 → 写入本地 DB →
        // 每次 unlock OOM/卡死，用户被永久锁在密码库外。上限取 OWASP 推荐值 4 倍：
        //   - memory_kib ≤ 262144（256 MiB，OWASP 推荐 64MiB × 4）
        //   - iterations ≤ 10（OWASP 推荐 3 × ~3）
        //   - parallelism ≤ 16（OWASP 推荐 4 × 4）
        ensure!(
            memory_kib <= 262144,
            "远程 kdf_memory_kib 过高（{}，安全上限 ≤ 262144 KiB / 256MiB——\
             超此值会 OOM/卡死；若见此错检查同步仓库 vault_meta 是否被篡改）",
            memory_kib
        );
        ensure!(
            iterations <= 10,
            "远程 kdf_iterations 过高（{}，安全上限 ≤ 10——\
             超此值派生耗时数分钟/小时）",
            iterations
        );
        ensure!(
            parallelism <= 16,
            "远程 kdf_parallelism 过高（{}，安全上限 ≤ 16）",
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

    /// 测试专用弱 KDF 参数（argon2 派生从 ~0.4s/次 → <1ms/次）。
    ///
    /// memory_kib=8 是 `from_i64` 的崩溃级下限（argon2 crate 要求 ≥8），
    /// iterations=1 / parallelism=1 最小化计算。**仅用于 `#[cfg(test)]`**——
    /// 生产代码必须用 [`Default`]（64 MiB / 3 iterations / 4 parallelism）。
    ///
    /// 安全性：弱参数通过 `from_i64`（本地 DB 可信路径），但**不通过 `from_i64_strict`**
    /// （memory_kib < 16384）。sync engine 的 `resolve_with_remote` 走 strict 路径，
    /// 不会被测试的弱参数污染（unlock 测试用 local DB，不经 remote sync）。
    #[cfg(test)]
    pub(crate) fn test_params() -> Self {
        Self {
            iterations: 1,
            memory_kib: 8,
            parallelism: 1,
        }
    }
}

/// 从 master_password + 32B salt 派生 master_root_key。
///
/// **源秘密清零约定**（H1 修复，2026-07-24）：本函数收 `&[u8]` 只读借用，不接管
/// password 引用。调用方（`unlock.rs` 的 4 个入口 + `sync/engine.rs` 的 2 个 resolve）
/// 已统一收 `Zeroizing<String>` 所有权，函数结束时自动清零 heap——源秘密这条线的
/// Zeroizing 卫生已闭环。底层本函数保持 `&[u8]` 通用签名（KDF 库要求）。
pub fn derive_master_root_key(password: &[u8], salt: &[u8], params: &Argon2Params) -> Result<DerivedKey> {
    ensure!(salt.len() == 32, "salt 必须为 32 字节，当前 {}", salt.len());

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.to_params()?);
    // C1 修复（2026-07-24）：用 Zeroizing<[u8;32]> 借用写入，move 整个 Zeroizing。
    // 之前用裸 [u8;32]（Copy 类型）→ Zeroizing::new(out) 是复制，原栈数组 drop 时
    // 不清零（[u8;32] 的 Drop 是 no-op），成功路径残留 + 失败路径（? 提前返回）更残留。
    // 现在 Zeroizing 持有唯一副本，任一返回路径 scope 结束都清零。
    //
    // 第七十二轮（2026-07-27）记录 64 MiB argon2 memory blocks deliberate trade-off：
    // hash_password_into 内部分配 ~64 MiB memory blocks（Vec<Block>，Block 是 Copy + 无
    // ZeroizeOnDrop），hash 后 Vec drop 只 dealloc 不清零 → 64 MiB argon2 中间态 heap 残留。
    // 这是当前选择不清的权衡——argon2 单向性（G 函数 + data-dependent addressing）保证
    // memory blocks 不可逆推 password，残留是 cold boot / heap dump 攻击面，非密钥泄漏。
    // vault 启用 argon2 zeroize feature 只让 ~2KB 中间状态（initial_hash 64B + blockhash
    // 1024B + blockhash_bytes 1024B）清零，不覆盖 64 MiB memory。
    //
    // 未来如需清除 64 MiB，可改用 hash_password_into_with_memory 自己 own Vec<Block>
    // 后遍历 .zeroize()（与 argon2 官方 lib.rs:230 hash_password_into 实现一致）：
    //   let mut blocks = vec![Block::default(); params.block_count()];
    //   argon2.hash_password_into_with_memory(password, salt, &mut *out, &mut blocks)?;
    //   for block in &mut blocks { block.zeroize(); }
    let mut out = Zeroizing::new([0u8; 32]);
    argon2
        .hash_password_into(password, salt, &mut *out)
        .context("Argon2id 派生失败")?;
    Ok(DerivedKey::from_zeroizing(out))
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
        // 同 password + salt + params → 同 master_root_key（用 test_params 加速）
        let salt = [42u8; 32];
        let p = Argon2Params::test_params();
        let k1 = derive_master_root_key(b"my-password", &salt, &p).unwrap();
        let k2 = derive_master_root_key(b"my-password", &salt, &p).unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_different_password_different_key() {
        let salt = [42u8; 32];
        let p = Argon2Params::test_params();
        let k1 = derive_master_root_key(b"password1", &salt, &p).unwrap();
        let k2 = derive_master_root_key(b"password2", &salt, &p).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_different_salt_different_key() {
        let s1 = [1u8; 32];
        let s2 = [2u8; 32];
        let p = Argon2Params::test_params();
        let k1 = derive_master_root_key(b"same-pwd", &s1, &p).unwrap();
        let k2 = derive_master_root_key(b"same-pwd", &s2, &p).unwrap();
        assert_ne!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn test_invalid_salt_length() {
        let p = Argon2Params::test_params();
        let result = derive_master_root_key(b"pwd", &[0u8; 16], &p);
        assert!(result.is_err());
    }

    /// K1 守护（2026-07-24）：from_i64_strict 拒绝弱 KDF 参数（远程不可信输入）。
    ///
    /// 攻击链：攻击者污染同步仓库 vault_meta 为 kdf_memory_kib=8 → 受害者 sync →
    /// resolve_with_remote 用弱参数派生 → 攻击者离线爆破时 GPU 高度并行（8KB 可全放
    /// 寄存器，内存硬度归零）。from_i64_strict 用安全下限拦截。
    #[test]
    fn from_i64_strict_rejects_weak_remote_params() {
        // memory_kib=8（崩溃下限，from_i64 接受但 from_i64_strict 必须拒）
        assert!(
            Argon2Params::from_i64(3, 8, 4).is_ok(),
            "from_i64 接受 memory=8（本地 DB 可信，崩溃下限）"
        );
        assert!(
            Argon2Params::from_i64_strict(3, 8, 4).is_err(),
            "from_i64_strict 必须拒 memory=8（远程不可信，废掉内存硬度）"
        );

        // iterations=1（from_i64 接受，from_i64_strict 拒，安全下限 ≥2）
        assert!(Argon2Params::from_i64_strict(1, 65536, 4).is_err());

        // memory_kib=16384（16MiB，安全下限边界）应通过
        assert!(Argon2Params::from_i64_strict(3, 16384, 4).is_ok());
        // memory_kib=16383（刚好低于下限）应拒
        assert!(Argon2Params::from_i64_strict(3, 16383, 4).is_err());

        // 默认参数（OWASP 推荐 t=3/m=64MiB/p=4）两条路径都应通过
        let default = Argon2Params::default();
        assert!(Argon2Params::from_i64_strict(
            default.iterations as i64,
            default.memory_kib as i64,
            default.parallelism as i64,
        )
        .is_ok());

        // 负值 / 超大值两条路径都拒
        assert!(Argon2Params::from_i64_strict(-1, 65536, 4).is_err());
        assert!(Argon2Params::from_i64_strict(3, -1, 4).is_err());
    }

    /// K-KDF-STRICT-MISSING-CEILING 守护（2026-07-26）：from_i64_strict 拒绝过高远程参数。
    ///
    /// K1 守了机密性下限（防弱 KDF 爆破），本测试守可用性上限（防资源耗尽）。
    /// 攻击者污染 meta.json 为 memory_kib=2GiB / iterations=u32::MAX → OOM/卡死。
    #[test]
    fn from_i64_strict_rejects_huge_remote_params() {
        // memory_kib 上限 262144（256 MiB）
        assert!(Argon2Params::from_i64_strict(3, 262144, 4).is_ok(), "边界值 256MiB 应通过");
        assert!(Argon2Params::from_i64_strict(3, 262145, 4).is_err(), "超 256MiB 应拒");
        assert!(Argon2Params::from_i64_strict(3, 2097152, 4).is_err(), "2GiB 应拒（OOM 攻击）");

        // iterations 上限 10
        assert!(Argon2Params::from_i64_strict(10, 65536, 4).is_ok(), "边界值 10 应通过");
        assert!(Argon2Params::from_i64_strict(11, 65536, 4).is_err(), "超 10 应拒");
        assert!(Argon2Params::from_i64_strict(4294967295, 65536, 4).is_err(), "u32::MAX 应拒");

        // parallelism 上限 16
        assert!(Argon2Params::from_i64_strict(3, 65536, 16).is_ok(), "边界值 16 应通过");
        assert!(Argon2Params::from_i64_strict(3, 65536, 17).is_err(), "超 16 应拒");
    }

    /// M1-mod 守护：DerivedKey 字段 private，外部经 from_raw 构造，as_bytes 读取。
    #[test]
    fn derived_key_from_raw_round_trip() {
        let arr = [0xAB; 32];
        let key = DerivedKey::from_raw(arr);
        assert_eq!(key.as_bytes(), &arr);
        // 字段 private：以下若取消注释应编译失败（验证字段不可直接访问）
        // let _ = key.0;
    }
}
