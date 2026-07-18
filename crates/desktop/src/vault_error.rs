//! User-safe vault error messages（follow-up #9）。
//!
//! 内部错误（anyhow）的链文本常含文件路径、SQL 片段、加密算法细节等不应暴露给
//! UI 的内容。本模块在 Tauri 命令边界把它们映射为稳定、用户友好的消息。
//!
//! 设计原则：
//! - **不**重构 vault crate 的 anyhow 用法——内部逻辑继续用 anyhow
//! - 仅命令层（`vault_commands.rs`）翻译为 `VaultError`
//! - 序列化为 `{ code, message }` JSON：前端可按 `code` 程序化处理（如弹解锁框），
//!   `message` 直接展示给用户
//! - `InternalError` 是兜底——**绝不**包含内部细节
//!
//! 与前端的契约见 `crates/desktop/frontend/src/pages/VaultPicker/classifyError.ts`：
//! 前端先尝试 JSON parse（新格式），失败则退回旧的字符串匹配（向后兼容）。

use std::fmt;

/// 用户安全的 vault 错误枚举。
///
/// 所有变体的 `Display` / `user_message` 都不含文件路径、SQL 片段、加密内部状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    /// vault 尚未初始化（首次运行）——前端提示去 Settings 创建。
    NotInitialized,
    /// vault 已初始化但当前会话未解锁——前端弹内联解锁表单。
    Locked,
    /// 主密码错误（unlock / change_password）。
    InvalidMasterPassword,
    /// 指定 id 的 cipher 不存在（已删除 / id 错）。
    CipherNotFound(i64),
    /// 通用用户输入错误（DTO 校验失败等）。
    InvalidInput(String),
    /// TOTP secret 不是合法 Base32。
    TotpInvalidSecret,
    /// 导入失败（Bitwarden JSON 格式错误等）。仅携带用户安全上下文。
    ImportFailed(String),
    /// 系统密钥串不可用（macOS Keychain 拒访 / Linux 无 secret service）。
    KeychainUnavailable,
    /// 剪贴板写入失败（权限 / 系统资源）。
    ClipboardError,
    /// Auto-Type 失败（已 fallback 到剪贴板）。
    AutoTypeFailed,
    /// 兜底——**绝不**包含内部细节。仅提示用户重试 / 重启。
    InternalError,
}

impl VaultError {
    /// 用户安全消息（无文件路径、无内部结构细节）。
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::NotInitialized => "密码库尚未初始化，请先在设置中创建",
            Self::Locked => "密码库已锁定，请输入主密码解锁",
            Self::InvalidMasterPassword => "主密码错误",
            Self::CipherNotFound(_) => "未找到该密码条目",
            Self::InvalidInput(_) => "输入无效，请检查后重试",
            Self::TotpInvalidSecret => "TOTP 密钥格式无效（应为 Base32 字符串）",
            Self::ImportFailed(_) => "导入失败，请检查文件格式",
            Self::KeychainUnavailable => "系统密钥串不可用，请联系管理员或换用主密码解锁",
            Self::ClipboardError => "剪贴板写入失败",
            Self::AutoTypeFailed => "自动填充失败，已改用剪贴板复制",
            Self::InternalError => "内部错误，请重试；如问题持续请重启应用",
        }
    }

    /// 稳定错误码，供前端程序化处理（如 locked → 弹解锁框）。
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotInitialized => "not_initialized",
            Self::Locked => "locked",
            Self::InvalidMasterPassword => "invalid_master_password",
            Self::CipherNotFound(_) => "cipher_not_found",
            Self::InvalidInput(_) => "invalid_input",
            Self::TotpInvalidSecret => "totp_invalid",
            Self::ImportFailed(_) => "import_failed",
            Self::KeychainUnavailable => "keychain_unavailable",
            Self::ClipboardError => "clipboard_error",
            Self::AutoTypeFailed => "autotype_failed",
            Self::InternalError => "internal_error",
        }
    }
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.user_message())
    }
}

impl std::error::Error for VaultError {}

/// 把内部 anyhow 错误分类为用户安全的 `VaultError`。
///
/// 基于 anyhow 链文本的启发式匹配（anyhow chain 是字符串）。无法分类的兜底为
/// [`VaultError::InternalError`]——**绝不**把内部错误细节透传给前端。
///
/// 关键匹配规则与 vault crate 实际错误字符串对齐（见
/// `crates/vault/src/unlock.rs` 等）：
/// - `"vault 未初始化"` / `"not initialized"` → `NotInitialized`
/// - `"vault 未解锁"` / `"vault 已锁定"` / `"locked"` → `Locked`
/// - `"旧主密码错误"` / `"主密码错误"` / `"invalid master"` → `InvalidMasterPassword`
/// - `"cipher"` + (`"不存在"` / `"not found"`) → `CipherNotFound(0)`（id 提取留待后续）
/// - `"totp"` + (`"base32"` / `"格式"` / `"invalid"`) → `TotpInvalidSecret`
/// - `"keychain"` / `"密钥串"` → `KeychainUnavailable`
/// - `"clipboard"` / `"剪贴板"` → `ClipboardError`
/// - `"autotype"` / `"自动填充"` → `AutoTypeFailed`
pub fn classify(err: &anyhow::Error) -> VaultError {
    let msg = err.to_string();
    let chain: String = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    // 拼接 head 错误 + 全链，统一小写后子串匹配——链上下文可能让 head 不含关键字
    // （如 head 是 SQL error，链上层用 .context("vault 未初始化") 包装）。
    let combined = format!("{} {}", msg, chain).to_lowercase();

    if combined.contains("vault 未初始化") || combined.contains("not initialized") {
        return VaultError::NotInitialized;
    }
    if combined.contains("vault 未解锁")
        || combined.contains("vault 已锁定")
        || combined.contains("locked")
    {
        return VaultError::Locked;
    }
    if combined.contains("主密码错误")
        || combined.contains("旧主密码错误")
        || combined.contains("invalid master")
    {
        return VaultError::InvalidMasterPassword;
    }
    if combined.contains("cipher") && (combined.contains("不存在") || combined.contains("not found"))
    {
        // id 提取较脆弱（cipher id 可能不出现在错误链里），统一返回 0——
        // 前端按 code=cipher_not_found 处理即可，不需要 id。
        return VaultError::CipherNotFound(0);
    }
    if combined.contains("totp")
        && (combined.contains("base32") || combined.contains("格式") || combined.contains("invalid"))
    {
        return VaultError::TotpInvalidSecret;
    }
    if combined.contains("keychain") || combined.contains("密钥串") {
        return VaultError::KeychainUnavailable;
    }
    if combined.contains("clipboard") || combined.contains("剪贴板") {
        return VaultError::ClipboardError;
    }
    if combined.contains("autotype") || combined.contains("自动填充") {
        return VaultError::AutoTypeFailed;
    }
    // 密码生成器输入校验（length / word_count / 字符集为空）——ensure! 文案以
    // "必须" / "至少需要" 开头。把 head 错误文案完整透传给前端（这些都是我们自己
    // 写的用户友好文案，无内部细节）。
    if combined.contains("必须") || combined.contains("至少需要") {
        return VaultError::InvalidInput(msg);
    }
    VaultError::InternalError
}

/// 序列化 `VaultError` 为 Tauri 用的 JSON 字符串：`{ code, message }`。
///
/// 前端可对 `code` 分支处理（如 locked → 弹解锁框），`message` 直接展示。
///
/// 对携带动态消息的变体（`InvalidInput` / `ImportFailed`）直接使用其 payload——
/// 这些消息由我们自己控制（如生成器校验文案），用户友好且不含内部细节。
/// 其它变体走 `user_message()`（稳定文案）。
pub fn serialize(err: &VaultError) -> String {
    let message = match err {
        VaultError::InvalidInput(m) if !m.is_empty() => m.clone(),
        VaultError::ImportFailed(m) if !m.is_empty() => m.clone(),
        _ => err.user_message().to_string(),
    };
    serde_json::json!({
        "code": err.code(),
        "message": message,
    })
    .to_string()
}

/// 把 `anyhow::Error` 转 `VaultError` 再序列化为 JSON 字符串。
///
/// 适配 Tauri 命令的 `Result<T, String>` 返回类型——命令层用
/// `.map_err(vault_error::to_tauri_error)?` 替换原 `.map_err(|e| e.to_string())?`。
pub fn to_tauri_error(err: anyhow::Error) -> String {
    serialize(&classify(&err))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个 anyhow 错误（带可选上下文链）。第一个 msg 是 head，后续是
    /// `.context(...)` 包装，模拟 vault crate 的 `.context("vault 未初始化")?` 链。
    ///
    /// `context` 要求 `'static`，故把 `&str` 拷贝为 `String`（测试输入字面量，无开销）。
    fn err_with_chain(msgs: &[&str]) -> anyhow::Error {
        match msgs.split_first() {
            None => anyhow::anyhow!(""),
            Some((head, rest)) => {
                let mut e = anyhow::anyhow!("{}", head);
                for ctx in rest {
                    e = e.context((*ctx).to_string());
                }
                e
            }
        }
    }

    /// NotInitialized：vault crate 的 `"vault 未初始化"` context 应被识别。
    #[test]
    fn classify_not_initialized_zh() {
        let e = err_with_chain(&["db error", "vault 未初始化"]);
        assert_eq!(classify(&e), VaultError::NotInitialized);
    }

    /// NotInitialized：英文文案也兼容（未来若改文案）。
    #[test]
    fn classify_not_initialized_en() {
        let e = err_with_chain(&["Error: vault is not initialized"]);
        assert_eq!(classify(&e), VaultError::NotInitialized);
    }

    /// Locked：require_user_vault_key 直接返回 VaultError::Locked（不走 classify），
    /// 但其他链中的 `"locked"` / `"vault 已锁定"` 应识别。
    #[test]
    fn classify_locked() {
        let e = err_with_chain(&["vault 已锁定"]);
        assert_eq!(classify(&e), VaultError::Locked);

        let e = err_with_chain(&["session locked"]);
        assert_eq!(classify(&e), VaultError::Locked);
    }

    /// InvalidMasterPassword：unlock.rs 的 `"旧主密码错误"` context。
    #[test]
    fn classify_invalid_master_password() {
        let e = err_with_chain(&["旧主密码错误"]);
        assert_eq!(classify(&e), VaultError::InvalidMasterPassword);

        let e = err_with_chain(&["Error: invalid master password"]);
        assert_eq!(classify(&e), VaultError::InvalidMasterPassword);
    }

    /// CipherNotFound：command 层抛 `"cipher 42 不存在"`。
    #[test]
    fn classify_cipher_not_found() {
        let e = err_with_chain(&["cipher 42 不存在"]);
        assert_eq!(classify(&e), VaultError::CipherNotFound(0));

        let e = err_with_chain(&["cipher 42 not found"]);
        assert_eq!(classify(&e), VaultError::CipherNotFound(0));
    }

    /// TotpInvalidSecret：totp.rs 的 `"TOTP secret Base32 解码失败"` context。
    #[test]
    fn classify_totp_invalid_secret() {
        let e = err_with_chain(&["TOTP secret Base32 解码失败"]);
        assert_eq!(classify(&e), VaultError::TotpInvalidSecret);
    }

    /// KeychainUnavailable：含 `"keychain"` 或 `"密钥串"`。
    #[test]
    fn classify_keychain_unavailable() {
        let e = err_with_chain(&["macOS keychain denied access"]);
        assert_eq!(classify(&e), VaultError::KeychainUnavailable);

        let e = err_with_chain(&["系统密钥串不可用"]);
        assert_eq!(classify(&e), VaultError::KeychainUnavailable);
    }

    /// ClipboardError：含 `"clipboard"` 或 `"剪贴板"`。
    #[test]
    fn classify_clipboard_error() {
        let e = err_with_chain(&["clipboard write failed"]);
        assert_eq!(classify(&e), VaultError::ClipboardError);

        let e = err_with_chain(&["剪贴板写入失败"]);
        assert_eq!(classify(&e), VaultError::ClipboardError);
    }

    /// AutoTypeFailed：含 `"autotype"` 或 `"自动填充"`。
    #[test]
    fn classify_autotype_failed() {
        let e = err_with_chain(&["autotype keystroke injection failed"]);
        assert_eq!(classify(&e), VaultError::AutoTypeFailed);

        let e = err_with_chain(&["自动填充失败"]);
        assert_eq!(classify(&e), VaultError::AutoTypeFailed);
    }

    /// InvalidInput：密码生成器 ensure! 文案以 "必须" / "至少需要" 开头。
    /// 需把完整的 head 错误文案透传给前端（我们自己写的用户友好文案）。
    #[test]
    fn classify_generator_invalid_input() {
        let e = err_with_chain(&["密码长度必须 ≥ 5（当前 4）"]);
        let classified = classify(&e);
        assert_eq!(classified.code(), "invalid_input");
        assert_eq!(
            serialize(&classified),
            serde_json::json!({
                "code": "invalid_input",
                "message": "密码长度必须 ≥ 5（当前 4）"
            })
            .to_string()
        );

        let e = err_with_chain(&["至少需要启用一种字符类型（大写/小写/数字/符号）"]);
        assert_eq!(classify(&e).code(), "invalid_input");

        let e = err_with_chain(&["中文短语词数必须 ≤ 8（当前 9）"]);
        assert_eq!(classify(&e).code(), "invalid_input");
    }

    /// 兜底：不可分类的错误（SQL error、文件路径、加密内部）→ InternalError。
    /// 关键不变量：**不**透传内部细节。
    ///
    /// 注意 `"database is locked"` 会被误识别为 `Locked`（启发式的固有局限）；
    /// 此处特意用不含锁定关键字的真实内部错误示例。
    #[test]
    fn classify_internal_fallback() {
        // 真实 SQL 内部错误（含路径）——不应透传给前端
        let e = err_with_chain(&["rusqlite Error: no such table: ciphers at /Users/x/.octopus/db.sqlite"]);
        assert_eq!(classify(&e), VaultError::InternalError);

        // 加密内部细节——不应透传
        let e = err_with_chain(&["ChaCha20Poly1305 AEAD decryption failure: nonce mismatch"]);
        assert_eq!(classify(&e), VaultError::InternalError);

        // 纯路径 / IO 错误
        let e = err_with_chain(&["Os { code: 13, kind: PermissionDenied, message: \"Permission denied\" }"]);
        assert_eq!(classify(&e), VaultError::InternalError);
    }

    /// 序列化：`{ code, message }` JSON 结构稳定。
    #[test]
    fn serialize_shape() {
        let s = serialize(&VaultError::Locked);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert_eq!(v["code"], "locked");
        assert_eq!(v["message"], "密码库已锁定，请输入主密码解锁");
    }

    /// to_tauri_error：anyhow → JSON 字符串。
    #[test]
    fn to_tauri_error_maps_anyhow() {
        let e = anyhow::anyhow!("vault 未初始化");
        let s = to_tauri_error(e);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        assert_eq!(v["code"], "not_initialized");
    }

    /// code() 应稳定（前端按 code 分支，不能改）。
    #[test]
    fn codes_are_stable() {
        assert_eq!(VaultError::NotInitialized.code(), "not_initialized");
        assert_eq!(VaultError::Locked.code(), "locked");
        assert_eq!(VaultError::InvalidMasterPassword.code(), "invalid_master_password");
        assert_eq!(VaultError::CipherNotFound(0).code(), "cipher_not_found");
        assert_eq!(VaultError::InvalidInput("x".into()).code(), "invalid_input");
        assert_eq!(VaultError::TotpInvalidSecret.code(), "totp_invalid");
        assert_eq!(VaultError::ImportFailed("x".into()).code(), "import_failed");
        assert_eq!(VaultError::KeychainUnavailable.code(), "keychain_unavailable");
        assert_eq!(VaultError::ClipboardError.code(), "clipboard_error");
        assert_eq!(VaultError::AutoTypeFailed.code(), "autotype_failed");
        assert_eq!(VaultError::InternalError.code(), "internal_error");
    }

    /// Display 不应暴露内部细节（即等于 user_message）。
    #[test]
    fn display_matches_user_message() {
        assert_eq!(
            format!("{}", VaultError::NotInitialized),
            VaultError::NotInitialized.user_message()
        );
        assert_eq!(
            format!("{}", VaultError::InternalError),
            VaultError::InternalError.user_message()
        );
    }
}
