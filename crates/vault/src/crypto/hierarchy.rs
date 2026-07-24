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
    ///
    /// 修复 #10：HMAC-SHA512 输出 64B——前 32B 是 child key（已 Zeroizing 包装），
    /// 后 32B 是 chain code（octopus 不用）。
    ///
    /// **A2 修复（第五轮审查）**：#10 的原实现把 `bytes`（GenericArray）的拷贝
    /// 进 Zeroizing 数组后清零，但原件 `bytes` 自身 drop 时不清零——后 32B chain
    /// code 仍残留栈帧。报告建议的 `mac.finalize_into(&mut Zeroizing<[u8;64]>)`
    /// 类型不成立（finalize_into 签名是 `&mut GenericArray<u8, U64>`）。
    ///
    /// 正解：启用 generic-array 的 zeroize feature（vault/Cargo.toml 显式声明），
    /// `GenericArray<u8, U64>` 自动实现 `Zeroize`，再用 `Zeroizing` 包装 `bytes`
    /// 本体，scope 结束时**原件**被清零——chain code 不再残留。
    pub fn child(&self, label: &[u8]) -> DerivedKey {
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(self.as_bytes())
            .expect("HMAC 接受任意 key 长度");
        mac.update(label);
        // Zeroizing 包装 GenericArray 本体——Deref<Target=GenericArray> 让
        // copy_from_slice 仍可用；scope 结束时直接对原件 zeroize。
        let bytes = crate::Zeroizing::new(mac.finalize().into_bytes());
        // C1 修复（2026-07-24）：用 Zeroizing<[u8;32]> 借用写入，move 整个 Zeroizing。
        // 之前用裸 [u8;32]（Copy 类型）→ Zeroizing::new(child) 是复制，原栈数组
        // drop 时不清零（child key 残留栈帧）。与 A2（chain code 清零）同类修复。
        let mut child = crate::Zeroizing::new([0u8; 32]);
        child.copy_from_slice(&bytes[..32]);
        DerivedKey::from_zeroizing(child)
        // `bytes` 在此 drop——Zeroizing 触发 GenericArray::zeroize() 清零整个 64B
        //（含后 32B chain code）。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(byte: u8) -> DerivedKey {
        DerivedKey::from_raw([byte; 32])
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

    // === A2 修复验证（第五轮审查） ===
    //
    // GenericArray<u8, U64> 启用 generic-array zeroize feature 后自动实现 Zeroize。
    // 这条编译期断言锁住该 feature 依赖：若有人误删 Cargo.toml 里的 features 配置，
    // 编译会断（zeroize trait 找不到），此测试也跟着断。

    /// 编译型测试：GenericArray<u8, U64> 实现 Zeroize。
    /// 若 Cargo.toml 误删 generic-array 的 zeroize feature，此函数无法编译。
    #[allow(dead_code)]
    fn _assert_generic_array_zeroize_compiles()
    where
        generic_array::GenericArray<u8, generic_array::typenum::U64>: zeroize::Zeroize,
    {
    }

    /// A2：Zeroizing 包装的 GenericArray 在 child() scope 结束时会被 zeroize。
    ///
    /// 行为上不可直接断言栈帧被清零，但可验证：
    ///   1. child() 仍正确 deterministic（同 key + 同 label → 同 child）
    ///   2. Zeroizing 包装的 GenericArray 调 zeroize 不 panic
    ///   3. child 与 label 不同 → 不同（防止有人误改成"返回整个 64B"等错误回归）
    #[test]
    fn test_child_still_deterministic_after_a2_fix() {
        let parent = make_key(99);
        let c1 = parent.child(LABEL_USER_VAULT);
        let c2 = parent.child(LABEL_USER_VAULT);
        let c3 = parent.child(LABEL_APP_SECRETS);
        assert_eq!(c1.as_bytes(), c2.as_bytes(), "A2 修复后仍应 deterministic");
        assert_ne!(
            c1.as_bytes(),
            c3.as_bytes(),
            "A2 修复后不同 label 仍应派生不同 child"
        );

        // 验证 GenericArray 的 zeroize trait 可调（编译型 + 运行时不 panic）
        let mut arr: generic_array::GenericArray<u8, generic_array::typenum::U64> =
            Default::default();
        // 给数组填点值，调 zeroize 看是否能正常清零（不 panic）
        for i in 0..64 {
            arr[i] = (i as u8) ^ 0xAA;
        }
        use zeroize::Zeroize;
        arr.zeroize();
        for i in 0..64 {
            assert_eq!(arr[i], 0, "zeroize 应把 GenericArray 清零（i={}）", i);
        }
    }
}
