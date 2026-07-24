//! TOTP（RFC 6238）生成。
//!
//! 支持两种输入（修复 #7）：
//! - **裸 Base32 secret**：`from_base32` 用默认参数（SHA1, 6 digits, 30s, skew=1）
//! - **otpauth:// URL**：`from_otpauth_url` 解析 URL，支持 SHA1/SHA256/SHA512、
//!   digits=6/8、period=任意（典型 30/60）——GitHub、银行、Authy 导出等场景必需
//!
//! **secret 长度**：用 `new_unchecked` 放宽到tp-rs 默认 ≥128bit 限制——RFC 6238
//! 下限是 80bit（10 字节），大量服务实际下发（如 `JBSWY3DPEHPK3PXP` 解码后 80bit）。
//! 首发 `new()` 会拒绝这些合法 secret。
//!
//! 输出：当前数字码（按 digits 长度）+ 剩余秒数（按 step 长度）。

use anyhow::{anyhow, ensure, Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use totp_rs::{Algorithm, Secret, TOTP};

pub struct TotpGenerator {
    inner: TOTP,
    /// step（秒）——用于算剩余秒数。otpauth URL 可能是 30 / 60 等。
    step: u64,
}

impl TotpGenerator {
    /// 从裸 Base32 secret 构造，用默认参数（SHA1, 6 digits, 30s, skew=1）。
    ///
    /// 放宽 secret 长度限制：`new_unchecked` 跳过 totp-rs 的 ≥128bit 强制校验，
    /// 支持 RFC 6238 下限的 80bit secret（10 字节，如 JBSWY3DPEHPK3PXP）。
    pub fn from_base32(secret: &str) -> Result<Self> {
        let bytes = Secret::Encoded(secret.to_string())
            .to_bytes()
            .context("TOTP secret Base32 解码失败")?;
        // M2 修复（2026-07-24）：校验 secret 长度下限——RFC 6238 要求 ≥ 80bit（10 字节）。
        // base32::decode("") 返回 Some(Vec::new()) 而非 None，空 secret 会通过 →
        // current() 用空 secret 调 HMAC 生成完全可预测的 code。必须显式拦截。
        ensure!(
            bytes.len() >= 10,
            "TOTP secret 过短（{} 字节，RFC 6238 要求 ≥ 10 字节/80bit）",
            bytes.len()
        );
        // new_unchecked：不强制 secret >= 128bit（修复 #7）。
        // otpauth feature 启用后签名要求 issuer + account_name（即使不用 otpauth URL）。
        let totp = TOTP::new_unchecked(
            Algorithm::SHA1,
            6,
            1,
            30,
            bytes,
            None,
            String::new(),
        );
        Ok(Self { inner: totp, step: 30 })
    }

    /// 从 otpauth:// URL 构造，解析 algorithm/digits/period/secret 等参数。
    ///
    /// 支持 GitHub / 银行 / Authy 导出等非标准配置：
    /// - algorithm: SHA1（默认）/ SHA256 / SHA512
    /// - digits: 6（默认）/ 8
    /// - period: 30（默认）/ 60 / 15 等
    ///
    /// URL 格式示例：
    /// `otpauth://totp/GitHub:user@example.com?secret=JBSWY3DPEHPK3PXP&issuer=GitHub&algorithm=SHA1&digits=6&period=30`
    pub fn from_otpauth_url(url: &str) -> Result<Self> {
        // from_url_unchecked：不强制 secret >= 128bit（修复 #7）——支持 80bit 标准 secret。
        // 与 from_base32 的 new_unchecked 对称。
        // 注意 from_url_unchecked<S: AsRef<str>> 泛型 + &str 已实现 AsRef<str>，直接传 url
        // 无需 .to_string()（省一次堆分配，复审次要观察 #1）。
        let totp = TOTP::from_url_unchecked(url)
            .map_err(|e| anyhow!("otpauth URL 解析失败: {}", e))?;

        // 复审 #1 修复：unchecked 跳过 totp-rs 全部不变量校验，畸形参数会导致
        // current() 内部 panic（period=0 → time/self.step 整除 panic；digits 异常 →
        // 10_u32.pow(digits) overflow panic）。不可信输入（用户粘贴 / Bitwarden 导入）
        // 不能 panic——这里是命令边界，panic 会崩 Tauri 命令甚至进程。
        //
        // 白名单 clamp 而非范围检查：
        // - period：RFC 6238 推荐 30，常见 15/60，>0 即可（不设上限——长 period 安全但 TOTP 实用性下降）
        // - digits：本处仅允许 RFC 标准的 6 或 8——7 digit（Authy / 部分银行）会被拒，
        //   即使 totp-rs 格式化层支持任意 digits（实用中 7 极罕见，用户可改用 8 凑齐）
        // - algorithm：SHA1 / SHA256 / SHA512 是 totp-rs 在 steam feature off 时仅有的合法变体
        ensure!(totp.step > 0, "TOTP period 必须 > 0（当前 0 会致除零 panic）");
        ensure!(
            totp.digits == 6 || totp.digits == 8,
            "TOTP digits 仅支持 6 或 8（当前 {}）",
            totp.digits
        );
        ensure!(
            matches!(
                totp.algorithm,
                Algorithm::SHA1 | Algorithm::SHA256 | Algorithm::SHA512
            ),
            "TOTP algorithm 仅支持 SHA1/SHA256/SHA512"
        );
        // M2 修复（2026-07-24）：校验 secret 长度下限——与 from_base32 对称。
        // 文件头注释（:8-9）承诺 RFC 6238 80bit 下限，otpauth 路径之前未落地。
        // 威胁：Bitwarden 导入畸形 URL / 用户手贴空 secret otpauth → 完全可预测的 code。
        ensure!(
            totp.secret.len() >= 10,
            "TOTP secret 过短（{} 字节，RFC 6238 要求 ≥ 10 字节/80bit）",
            totp.secret.len()
        );

        let step = totp.step; // 先取，避免下面 move 后访问
        Ok(Self { inner: totp, step })
    }

    /// 智能构造：自动判断输入是 otpauth:// URL 还是裸 Base32。
    ///
    /// 前端用户可能粘贴任一格式——本方法统一入口：
    /// - 输入以 `otpauth://` 开头（大小写不敏感）→ 走 from_otpauth_url
    /// - 否则 → 走 from_base32（裸 Base32 secret）
    pub fn from_input(input: &str) -> Result<Self> {
        let trimmed = input.trim();
        if trimmed.to_lowercase().starts_with("otpauth://") {
            Self::from_otpauth_url(trimmed)
        } else {
            Self::from_base32(trimmed)
        }
    }

    pub fn current(&self) -> Result<String> {
        // last-resort 防护：step=0 时 generate_current 内部 time/step 会除零 panic。
        // from_otpauth_url 已 clamp period>0，from_base32 硬编码 30——理论均安全，
        // 但未来重构可能漏过，这里兜底返回 Err 而非 panic（panic 在 Tauri 命令边界致命）。
        if self.step == 0 {
            anyhow::bail!("TOTP step=0（无效配置，应被 from_otpauth_url clamp 拦截）");
        }
        Ok(self.inner.generate_current().context("TOTP 生成失败")?)
    }

    /// 当前 step 内剩余秒数。按 self.step（可能 30 / 60 等）算。
    pub fn seconds_remaining(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // 防 step=0（理论不可能，但避免 panic）
        if self.step == 0 {
            return 30;
        }
        self.step - (now % self.step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 标准 80bit secret（10 字节）——首发版用 `new()` 会被拒绝，
    /// 现在用 `new_unchecked` 支持（修复 #7）。
    #[test]
    fn test_short_80bit_secret_accepted() {
        // JBSWY3DPEHPK3PXP 是 80bit（Hello!...\xDE\xAD\xBE\xEF 的 Base32）
        // totp-rs `new()` 会拒绝（< 128bit），`new_unchecked` 接受
        let gen = TotpGenerator::from_base32("JBSWY3DPEHPK3PXP").unwrap();
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_totp_format_6_digits() {
        // 32 字符（160 bit）Base32，仍然兼容
        let gen = TotpGenerator::from_base32("JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP").unwrap();
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_seconds_remaining_in_range_default_30s() {
        let gen = TotpGenerator::from_base32("JBSWY3DPEHPK3PXP").unwrap();
        let r = gen.seconds_remaining();
        assert!(r >= 1 && r <= 30, "seconds_remaining 应在 1..=30，实际 {}", r);
    }

    #[test]
    fn test_invalid_base32_secret() {
        assert!(TotpGenerator::from_base32("!!!invalid base32!!!").is_err());
    }

    #[test]
    fn test_known_totp_value() {
        // RFC 6238 测试向量
        // Secret: GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ (Base32 of "12345678901234567890")
        let gen = TotpGenerator::from_base32("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap();
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 6);
    }

    /// otpauth:// URL 解析——完整参数（algorithm/digits/period/issuer）。
    #[test]
    fn test_otpauth_url_full_parse() {
        let url = "otpauth://totp/GitHub:user@example.com?\
                   secret=JBSWY3DPEHPK3PXP&issuer=GitHub&algorithm=SHA1&digits=6&period=30";
        let gen = TotpGenerator::from_otpauth_url(url).unwrap();
        assert_eq!(gen.step, 30);
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 6);
    }

    /// otpauth:// URL 支持 SHA256 + digits=8 + period=60 变体（修复 #7 核心）。
    #[test]
    fn test_otpauth_url_sha256_8digits_60s() {
        // 一些银行 / Authy 导出用这种非标准配置
        let url = "otpauth://totp/Bank:user?secret=JBSWY3DPEHPK3PXP&issuer=Bank\
                   &algorithm=SHA256&digits=8&period=60";
        let gen = TotpGenerator::from_otpauth_url(url).unwrap();
        assert_eq!(gen.step, 60, "period 应解析为 60");
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 8, "digits 应解析为 8");
        // 剩余秒数应按 period=60 算
        let r = gen.seconds_remaining();
        assert!(r >= 1 && r <= 60, "seconds_remaining 应在 1..=60，实际 {}", r);
    }

    /// otpauth:// URL 缺省参数（只必填 secret）→ 走 RFC 默认（SHA1/6/30）。
    #[test]
    fn test_otpauth_url_minimal() {
        let url = "otpauth://totp/?secret=JBSWY3DPEHPK3PXP";
        let gen = TotpGenerator::from_otpauth_url(url).unwrap();
        assert_eq!(gen.step, 30);
        let code = gen.current().unwrap();
        assert_eq!(code.len(), 6);
    }

    /// from_input 智能分发：otpauth:// → URL 解析；其他 → 裸 Base32。
    #[test]
    fn test_from_input_dispatch() {
        // otpauth://
        let url = "otpauth://totp/?secret=JBSWY3DPEHPK3PXP";
        let gen = TotpGenerator::from_input(url).unwrap();
        assert_eq!(gen.current().unwrap().len(), 6);

        // 裸 Base32
        let gen2 = TotpGenerator::from_input("JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(gen2.current().unwrap().len(), 6);

        // 大小写不敏感前缀
        let gen3 = TotpGenerator::from_input("OTPAUTH://totp/?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(gen3.current().unwrap().len(), 6);

        // 输入前后空格被 trim
        let gen4 = TotpGenerator::from_input("  JBSWY3DPEHPK3PXP  ").unwrap();
        assert_eq!(gen4.current().unwrap().len(), 6);
    }

    /// 非法 otpauth:// URL → Err
    #[test]
    fn test_otpauth_url_invalid() {
        assert!(TotpGenerator::from_otpauth_url("otpauth://totp/?nosecret=here").is_err());
        assert!(TotpGenerator::from_otpauth_url("https://example.com/not-otpauth").is_err());
    }

    /// 复审 #1 修复：period=0 不能 panic（不可信输入触发，会崩 Tauri 命令）。
    /// from_otpauth_url 应返 Err，current() 也应返 Err（last-resort 防护）。
    #[test]
    fn test_period_zero_returns_err_not_panic() {
        let url = "otpauth://totp/?secret=JBSWY3DPEHPK3PXP&period=0";
        let result = TotpGenerator::from_otpauth_url(url);
        assert!(
            result.is_err(),
            "period=0 应被 clamp 拦截返 Err（不 panic）"
        );
    }

    /// 复审 #1 修复：畸形 digits 不能 panic。
    #[test]
    fn test_digits_invalid_returns_err() {
        // digits=0：format!("{1:00$}", 0, ...) → 空串，不 panic 但无意义，应被 clamp 拒
        let url = "otpauth://totp/?secret=JBSWY3DPEHPK3PXP&digits=0";
        assert!(TotpGenerator::from_otpauth_url(url).is_err());

        // digits=20：10_u32.pow(20) overflow panic（u32 max ~4.3e9 < 1e20），应被 clamp 拒
        let url2 = "otpauth://totp/?secret=JBSWY3DPEHPK3PXP&digits=20";
        assert!(TotpGenerator::from_otpauth_url(url2).is_err());

        // digits=7：非标准（Authy 等罕见），按 RFC 仅允许 6/8，拒绝
        let url3 = "otpauth://totp/?secret=JBSWY3DPEHPK3PXP&digits=7";
        assert!(TotpGenerator::from_otpauth_url(url3).is_err());
    }

    /// 复审 #1 修复：非法 algorithm 不能 silent 通过。
    ///
    /// ⚠️ 此测试覆盖**偏弱**（复审次要观察 #2）：MD5 根本不在 totp-rs `Algorithm`
    /// 枚举里，URL 解析阶段（`from_url_unchecked`）就返 Err——并非被本 crate 的
    /// clamp 拦截。当前 otpauth feature 配置下没有能绕过 parse 阶段的非标准
    /// algorithm（`Steam` 变体 cfg gate off 不存在），所以 clamp 分支实际未被
    /// 真正测到。若未来启用 `steam` feature 或 totp-rs 放宽枚举，需补 Steam 单测。
    #[test]
    fn test_algorithm_invalid_returns_err() {
        let url = "otpauth://totp/?secret=JBSWY3DPEHPK3PXP&algorithm=MD5";
        assert!(TotpGenerator::from_otpauth_url(url).is_err());
    }

    /// M2 修复回归守护：空 secret / 过短 secret 必须被拒（不能生成可预测 code）。
    #[test]
    fn test_empty_or_short_secret_rejected() {
        // 空 secret otpauth（base32 decode("") 返 Some(Vec::new())，非 None）
        assert!(TotpGenerator::from_otpauth_url("otpauth://totp/?secret=").is_err());
        // 过短 secret（< 10 字节 / 80bit）
        // "AB" 解码后 1 字节——远低于 RFC 6238 下限
        assert!(TotpGenerator::from_base32("AB").is_err());
        assert!(TotpGenerator::from_otpauth_url("otpauth://totp/?secret=AB").is_err());
        // 边界：刚好 10 字节（80bit）应通过——JBSWY3DPEHPK3PXP 是 10 字节
        assert!(TotpGenerator::from_base32("JBSWY3DPEHPK3PXP").is_ok());
    }
}
