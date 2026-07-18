//! AES-256-GCM 对称加密。
//!
//! 密文格式（统一）：v1:<base64(nonce[12B] || ciphertext || tag[16B])>
//! AES-GCM 自带 16B 认证 tag，不需要独立 HMAC。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{ensure, Context, Result};

use super::util::{base64_decode, base64_encode, random_bytes};
use crate::Zeroizing;

pub const CIPHERTEXT_PREFIX: &str = "v1:";
const NONCE_LEN: usize = 12;

impl super::DerivedKey {
    /// 加密，返回 "v1:<base64(nonce||ct||tag)>"。
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<String> {
        let cipher = Aes256Gcm::new_from_slice(self.as_bytes())
            .context("AES-256-GCM key 长度必须为 32 字节")?;
        let nonce_bytes = random_bytes(NONCE_LEN);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .context("AES-256-GCM 加密失败")?;

        let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);
        Ok(format!("{}{}", CIPHERTEXT_PREFIX, base64_encode(&combined)))
    }

    /// 解密 v1: 前缀的密文。
    pub fn decrypt(&self, ciphertext: &str) -> Result<Zeroizing<Vec<u8>>> {
        let ct_str = ciphertext
            .strip_prefix(CIPHERTEXT_PREFIX)
            .context("密文必须以 v1: 开头")?;
        let combined = base64_decode(ct_str)?;
        ensure!(
            combined.len() > NONCE_LEN,
            "密文长度不足（缺 nonce）：{} bytes",
            combined.len()
        );

        let (nonce_bytes, ct) = combined.split_at(NONCE_LEN);
        let cipher = Aes256Gcm::new_from_slice(self.as_bytes())
            .context("AES-256-GCM key 长度必须为 32 字节")?;
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ct)
            .context("AES-256-GCM 解密失败：密文可能已损坏或 key 不匹配")?;

        Ok(Zeroizing::new(plaintext))
    }
}

#[cfg(test)]
mod tests {
    use crate::crypto::DerivedKey;
    use crate::Zeroizing as Z;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(Z::new([byte; 32]))
    }

    #[test]
    fn test_encrypt_decrypt_round_trip() {
        let key = make_key(1);
        let plaintext = b"sensitive data 1234";
        let ct = key.encrypt(plaintext).unwrap();
        assert!(ct.starts_with("v1:"));
        let pt = key.decrypt(&ct).unwrap();
        assert_eq!(&pt[..], plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let k1 = make_key(1);
        let k2 = make_key(2);
        let ct = k1.encrypt(b"secret").unwrap();
        assert!(k2.decrypt(&ct).is_err());
    }

    #[test]
    fn test_nonce_uniqueness() {
        // 同 key 同明文 → 不同密文（nonce 随机）
        let key = make_key(1);
        let c1 = key.encrypt(b"same").unwrap();
        let c2 = key.encrypt(b"same").unwrap();
        assert_ne!(c1, c2);
        // 但都能解出来
        assert_eq!(&key.decrypt(&c1).unwrap()[..], b"same");
        assert_eq!(&key.decrypt(&c2).unwrap()[..], b"same");
    }

    #[test]
    fn test_decrypt_invalid_prefix() {
        let key = make_key(1);
        assert!(key.decrypt("no-prefix").is_err());
        assert!(key.decrypt("v2:abc").is_err());
    }

    #[test]
    fn test_decrypt_truncated() {
        let key = make_key(1);
        // base64 of 5 bytes（少于 12B nonce）
        assert!(key.decrypt("v1:AAAAAAA").is_err());
    }

    #[test]
    fn test_encrypt_empty_plaintext() {
        let key = make_key(1);
        let ct = key.encrypt(b"").unwrap();
        let pt = key.decrypt(&ct).unwrap();
        assert!(pt.is_empty());
    }

    #[test]
    fn test_encrypt_large_plaintext() {
        let key = make_key(1);
        let big = vec![42u8; 100_000];
        let ct = key.encrypt(&big).unwrap();
        let pt = key.decrypt(&ct).unwrap();
        assert_eq!(&pt[..], &big[..]);
    }
}
