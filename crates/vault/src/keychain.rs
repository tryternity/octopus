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

// 测试专用：thread-local 的 in-memory Keychain 覆盖。
//
// 与 `octopus_infra::db::set_test_db()` 同构：
//   - thread_local! + RefCell<Option<...>>（不用 OnceLock<Mutex<...>>，与 Wave 1 保持一致）
//   - 不用 `#[cfg(test)]` 门控，保持运行时可见，便于跨 crate
//     （octopus-vault / octopus-desktop）的单元测试调用
//   - 未设置时（生产 / 普通测试）所有公开函数走真实 OS Keychain 路径，
//     仅多一次 thread_local 读，对生产零影响。
thread_local! {
    static TEST_KEYCHAIN_OVERRIDE: std::cell::RefCell<Option<InMemoryKeychain>>
        = std::cell::RefCell::new(None);
}

/// 进程内 in-memory Keychain（仅测试用）。
///
/// 用 `service + "\0" + user` 作为复合 key 存 32B raw bytes（不走 base64，
/// 因为这不是真实 OS Keychain，无需 string 化）。`\0` 在 service / user 中不合法，
/// 可安全作为分隔符。
#[derive(Default)]
#[doc(hidden)]
pub struct InMemoryKeychain {
    store: std::collections::HashMap<String, Vec<u8>>,
}

impl InMemoryKeychain {
    fn make_key(service: &str, user: &str) -> String {
        format!("{}\0{}", service, user)
    }

    fn set(&mut self, service: &str, user: &str, val: Vec<u8>) {
        self.store.insert(Self::make_key(service, user), val);
    }

    fn get(&self, service: &str, user: &str) -> Option<&Vec<u8>> {
        self.store.get(&Self::make_key(service, user))
    }

    fn delete(&mut self, service: &str, user: &str) -> bool {
        self.store.remove(&Self::make_key(service, user)).is_some()
    }
}

/// 测试专用：注入一个空的 in-memory Keychain，后续 4 个公开函数会优先使用它。
///
/// 多次调用替换前一次注入（不累积）。线程隔离：仅影响调用线程。
#[doc(hidden)]
pub fn set_test_keychain() {
    TEST_KEYCHAIN_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = Some(InMemoryKeychain::default());
    });
}

/// 测试专用：清除 thread-local Keychain 覆盖，恢复真实 OS Keychain 路径。
#[doc(hidden)]
pub fn clear_test_keychain() {
    TEST_KEYCHAIN_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

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
    // 测试覆盖优先：若设置了 thread-local InMemoryKeychain（Some(_)），全部
    // 走它（无论是否存过 K_machine）——保持生产行为完全不变仅在 override 被启用时。
    let test_hit = TEST_KEYCHAIN_OVERRIDE.with(|cell| -> Result<Option<Option<Zeroizing<[u8; 32]>>>> {
        let b = cell.borrow();
        match *b {
            None => Ok(None), // 未设测试覆盖 → 交还真实路径
            Some(ref kc) => {
                match kc.get(KEYCHAIN_SERVICE, KEYCHAIN_USER) {
                    None => Ok(Some(None)), // override 设了但没存过
                    Some(bytes) => {
                        ensure!(bytes.len() == 32, "K_machine 长度异常：{} bytes", bytes.len());
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(bytes);
                        Ok(Some(Some(Zeroizing::new(arr))))
                    }
                }
            }
        }
    })?;
    if let Some(inner) = test_hit {
        return Ok(inner);
    }

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
    // 测试覆盖优先。
    let was_test = TEST_KEYCHAIN_OVERRIDE.with(|cell| {
        let mut b = cell.borrow_mut();
        if let Some(ref mut kc) = *b {
            kc.set(KEYCHAIN_SERVICE, KEYCHAIN_USER, key.to_vec());
            true
        } else {
            false
        }
    });
    if was_test {
        return Ok(());
    }

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
    // 测试覆盖优先。
    let was_test = TEST_KEYCHAIN_OVERRIDE.with(|cell| {
        let mut b = cell.borrow_mut();
        if let Some(ref mut kc) = *b {
            kc.delete(KEYCHAIN_SERVICE, KEYCHAIN_USER);
            true
        } else {
            false
        }
    });
    if was_test {
        return Ok(());
    }

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
