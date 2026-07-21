//! outline.json 增量索引——uuid → md5 映射。
//!
//! 同步时客户端先拉 outline.json，对比本地 outline，按 md5 差异决定哪些 cipher
//! 文件需要下载。避免 git fetch 全部历史 + 让客户端能精确控制同步粒度。
//!
//! vault_version 是 monotonic 递增整数，**有变化时**才 +1（无变化 sync 不递增），
//! 用于检测「远程版本比本地旧」（防 push 旧数据覆盖）。
//!
//! **BTreeMap 而非 HashMap**（2026-07-21 修复）：BTreeMap 按 key 字典序迭代，
//! serde 序列化为 JSON object 时 key 顺序稳定——保证相同输入产生字节相同的
//! outline.json，避免 git 误判为变化产生空 commit（用户实测「每次同步都推 4 条」
//! 的根因之一）。HashMap 迭代顺序随机，每次写盘 JSON 字节不同。
//!
//! **字段命名**（2026-07-22 修订，不做旧文件兼容——用户已同意删 .vault 重建）：
//! - `md5`：逻辑内容指纹（32 字符 hex），跨设备一致
//! - `updated_ms`：Unix 毫秒时间戳（i64），数值比较可靠（旧版 ISO 字符串无法准确比较）

use std::collections::BTreeMap;

/// outline 单条条目——uuid 对应的 md5 + 最后更新时间（毫秒）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct OutlineEntry {
    /// cipher/folder 逻辑内容的 md5（hex 32 字符），用于增量同步去重。
    pub md5: String,
    /// 最后更新时间——Unix 毫秒时间戳（i64），merge 时取较新者。
    pub updated_ms: i64,
}

/// outline.json 完整结构。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Outline {
    /// outline 格式版本（当前 1）。
    pub version: u32,
    /// vault 整体版本（monotonic 递增，**有变化时** +1）。
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

/// 合并本地与远程 outline——按 uuid 取 updated_ms 更新者。
///
/// 返回 merged outline。vault_version 取 max。
///
/// 设计：cipher 和 folder 各自独立 merge（key 是 uuid，全局唯一无冲突）。
/// 对于「本地有 + 远程无」的 uuid——保留（远程可能还没 push）。
/// 对于「本地无 + 远程有」的 uuid——加入（远程新增）。
/// 对于「双方都有」——取 updated_ms 更大的（毫秒时间戳数值比较）。
pub fn merge_outlines(local: &Outline, remote: &Outline) -> Outline {
    let mut merged = local.clone();
    // ciphers
    for (uuid, remote_entry) in &remote.ciphers {
        match merged.ciphers.get(uuid) {
            None => {
                merged.ciphers.insert(uuid.clone(), remote_entry.clone());
            }
            Some(local_entry) => {
                if remote_entry.updated_ms > local_entry.updated_ms {
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
                if remote_entry.updated_ms > local_entry.updated_ms {
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

    fn entry(md5: &str, updated_ms: i64) -> OutlineEntry {
        OutlineEntry {
            md5: md5.into(),
            updated_ms,
        }
    }

    #[test]
    fn merge_both_add_new() {
        // 本地有 c1，远程有 c2 → 合并后两个都有
        let local = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("md5_1", 1000))]),
            ..Default::default()
        };
        let remote = Outline {
            ciphers: BTreeMap::from([("c2".into(), entry("md5_2", 2000))]),
            ..Default::default()
        };
        let merged = merge_outlines(&local, &remote);
        assert_eq!(merged.ciphers.len(), 2);
        assert!(merged.ciphers.contains_key("c1"));
        assert!(merged.ciphers.contains_key("c2"));
    }

    #[test]
    fn merge_same_uuid_takes_newer() {
        // 双方都有 c1，远程 updated_ms 更大 → 取远程
        let local = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("md5-old", 1000))]),
            ..Default::default()
        };
        let remote = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("md5-new", 2000))]),
            ..Default::default()
        };
        let merged = merge_outlines(&local, &remote);
        assert_eq!(merged.ciphers["c1"].md5, "md5-new");
    }

    #[test]
    fn merge_same_uuid_keeps_local_if_newer() {
        let local = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("md5-new", 2000))]),
            ..Default::default()
        };
        let remote = Outline {
            ciphers: BTreeMap::from([("c1".into(), entry("md5-old", 1000))]),
            ..Default::default()
        };
        let merged = merge_outlines(&local, &remote);
        assert_eq!(merged.ciphers["c1"].md5, "md5-new");
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
            ciphers: BTreeMap::from([("c1".into(), entry("md5_1", 1000))]),
            folders: BTreeMap::new(),
            ..Default::default()
        };
        let remote = Outline {
            ciphers: BTreeMap::new(),
            folders: BTreeMap::from([("f1".into(), entry("md5-f1", 1000))]),
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
                ("c1".into(), entry("md5_1", 1000)),
                ("c2".into(), entry("md5_2", 2000)),
            ]),
            folders: BTreeMap::from([("f1".into(), entry("md5-f1", 1500))]),
        };
        let json = serde_json::to_string(&outline).unwrap();
        let parsed: Outline = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.vault_version, 42);
        assert_eq!(parsed.ciphers.len(), 2);
        assert_eq!(parsed.folders.len(), 1);
        assert_eq!(parsed.ciphers["c1"].md5, "md5_1");
        assert_eq!(parsed.ciphers["c1"].updated_ms, 1000);
    }

    /// BTreeMap 序列化顺序稳定——同样输入两次序列化结果字节一致。
    /// 回归测试：HashMap 时这个测试会随机失败（顺序不稳定）。
    #[test]
    fn outline_serialization_is_deterministic() {
        let outline = Outline {
            version: 1,
            vault_version: 1,
            ciphers: BTreeMap::from([
                ("zzz".into(), entry("md5-z", 1000)),
                ("aaa".into(), entry("md5-a", 1000)),
                ("mmm".into(), entry("md5-m", 1000)),
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
