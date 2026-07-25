//! 加密原语：KDF、密钥派生、对称加密。

pub mod kdf;
pub mod hierarchy;
pub mod symmetric;
pub mod util;

use zeroize::Zeroizing;

/// 32 字节密钥，所有派生/加密 key 都用此类型。Drop 时自动清零。
///
/// M1-mod 修复（2026-07-24）：字段改 private。之前 `pub struct DerivedKey(pub Zeroizing<[u8;32]>)`
/// 字段 pub，任何持有 DerivedKey 的代码可经 `.0` 直接读原始 32B 字节，绕过 `as_bytes`
/// 受控接口——若某处把 `.0` 字节拷到非 Zeroizing 缓冲（log/String），绕过清零。
/// 现字段 private，crate 内构造走 `from_zeroizing` / `from_raw`，外部 crate 走 `from_raw`。
#[derive(Clone)]
pub struct DerivedKey(Zeroizing<[u8; 32]>);

impl DerivedKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 从已知 32B 数组构造（用于把 K_machine 包装成 DerivedKey）。
    /// 外部 crate（如 desktop 测试）构造 DerivedKey 的公开入口。
    pub fn from_raw(arr: [u8; 32]) -> Self {
        DerivedKey(Zeroizing::new(arr))
    }

    /// 从 Zeroizing 包装的 32B 构造（crate 内部用，如 KDF 派生结果 / hierarchy 子 key）。
    ///
    /// pub(crate)：外部 crate 无法绕过 from_raw 直接构造，但 crate 内的生产代码
    /// （kdf.rs derive_master_root_key / hierarchy.rs derive_child_key）可复用已有 Zeroizing。
    pub(crate) fn from_zeroizing(key: Zeroizing<[u8; 32]>) -> Self {
        DerivedKey(key)
    }
}
