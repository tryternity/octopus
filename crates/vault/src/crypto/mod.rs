//! 加密原语：KDF、密钥派生、对称加密。

pub mod kdf;
pub mod hierarchy;
pub mod symmetric;
pub mod util;

use zeroize::Zeroizing;

/// 32 字节密钥，所有派生/加密 key 都用此类型。Drop 时自动清零。
#[derive(Clone)]
pub struct DerivedKey(pub Zeroizing<[u8; 32]>);

impl DerivedKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 从已知 32B 数组构造（用于把 K_machine 包装成 DerivedKey）。
    pub fn from_raw(arr: [u8; 32]) -> Self {
        DerivedKey(Zeroizing::new(arr))
    }
}
