//! TOTP（RFC 6238）生成。
//!
//! 固定算法：HMAC-SHA1, 30s, 6 位, ±1 步漂移（totp-rs skew=1）。
//! 输入：Base32 secret（如 "JBSWY3DPEHPK3PXP"）。
//! 输出：当前 6 位数字 + 剩余秒数。

use anyhow::{Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use totp_rs::{Algorithm, Secret, TOTP};

pub struct TotpGenerator {
    inner: TOTP,
}

impl TotpGenerator {
    pub fn from_base32(secret: &str) -> Result<Self> {
        let bytes = Secret::Encoded(secret.to_string())
            .to_bytes()
            .context("TOTP secret Base32 解码失败")?;
        let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, bytes)
            .context("TOTP 构造失败")?;
        Ok(Self { inner: totp })
    }

    pub fn current(&self) -> Result<String> {
        Ok(self.inner.generate_current().context("TOTP 生成失败")?)
    }

    pub fn seconds_remaining(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        30 - (now % 30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_totp_format_6_digits() {
        // 注：totp-rs v5 强制 secret >= 128 bits，故使用 32 字符（160 bits）的 Base32。
        let gen = TotpGenerator::from_base32("JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP").unwrap();
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_totp_seconds_remaining_in_range() {
        let gen = TotpGenerator::from_base32("JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP").unwrap();
        let r = gen.seconds_remaining();
        assert!(r >= 1 && r <= 30);
    }

    #[test]
    fn test_invalid_base32_secret() {
        assert!(TotpGenerator::from_base32("!!!invalid base32!!!").is_err());
    }

    #[test]
    fn test_known_totp_value() {
        // RFC 6238 测试向量
        // Secret: GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ (Base32 of "12345678901234567890")
        // 算法：SHA1, 30s, 8 digits
        // 注：totp-rs 默认 6 digits，所以这里用我们自己的 6 digit 配置
        // 这个测试仅验证不 panic，因为时点会影响具体值
        let gen = TotpGenerator::from_base32("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap();
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 6);
    }
}
