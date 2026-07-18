//! 工具函数：CSPRNG、Base64、常量时间比较。

use anyhow::{Context, Result};
use data_encoding::BASE64;
use rand::rngs::OsRng;
use rand::RngCore;

/// 用 OS 熵源生成随机字节（CSPRNG）。
pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf
}

/// 生成 32B 随机（用于 K_machine / salt / key 等）。
pub fn random_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
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
        assert_ne!(a, b);
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
