//! K_machine 在 OS Keychain 的存取。
//!
//! macOS: Keychain Services
//! Windows: Credential Manager
//! Linux: Secret Service（需 gnome-keyring / KDE Wallet，否则降级到每次输主密码）

use anyhow::{ensure, Context, Result};
use keyring::Entry;

use crate::crypto::util::random_32;
use crate::Zeroizing;

pub const KEYCHAIN_SERVICE: &str = "octopus-vault";
pub const KEYCHAIN_USER: &str = "machine-key";

/// 读取或创建 K_machine。
///
/// - 首次调用（不存在）→ 生成新 32B 随机 key 存入 Keychain，返回
/// - 后续调用 → 读已有 key 返回
///
/// 失败场景：Linux 无 secret service → 返回 Err（调用方应降级到方案 Y）
pub fn load_or_create_machine_key() -> Result<Zeroizing<[u8; 32]>> {
    if let Some(existing) = load_machine_key()? {
        return Ok(existing);
    }
    let new_key = random_32();
    save_machine_key(&new_key)?;
    Ok(Zeroizing::new(new_key))
}

/// 读取已有 K_machine。不存在返回 Ok(None)。
pub fn load_machine_key() -> Result<Option<Zeroizing<[u8; 32]>>> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)
        .context("无法访问 OS Keychain")?;
    match entry.get_password() {
        Ok(s) => {
            let bytes = crate::crypto::util::base64_decode(&s)?;
            ensure!(bytes.len() == 32, "K_machine 长度异常：{} bytes", bytes.len());
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Ok(Some(Zeroizing::new(arr)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("读取 K_machine 失败"),
    }
}

/// 保存 K_machine（覆盖式）。
pub fn save_machine_key(key: &[u8; 32]) -> Result<()> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)
        .context("无法访问 OS Keychain")?;
    // Keychain API 接收 String（password 风格），把 32B 当 UTF-8 直接转
    // 注意：32B 随机可能含无效 UTF-8，所以用 base64 编码后存
    let s = crate::crypto::util::base64_encode(key);
    entry.set_password(&s).context("写入 K_machine 失败")?;
    Ok(())
}

/// 删除 K_machine（仅测试 / reset 用）。
pub fn delete_machine_key() -> Result<()> {
    let entry = Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER)
        .context("无法访问 OS Keychain")?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("删除 K_machine 失败"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ⚠️ 真实 Keychain 测试，需要在本地跑（会弹授权框）
    #[test]
    #[ignore]
    fn test_machine_key_round_trip() {
        // 清理旧数据
        let _ = delete_machine_key();

        // 首次读应不存在
        assert!(load_machine_key().unwrap().is_none());

        // 创建
        let k1 = load_or_create_machine_key().unwrap();
        assert_eq!(k1.len(), 32);

        // 再次读应能拿到同一把
        let k2 = load_machine_key().unwrap().unwrap();
        assert_eq!(k1.as_ref(), k2.as_ref());

        // 再调 load_or_create 应返回同一把（不覆盖）
        let k3 = load_or_create_machine_key().unwrap();
        assert_eq!(k1.as_ref(), k3.as_ref());

        // 清理
        delete_machine_key().unwrap();
        assert!(load_machine_key().unwrap().is_none());
    }
}
