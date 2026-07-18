//! HMAC-SHA512 child() 派生（简化 BIP44 思想）。
//!
//! child_key = HMAC-SHA512(parent_key, label)[..32]
//! 后 32B 在完整 BIP32 中是 chain code，octopus 不用。
//!
//! label 固定（spec 第 2.1 节）：
//! - b"octopus/v1/user-vault"   → 加密 cipher
//! - b"octopus/v1/app-secrets"  → 加密 API Key
//! - b"octopus/v1/sync"         → 预留（MVP 不生成）
//! - b"octopus/v1/send"         → 预留（MVP 不生成）

use hmac::{Hmac, Mac};
use sha2::Sha512;

use super::DerivedKey;

/// 固定派生 label（spec INV：不可改）。
pub const LABEL_USER_VAULT: &[u8] = b"octopus/v1/user-vault";
pub const LABEL_APP_SECRETS: &[u8] = b"octopus/v1/app-secrets";
pub const LABEL_SYNC: &[u8] = b"octopus/v1/sync";
pub const LABEL_SEND: &[u8] = b"octopus/v1/send";

impl DerivedKey {
    /// 从当前 key 派生子 key。HMAC-SHA512，取前 32B。
    pub fn child(&self, label: &[u8]) -> DerivedKey {
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(self.as_bytes())
            .expect("HMAC 接受任意 key 长度");
        mac.update(label);
        let result = mac.finalize().into_bytes();
        let mut child = [0u8; 32];
        child.copy_from_slice(&result[..32]);
        DerivedKey(crate::Zeroizing::new(child))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey(crate::Zeroizing::new([byte; 32]))
    }

    #[test]
    fn test_child_deterministic() {
        let parent = make_key(42);
        let c1 = parent.child(LABEL_USER_VAULT);
        let c2 = parent.child(LABEL_USER_VAULT);
        assert_eq!(c1.as_bytes(), c2.as_bytes());
    }

    #[test]
    fn test_different_labels_different_children() {
        let parent = make_key(42);
        let user_vault = parent.child(LABEL_USER_VAULT);
        let app_secrets = parent.child(LABEL_APP_SECRETS);
        assert_ne!(user_vault.as_bytes(), app_secrets.as_bytes());
    }

    #[test]
    fn test_different_parents_different_children() {
        let p1 = make_key(1);
        let p2 = make_key(2);
        let c1 = p1.child(LABEL_USER_VAULT);
        let c2 = p2.child(LABEL_USER_VAULT);
        assert_ne!(c1.as_bytes(), c2.as_bytes());
    }

    #[test]
    fn test_child_different_from_parent() {
        let parent = make_key(42);
        let child = parent.child(LABEL_USER_VAULT);
        assert_ne!(parent.as_bytes(), child.as_bytes());
    }

    #[test]
    fn test_labels_immutable() {
        // 防止后续手贱改 label（spec INV）
        assert_eq!(LABEL_USER_VAULT, b"octopus/v1/user-vault");
        assert_eq!(LABEL_APP_SECRETS, b"octopus/v1/app-secrets");
        assert_eq!(LABEL_SYNC, b"octopus/v1/sync");
        assert_eq!(LABEL_SEND, b"octopus/v1/send");
    }
}
