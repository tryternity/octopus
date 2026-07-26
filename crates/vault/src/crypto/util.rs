//! 工具函数：CSPRNG、Base64。
//!
//! C2 备注（2026-07-24）：原注释提及「常量时间比较」但全文无此函数——AES-GCM
//! tag 验证天然替代了密码比较（常量时间），不需独立实现。死注释已删。

use anyhow::{Context, Result};
use data_encoding::BASE64;
use rand::rngs::OsRng;
use rand::RngCore;
use zeroize::Zeroizing;

/// 用 OS 熵源生成随机字节（CSPRNG）。
pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf
}

/// 生成 32B 随机（用于 K_machine / salt / key 等）。
///
/// E-ZEROIZE-RESIDUE 修复（2026-07-26）：返 `Zeroizing<[u8;32]>` 而非裸 `[u8;32]`。
/// 之前返裸数组（Copy 类型）→ 调用方 `Zeroizing::new(arr)` 是复制，原栈数组 drop no-op
/// → K_machine 等密钥字节栈残留（与 kdf.rs/hierarchy.rs 已修的 C1 同型）。
/// 现在 Zeroizing 持有唯一副本，调用方 move 即可，无栈残留。
///
/// 公开 salt 调用方（如 unlock.rs kdf_salt）仍可用 `&*salt` 借用——salt 残留
/// 本身无害（公开值），但统一返 Zeroizing 让类型签名表达「这是敏感随机数据」。
pub fn random_32() -> Zeroizing<[u8; 32]> {
    let mut buf = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(&mut *buf);
    buf
}

pub fn base64_encode(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

pub fn base64_decode(s: &str) -> Result<Vec<u8>> {
    BASE64.decode(s.as_bytes()).context("Base64 解码失败")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_32_unique() {
        let a = random_32();
        let b = random_32();
        // Zeroizing<[u8;32]> 比较内部数组
        assert_ne!(*a, *b);
    }

    #[test]
    fn test_base64_round_trip() {
        let original = b"hello world 1234";
        let encoded = base64_encode(original);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_base64_decode_invalid() {
        assert!(base64_decode("!!!invalid base64!!!").is_err());
    }
}
