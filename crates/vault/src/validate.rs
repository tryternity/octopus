//! 主密码强度校验（后端版，复审报告 #1 修复）。
//!
//! 翻译自前端 `validateMasterPassword.ts`——策略：长度 ≥ 12 + 必含 4 类字符
//! （大写 / 小写 / 数字 / 符号）。spec INV-10 / §7.4 / F19 明确要求「前端 +
//! 后端双校验，防前端绕过」——DevTools 可直接 invoke('vault_setup', {password: 'a'})
//! 设任意弱密码，前端校验不可信。
//!
//! 字符集：95 可打印 ASCII × 12 位 ≈ 79 bit 熵，配合 Argon2id (t=3, m=64MiB)
//! 可抵抗 GPU 离线暴力。

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
}
