//! outline.json 增量索引——uuid → sha256 映射。
//!
//! 同步时客户端先拉 outline.json，对比本地 outline，按 sha 差异决定哪些 cipher
//! 文件需要下载。避免 git fetch 全部历史 + 让客户端能精确控制同步粒度。
//!
//! vault_version 是 monotonic 递增整数，每次本地改动 +1，用于检测「远程版本比
//! 本地旧」（防 push 旧数据覆盖）。
//!
//! **BTreeMap 而非 HashMap**（2026-07-21 修复）：BTreeMap 按 key 字典序迭代，
//! serde 序列化为 JSON object 时 key 顺序稳定——保证相同输入产生字节相同的
//! outline.json，避免 git 误判为变化产生空 commit（用户实测「每次同步都推 4 条」
//! 的根因之一）。HashMap 迭代顺序随机，每次写盘 JSON 字节不同。

use std::collections::BTreeMap;

/// outline 单条条目——uuid 对应的文件 sha + 最后更新时间。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct OutlineEntry {
    /// cipher/folder 文件内容的 sha256（hex），用于增量同步去重。
    pub sha: String,
    /// 最后更新时间（ISO 8601），merge 时取较新者。
    pub updated_at: String,
}

/// outline.json 完整结构。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Outline {
    /// outline 格式版本（当前 1）。
    pub version: u32,
    /// vault 整体版本（monotonic 递增，每次本地改动 +1）。
    pub vault_version: u64,
    /// cipher uuid → 条目。BTreeMap 保证 JSON 序列化顺序稳定。
    pub ciphers: BTreeMap<String, OutlineEntry>,
    /// folder uuid → 条目。BTreeMap 保证 JSON 序列化顺序稳定。
    pub folders: BTreeMap<String, OutlineEntry>,
}

impl Default for Outline {
    fn default() -> Self {
        Self {
            version: 1,
            vault_version: 0,
            ciphers: BTreeMap::new(),
            folders: BTreeMap::new(),
        }
    }
}

/// 合并本地与远程 outline——按 uuid 取最新 updated_at。
///
/// 返回 merged outline。vault_version 取 max。
///
/// 设计：cipher 和 folder 各自独立 merge（key 是 uuid，全局唯一无冲突）。
/// 对于「本地有 + 远程无」的 uuid——保留（远程可能还没 push）。
/// 对于「本地无 + 远程有」的 uuid——加入（远程新增）。
/// 对于「双方都有」——取 updated_at 更新的。
pub fn merge_outlines(local: &Outline, remote: &Outline) -> Outline {
    let mut merged = local.clone();
    // ciphers
    for (uuid, remote_entry) in &remote.ciphers {
        match merged.ciphers.get(uuid) {
            None => {
                merged.ciphers.insert(uuid.clone(), remote_entry.clone());
            }
            Some(local_entry) => {
                if remote_entry.updated_at > local_entry.updated_at {
                    merged.ciphers.insert(uuid.clone(), remote_entry.clone());
                }
            }
        }
    }
    // folders
    for (uuid, remote_entry) in &remote.folders {
        match merged.folders.get(uuid) {
            None => {
                merged.folders.insert(uuid.clone(), remote_entry.clone());
            }
            Some(local_entry) => {
                if remote_entry.updated_at > local_entry.updated_at {
                    merged.folders.insert(uuid.clone(), remote_entry.clone());
                }
            }
        }
    }
    // vault_version 取 max
    merged.vault_version = local.vault_version.max(remote.vault_version);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(sha: &str, ts: &str) -> OutlineEntry {
        OutlineEntry {
            sha: sha.into(),
            updated_at: ts.into(),
        }
    }

    #[test]
    fn merge_both_add_new() {
        // 本地有 c1，远程有 c2 → 合并后两个都有
        let local = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("sha1", "2026-07-21T10:00:00"))]),
            ..Default::default()
        };
        let remote = Outline {
            ciphers: BTreeMap::from([("c2".into(), entry("sha2", "2026-07-21T11:00:00"))]),
            ..Default::default()
        };
        let merged = merge_outlines(&local, &remote);
        assert_eq!(merged.ciphers.len(), 2);
        assert!(merged.ciphers.contains_key("c1"));
        assert!(merged.ciphers.contains_key("c2"));
    }

    #[test]
    fn merge_same_uuid_takes_newer() {
        // 双方都有 c1，远程更新 → 取远程
        let local = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("sha-old", "2026-07-21T10:00:00"))]),
            ..Default::default()
        };
        let remote = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("sha-new", "2026-07-21T11:00:00"))]),
            ..Default::default()
        };
        let merged = merge_outlines(&local, &remote);
        assert_eq!(merged.ciphers["c1"].sha, "sha-new");
    }

    #[test]
    fn merge_same_uuid_keeps_local_if_newer() {
        let local = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("sha-new", "2026-07-21T11:00:00"))]),
            ..Default::default()
        };
        let remote = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("sha-old", "2026-07-21T10:00:00"))]),
            ..Default::default()
        };
        let merged = merge_outlines(&local, &remote);
        assert_eq!(merged.ciphers["c1"].sha, "sha-new");
    }

    #[test]
    fn merge_vault_version_takes_max() {
        let local = Outline {
            vault_version: 5,
            ..Default::default()
        };
        let remote = Outline {
            vault_version: 10,
            ..Default::default()
        };
        let merged = merge_outlines(&local, &remote);
        assert_eq!(merged.vault_version, 10);
    }

    #[test]
    fn merge_folders_independent_from_ciphers() {
        let local = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("sha1", "2026-07-21T10:00:00"))]),
            folders: BTreeMap::new(),
            ..Default::default()
        };
        let remote = Outline {
            ciphers: BTreeMap::new(),
            folders: BTreeMap::from([("f1".into(), entry("sha-f1", "2026-07-21T10:00:00"))]),
            ..Default::default()
        };
        let merged = merge_outlines(&local, &remote);
        assert_eq!(merged.ciphers.len(), 1);
        assert_eq!(merged.folders.len(), 1);
    }

    #[test]
    fn outline_round_trip_json() {
        let outline = Outline {
            version: 1,
            vault_version: 42,
            ciphers: BTreeMap::from([
                ("c1".into(), entry("sha1", "2026-07-21T10:00:00")),
                ("c2".into(), entry("sha2", "2026-07-21T11:00:00")),
            ]),
            folders: BTreeMap::from([("f1".into(), entry("sha-f1", "2026-07-21T09:00:00"))]),
        };
        let json = serde_json::to_string(&outline).unwrap();
        let parsed: Outline = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.vault_version, 42);
        assert_eq!(parsed.ciphers.len(), 2);
        assert_eq!(parsed.folders.len(), 1);
        assert_eq!(parsed.ciphers["c1"].sha, "sha1");
    }

    /// BTreeMap 序列化顺序稳定——同样输入两次序列化结果字节一致。
    /// 回归测试：HashMap 时这个测试会随机失败（顺序不稳定）。
    #[test]
    fn outline_serialization_is_deterministic() {
        let outline = Outline {
            version: 1,
            vault_version: 1,
            ciphers: BTreeMap::from([
                ("zzz".into(), entry("sha-z", "2026-07-21T10:00:00")),
                ("aaa".into(), entry("sha-a", "2026-07-21T10:00:00")),
                ("mmm".into(), entry("sha-m", "2026-07-21T10:00:00")),
            ]),
            folders: BTreeMap::new(),
        };
        let json1 = serde_json::to_string(&outline).unwrap();
        let json2 = serde_json::to_string(&outline).unwrap();
        assert_eq!(json1, json2, "同输入序列化应字节一致");

        // BTreeMap 按 key 字典序——aaa 应排在 zzz 前
        let aaa_pos = json1.find("\"aaa\"").unwrap();
        let zzz_pos = json1.find("\"zzz\"").unwrap();
        assert!(
            aaa_pos < zzz_pos,
            "BTreeMap 应按字典序排列：aaa 应在 zzz 前"
        );
    }
}
