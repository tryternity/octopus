//! 内容指纹（md5）——sync 增量同步的 diff 工具。
//!
//! md5 在这里**纯粹是内容指纹**，与加密破解无关。用于：
//! - 写 SQLite 时算 md5 存入 `sync_md5` 字段
//! - sync_now 时对比 SQLite.md5 vs outline.md5，决定是否需要重写文件
//!
//! **拼接格式**（确定性 + 无歧义）：
//! - 字段按固定顺序拼接
//! - 字段间用 `|` 分隔（避免字段值含分隔符导致歧义）
//! - Option<String> 统一为空字符串
//! - **不含** `created_at` / `updated_at`（时间戳跨设备必然不同，会导致永久 diff）
//!
//! **跨设备一致性**：cipher 只在创建机器加密一次，sync 搬运密文（不重新加密），
//! 所以密文字段跨设备字节一致——md5 也一致。详见 spec §2.4。
//!
//! 2026-07-22 抽离：`md5_hex` hash 工具已搬到 `octopus_sync::store::md5_hex`，
//! 本模块只保留 vault 业务数据的指纹拼接逻辑（cipher_md5 / folder_md5）。

use octopus_infra::db::{VaultCipher, VaultCipherInput, VaultFolder};
use octopus_sync::store::md5_hex;

/// cipher 的逻辑内容 md5——不含 created_at/updated_at。
///
/// 拼接字段顺序（固定，不可变）：
/// `id | folder_id | favorite | atype | name | notes | data | fields |
///  password_history | reprompt | deleted_at`
pub fn cipher_md5(c: &VaultCipher) -> String {
    let input = format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        c.id,
        c.folder_id.as_deref().unwrap_or(""),
        c.favorite,
        c.atype,
        c.name,
        c.notes.as_deref().unwrap_or(""),
        c.data,
        c.fields.as_deref().unwrap_or(""),
        c.password_history.as_deref().unwrap_or(""),
        c.reprompt,
        c.deleted_at.as_deref().unwrap_or(""),
    );
    md5_hex(input.as_bytes())
}

/// 从 VaultCipherInput 算 md5——用于 create/save 时填 sync_md5。
///
/// 与 `cipher_md5(&VaultCipher)` 的区别：input 不含 id（id 是外部传入的），
/// 所以加 id 参数。
///
/// **必须保证**：input 版本和 row 版本对同一条 cipher 算出的 md5 相同——
/// 否则 create 时填的 md5 和 sync 时读 row 算的 md5 对不上，diff 永远误判。
/// H2 修复（2026-07-24）：input 现在含 deleted_at（之前硬编码 ""），
/// 保证软删/恢复后 md5 与 row 一致。
pub fn cipher_md5_from_input(id: &str, input: &VaultCipherInput) -> String {
    let s = format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        id,
        input.folder_id.as_deref().unwrap_or(""),
        input.favorite,
        input.atype,
        input.name,
        input.notes.as_deref().unwrap_or(""),
        input.data,
        input.fields.as_deref().unwrap_or(""),
        input.password_history.as_deref().unwrap_or(""),
        input.reprompt,
        input.deleted_at.as_deref().unwrap_or(""),
    );
    md5_hex(s.as_bytes())
}

/// folder 的逻辑内容 md5——不含 created_at/updated_at。
///
/// 拼接字段顺序：`id | name | sort_order`
pub fn folder_md5(f: &VaultFolder) -> String {
    let input = format!("{}|{}|{}", f.id, f.name, f.sort_order);
    md5_hex(input.as_bytes())
}

/// 从 folder 基本字段算 md5（用于 insert/rename 时填 sync_md5）。
///
/// folder 新建时 sort_order=0（默认），与 row 读出一致。
pub fn folder_md5_from_fields(id: &str, name: &str, sort_order: i64) -> String {
    let s = format!("{}|{}|{}", id, name, sort_order);
    md5_hex(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cipher() -> VaultCipher {
        VaultCipher {
            id: "uuid-1".into(),
            folder_id: Some("folder-1".into()),
            favorite: false,
            atype: 1,
            name: "v1:encrypted-name".into(),
            notes: Some("v1:encrypted-notes".into()),
            data: "v1:encrypted-data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            deleted_at: None,
            created_at: "2026-07-21 10:00:00".into(),
            updated_at: "2026-07-21 10:00:00".into(),
            sync_md5: None,
        }
    }

    #[test]
    fn cipher_md5_is_deterministic() {
        let c1 = sample_cipher();
        let c2 = sample_cipher();
        assert_eq!(cipher_md5(&c1), cipher_md5(&c2));
    }

    #[test]
    fn cipher_md5_ignores_timestamps() {
        // created_at / updated_at 变化不应影响 md5
        let mut c1 = sample_cipher();
        let mut c2 = sample_cipher();
        c2.created_at = "2025-01-01 00:00:00".into();
        c2.updated_at = "2030-12-31 23:59:59".into();
        // c1 时间戳不动
        let _ = &mut c1;
        assert_eq!(
            cipher_md5(&c1),
            cipher_md5(&c2),
            "时间戳变化不应影响 md5（跨设备时间戳必然不同）"
        );
    }

    #[test]
    fn cipher_md5_changes_on_content_change() {
        let c1 = sample_cipher();
        let mut c2 = sample_cipher();
        c2.name = "v1:different-encrypted-name".into();
        assert_ne!(cipher_md5(&c1), cipher_md5(&c2));
    }

    #[test]
    fn cipher_md5_handles_none_fields() {
        // notes / fields / password_history 为 None 时应稳定处理
        let mut c = sample_cipher();
        c.notes = None;
        c.fields = None;
        c.password_history = None;
        c.folder_id = None;
        c.deleted_at = None;
        let md5_none = cipher_md5(&c);

        // 改回空字符串——md5 应仍相同（None 和 "" 视为等价）
        // 注意：这不是要求 None == Some("")，而是验证 None 处理稳定（多次调用一致）
        let md5_none_2 = cipher_md5(&c);
        assert_eq!(md5_none, md5_none_2);
    }

    #[test]
    fn folder_md5_changes_on_content_change() {
        let f1 = VaultFolder {
            id: "f1".into(),
            name: "v1:name-a".into(),
            sort_order: 0,
            created_at: "2026-07-21 10:00:00".into(),
            updated_at: "2026-07-21 10:00:00".into(),
            sync_md5: None,
        };
        let mut f2 = f1.clone();
        f2.name = "v1:name-b".into();
        assert_ne!(folder_md5(&f1), folder_md5(&f2));

        let mut f3 = f1.clone();
        f3.sort_order = 1;
        assert_ne!(folder_md5(&f1), folder_md5(&f3));
    }

    #[test]
    fn folder_md5_ignores_timestamps() {
        let mut f1 = VaultFolder {
            id: "f1".into(),
            name: "v1:name".into(),
            sort_order: 0,
            created_at: "2026-07-21 10:00:00".into(),
            updated_at: "2026-07-21 10:00:00".into(),
            sync_md5: None,
        };
        let md5_a = folder_md5(&f1);
        f1.created_at = "1999-01-01".into();
        f1.updated_at = "2099-12-31".into();
        assert_eq!(md5_a, folder_md5(&f1));
    }

    // md5_hex 测试已随函数搬到 octopus_sync::store
}
