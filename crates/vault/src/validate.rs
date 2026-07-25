//! 主密码强度校验（后端版，复审报告 #1 修复）。
//!
//! 翻译自前端 `validateMasterPassword.ts`——策略：长度 ≥ 12 + 必含 4 类字符
//! （大写 / 小写 / 数字 / 符号）。spec INV-10 / §7.4 / F19 明确要求「前端 +
//! 后端双校验，防前端绕过」——DevTools 可直接 invoke('vault_setup', {password: 'a'})
//! 设任意弱密码，前端校验不可信。
//!
//! 熵估算（V1 修正，2026-07-24）：95 可打印 ASCII × 12 位 ≈ 79 bit 是**理论上界**
//! （假设每位从 95 字符均匀随机选）。本策略只强制「4 类各含 1 个」+ 长度 12，
//! 最弱合法密码（如 `Aa1!!!!!!!!`：1 大写 + 1 小写 + 1 数字 + 9 同一符号）实际熵
//! 远低于 79 bit。Argon2id (t=3, m=64MiB) 为弱密码兜底，抵抗 GPU 离线爆破。

use anyhow::{anyhow, Result};

pub const MIN_MASTER_PASSWORD_LENGTH: usize = 12;

/// 校验主密码强度，不达标返 Err（user-safe message）。
///
/// 4 个入口调用：`unlock::setup_vault` / `unlock::change_master_password` /
/// `vault_commands::vault_setup` / `vault_commands::vault_change_password`。
/// 前 2 个在 vault crate 内调用，后 2 个透传到前 2 个——所以 vault crate 校验
/// 已覆盖。
pub fn validate_master_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_MASTER_PASSWORD_LENGTH {
        return Err(anyhow!(
            "主密码长度不足（需 ≥{} 字符）",
            MIN_MASTER_PASSWORD_LENGTH
        ));
    }
    if !password.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(anyhow!("主密码必须包含大写字母"));
    }
    if !password.chars().any(|c| c.is_ascii_lowercase()) {
        return Err(anyhow!("主密码必须包含小写字母"));
    }
    if !password.chars().any(|c| c.is_ascii_digit()) {
        return Err(anyhow!("主密码必须包含数字"));
    }
    if !password.chars().any(is_symbol) {
        return Err(anyhow!("主密码必须包含符号"));
    }
    Ok(())
}

/// 符号判定——与前端 SYMBOL_CHARS 集合一致：
/// ASCII 标点 + 中文全角符号（覆盖中文用户输入习惯）。
fn is_symbol(c: char) -> bool {
    // ASCII 符号：所有非字母数字的 ASCII 可打印字符
    if c.is_ascii() && c.is_ascii_graphic() && !c.is_ascii_alphanumeric() {
        return true;
    }
    // 中文全角符号（与前端 SYMBOL_CHARS 中文段一致）
    matches!(
        c,
        '！' | '￥' | '…' | '（' | '）'
        | '—'
        | '【' | '】' | '「' | '」' | '『' | '』'
        | '；' | '：' | '“' | '”' | '’' | '‘'
        | '，' | '。' | '、'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_password_accepted() {
        assert!(validate_master_password("Abcdefghij1!").is_ok());
        assert!(validate_master_password("VeryStrong123#Pass").is_ok());
        // 全角符号也算
        assert!(validate_master_password("Abcdefghij1！").is_ok());
    }

    #[test]
    fn test_too_short_rejected() {
        assert!(validate_master_password("Abc1!").is_err());
        assert!(validate_master_password("Abcdefghij1!").is_ok()); // 12 字符刚好
        assert!(validate_master_password("Abcdefgh1!").is_err()); // 11 字符
    }

    #[test]
    fn test_missing_classes_rejected() {
        assert!(validate_master_password("abcdefghijkl!").is_err()); // 缺大写 + 数字
        assert!(validate_master_password("ABCDEFGHIJ1!").is_err()); // 缺小写
        assert!(validate_master_password("Abcdefghijk!").is_err()); // 缺数字
        assert!(validate_master_password("Abcdefghij12").is_err()); // 缺符号
    }

    /// 防前端绕过：DevTools invoke('vault_setup', {password: 'a'}) 必须被后端拒绝。
    #[test]
    fn test_one_char_password_rejected() {
        assert!(validate_master_password("a").is_err());
    }

    /// 空密码拒绝。
    #[test]
    fn test_empty_password_rejected() {
        assert!(validate_master_password("").is_err());
    }

    /// V4 守护（2026-07-24）：验证 is_symbol 覆盖前端 SYMBOL_CHARS 对齐的所有字符。
    ///
    /// 前后端符号集是双份手工实现（TS Set vs Rust matches!），无共享源。此测试
    /// 列举「按前端 SYMBOL_CHARS 应判 true」的字符（ASCII 全集标点 + 全角全集），
    /// 验证后端 is_symbol 全过——任一方改动导致漂移会在此暴露。
    ///
    /// 全角段必须与前端 `validateMasterPassword.ts` SYMBOL_CHARS 全角段逐字一致：
    ///   ！￥…（）—【】「」『』；：""''，。、
    /// （¥ U+00A5 已统一为 ￥ U+FFE5）
    #[test]
    fn is_symbol_covers_all_expected_chars() {
        // ASCII 标点全集：所有非字母数字的可打印 ASCII（码点 33-47, 58-64, 91-96, 123-126）
        for c in "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~".chars() {
            assert!(is_symbol(c), "ASCII 标点 '{}' 应判为 symbol", c);
        }
        // 字母数字不应判为 symbol
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".chars() {
            assert!(!is_symbol(c), "字母数字 '{}' 不应判为 symbol", c);
        }
        // 全角符号全集（与前端 SYMBOL_CHARS 全角段逐字对齐）
        for c in "！￥…（）—【】「」『』；：”“’‘，。、".chars() {
            assert!(is_symbol(c), "全角符号 '{}' (U+{:04X}) 应判为 symbol", c, c as u32);
        }
        // 边界：半角 ¥ (U+00A5) 不在后端全集（前端已统一为全角 ￥ U+FFE5）
        assert!(!is_symbol('¥'), "半角 ¥ (U+00A5) 不应判 symbol（已统一用全角 ￥ U+FFE5）");
        assert!(is_symbol('￥'), "全角 ￥ (U+FFE5) 应判 symbol");
    }
}
