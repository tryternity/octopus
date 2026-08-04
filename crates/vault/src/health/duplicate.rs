//! 重复密码检测（内存 SHA-256，不持久化 hash）。

use std::collections::HashMap;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::types::{Cipher, CipherData};

/// 一组密码相同（同 SHA-256）的 cipher。
///
/// 修复 #12：去掉了 `derive(Debug)`——派生的 Debug 会打印 `password_hash`
/// （SHA-256 hex），属于敏感信息；改手写 impl 对 `password_hash` redact。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    /// SHA-256(password)，仅用于分组；不跨 IPC 输出，避免泄露哈希。
    /// （final-review #5/M2）
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub cipher_ids: Vec<String>,
}

impl std::fmt::Debug for DuplicateGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 修复 #12：password_hash 是 SHA-256 hex（敏感），Debug 输出统一 redact，
        // 防止日志 / 错误信息意外泄露哈希。cipher_ids 不敏感，原样打印。
        f.debug_struct("DuplicateGroup")
            .field("password_hash", &"<redacted>")
            .field("cipher_ids", &self.cipher_ids)
            .finish()
    }
}

/// H1 修复（2026-07-24）：签名改为 &[&Cipher]——避免调用方深拷贝整个 cipher 列表
/// （之前 generate_report 行 46 把 Vec<&Cipher> clone 成 Vec<Cipher> 仅为匹配签名）。
pub fn find_duplicates(ciphers: &[&Cipher]) -> Vec<DuplicateGroup> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for c in ciphers {
        // L6 修复（2026-07-24）：跳过软删/回收站的 cipher——它们不应参与重复检测。
        if c.is_deleted {
            continue;
        }
        // CipherData 当前仅 Login 单变体；保留 if let 以便未来扩展 SecureNote/Card/Identity。
        #[allow(irrefutable_let_patterns)]
        if let CipherData::Login(login) = &c.data {
            if let Some(pwd) = &login.password {
                let mut hasher = Sha256::new();
                hasher.update(pwd.as_bytes());
                let hash = hasher.finalize();
                let hash_hex = data_encoding::HEXLOWER.encode(&hash);
                map.entry(hash_hex).or_default().push(c.id.clone());
            }
        }
    }
    // D5 修复（2026-07-25）：收集后按首个 cipher_id 排序，保证 duplicate_groups
    // 组间顺序确定（HashMap 迭代顺序不确定）。组内 cipher_ids 顺序已确定（按遍历
    // push，:51）。稳定的顺序让健康报告的重复组列表不会每次刷新都变。
    let mut groups: Vec<DuplicateGroup> = map
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(password_hash, cipher_ids)| DuplicateGroup {
            password_hash,
            cipher_ids,
        })
        .collect();
    groups.sort_by(|a, b| {
        // 按 cipher_ids 的首个 id 排序（每组至少 2 个 id，[0] 安全）
        a.cipher_ids[0].cmp(&b.cipher_ids[0])
    });
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CipherType, LoginData, RepromptType};

    fn make_cipher(id: &str, password: Option<&str>) -> Cipher {
        Cipher {
            id: id.to_string(),
            folder_id: None,
            favorite: false,
            atype: CipherType::Login,
            name: format!("c-{}", id),
            notes: None,
            data: CipherData::Login(LoginData {
                uris: vec![],
                username: None,
                password: password.map(String::from),
                totp: None,
                password_revision_date: None,
            }),
            fields: vec![],
            password_history: vec![],
            reprompt: RepromptType::None,
            is_deleted: false,
            created_at: "2026-07-18".into(),
            updated_at: "2026-07-18".into(),
        }
    }

    #[test]
    fn test_no_duplicates() {
        let ciphers = [make_cipher("c1", Some("a")),
            make_cipher("c2", Some("b")),
            make_cipher("c3", Some("c"))];
        assert!(find_duplicates(&ciphers.iter().collect::<Vec<_>>()).is_empty());
    }

    #[test]
    fn test_finds_duplicates() {
        let ciphers = [make_cipher("c1", Some("same")),
            make_cipher("c2", Some("same")),
            make_cipher("c3", Some("different")),
            make_cipher("c4", Some("same"))];
        let groups = find_duplicates(&ciphers.iter().collect::<Vec<_>>());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].cipher_ids.len(), 3);
    }

    #[test]
    fn test_multiple_duplicate_groups() {
        let ciphers = [make_cipher("c1", Some("a")),
            make_cipher("c2", Some("a")),
            make_cipher("c3", Some("b")),
            make_cipher("c4", Some("b")),
            make_cipher("c5", Some("unique"))];
        let groups = find_duplicates(&ciphers.iter().collect::<Vec<_>>());
        assert_eq!(groups.len(), 2);
    }

    /// D5 回归守护（2026-07-25）：duplicate_groups 组间顺序应确定（按首个 cipher_id）。
    ///
    /// 之前 HashMap.into_iter() 顺序不确定，健康报告的重复组列表每次刷新可能变。
    /// 现在收集后排序，多次调用结果一致。组内 cipher_ids 顺序也已确定（按遍历 push）。
    #[test]
    fn test_duplicate_groups_order_stable() {
        // 构造 3 组重复（首个 cipher_id 分别 c1/c3/c5），故意打乱 hash 顺序
        // （"z"/"a"/"m" 的 hash 顺序与 cipher_id 顺序无关）
        // clippy 误报 vec!——这是数组字面量非 vec![x; N]
        #[allow(clippy::useless_vec)]
        let ciphers = vec![
            make_cipher("c1", Some("zzz")), // group Z: c1, c2
            make_cipher("c2", Some("zzz")),
            make_cipher("c3", Some("aaa")), // group A: c3, c4
            make_cipher("c4", Some("aaa")),
            make_cipher("c5", Some("mmm")), // group M: c5, c6
            make_cipher("c6", Some("mmm")),
        ];
        // 多次调用验证顺序稳定
        for _ in 0..20 {
            let groups = find_duplicates(&ciphers.iter().collect::<Vec<_>>());
            assert_eq!(groups.len(), 3, "应有 3 组重复");
            // 按首个 cipher_id 升序：c1 < c3 < c5
            assert_eq!(
                groups[0].cipher_ids[0], "c1",
                "D5: 首组应是 c1（排序后），实际 {}",
                groups[0].cipher_ids[0]
            );
            assert_eq!(
                groups[1].cipher_ids[0], "c3",
                "D5: 第二组应是 c3，实际 {}",
                groups[1].cipher_ids[0]
            );
            assert_eq!(
                groups[2].cipher_ids[0], "c5",
                "D5: 第三组应是 c5，实际 {}",
                groups[2].cipher_ids[0]
            );
        }
    }

    #[test]
    fn test_skip_none_password() {
        let ciphers = [make_cipher("c1", None), make_cipher("c2", None)];
        assert!(find_duplicates(&ciphers.iter().collect::<Vec<_>>()).is_empty());
    }

    /// L6 修复回归守护：软删/回收站的 cipher 不应参与重复检测。
    #[test]
    fn test_skip_deleted_ciphers() {
        let mut c1 = make_cipher("c1", Some("same"));
        let c2 = make_cipher("c2", Some("same"));
        c1.is_deleted = true; // c1 软删
        let ciphers = [c1, c2];
        // c1 被过滤后只剩 c2（无重复）——不应报告重复组
        let groups = find_duplicates(&ciphers.iter().collect::<Vec<_>>());
        assert!(
            groups.is_empty(),
            "软删 cipher 不应参与重复检测（L6），实际 {} 组",
            groups.len()
        );
    }

    /// #12：Debug 输出必须 redact password_hash（SHA-256 hex，敏感），
    /// 不能让日志 / 错误信息意外泄露哈希。cipher_ids 应正常显示。
    #[test]
    fn test_debug_redacts_password_hash() {
        let ciphers = [make_cipher("redact-1", Some("topsecret")),
            make_cipher("redact-2", Some("topsecret"))];
        let groups = find_duplicates(&ciphers.iter().collect::<Vec<_>>());
        assert_eq!(groups.len(), 1);

        let debug_str = format!("{:?}", groups[0]);
        assert!(
            debug_str.contains("<redacted>"),
            "Debug 应对 password_hash redact，got: {}",
            debug_str
        );
        assert!(
            !debug_str.contains(&groups[0].password_hash),
            "Debug 不应泄露原始 hash：{}，got: {}",
            groups[0].password_hash,
            debug_str
        );
        // cipher_ids 不敏感，应仍可见
        assert!(
            debug_str.contains("cipher_ids"),
            "Debug 应包含 cipher_ids 字段，got: {}",
            debug_str
        );
    }
}
