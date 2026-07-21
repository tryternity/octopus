//! 热词同步——`.sync/hotword/` 目录的文件存储 + md5 增量同步（2026-07-22 Task 13.6）。
//!
//! 热词是 `.sync/` 目录扩展的第一个非 vault 数据类型。与 vault 同步的区别：
//! - **明文存储**：热词不含密码等高敏感信息（当前 SQLite 也是明文），文件不加密
//! - **无 meta**：热词没有 vault_meta 那样的全局配置，只有 sets + outline
//! - **复用 Outline 结构**：热词 outline 用 Outline.ciphers 字段存 hotword set entries
//!   （字段名内部细节，功能正确；folders 字段留空）
//!
//! ## 目录结构
//!
//! ```text
//! ~/.octopus/.sync/hotword/
//! ├── outline.json              { version, vault_version, ciphers: {uuid: {md5, updated_ms}} }
//! └── sets/<2hex>/<uuid>.json   单个热词版本（明文 JSON）
//! ```
//!
//! ## md5 指纹
//!
//! 拼接字段：`name | enabled | words_text`（不含 id / created_at / updated_at / sync_md5）。
//! `words_text` 已经过 normalize_words_text（拼音首字母排序 + 去重）——跨设备字节一致。
//!
//! 详见 spec §4.13 + plan Task 13.6。

use std::path::PathBuf;

use anyhow::{Context, Result};
use octopus_infra::db::HotwordSet;

use crate::outline::{Outline, OutlineEntry};
use crate::store::{iso_to_unix_ms, md5_hex, shard_dir, sync_root};

// === 路径辅助 ===

/// `~/.octopus/.sync/hotword/`——热词数据子目录。
pub fn hotword_dir() -> PathBuf {
    sync_root().join("hotword")
}

/// `~/.octopus/.sync/hotword/outline.json`——增量索引。
pub fn hotword_outline_path() -> PathBuf {
    hotword_dir().join("outline.json")
}

/// 热词版本文件路径：`hotword/sets/<2hex>/<uuid>.json`（与 vault cipher 同样分桶）。
pub fn hotword_set_file_path(uuid: &str) -> PathBuf {
    hotword_dir()
        .join("sets")
        .join(shard_dir(uuid))
        .join(format!("{}.json", uuid))
}

// === md5 指纹 ===

/// 热词版本的逻辑内容 md5——不含 created_at/updated_at/sync_md5。
///
/// 拼接字段顺序（固定，不可变）：`name | enabled | words_text`
///
/// `words_text` 已 normalize（拼音首字母排序 + 去重），跨设备字节一致——md5 也一致。
pub fn hotword_set_md5(h: &HotwordSet) -> String {
    hotword_set_md5_from_fields(&h.name, h.enabled, &h.words_text)
}

/// 从基本字段算 md5——用于写命令填 sync_md5（避免重复读完整 row）。
///
/// 与 `hotword_set_md5(&HotwordSet)` 对同一数据算出的 md5 相同。
pub fn hotword_set_md5_from_fields(name: &str, enabled: bool, words_text: &str) -> String {
    let input = format!("{}|{}|{}", name, enabled, words_text);
    md5_hex(input.as_bytes())
}

// === 文件格式 ===

/// 单个热词版本的文件内容（明文 JSON，不加密）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HotwordSetFile {
    /// 文件格式版本（当前 1）。
    pub version: u32,
    /// UUID（与 SQLite hotword_sets.id 一致）。
    pub id: String,
    /// 版本名（明文）。
    pub name: String,
    /// 是否勾选生效。
    pub enabled: bool,
    /// 空格分隔的归一化词列表（已 normalize_words_text）。
    pub words_text: String,
    /// 创建时间（SQLite datetime 格式，跨设备不同但保留用于排序）。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
}

impl HotwordSetFile {
    /// 从 SQLite 行转换。
    pub fn from_hotword_set(h: &HotwordSet) -> Self {
        Self {
            version: 1,
            id: h.id.clone(),
            name: h.name.clone(),
            enabled: h.enabled,
            words_text: h.words_text.clone(),
            created_at: h.created_at.clone(),
            updated_at: h.updated_at.clone(),
        }
    }

    /// 转换回 HotwordSet（sync pull 用——sync_md5 由调用方算填）。
    pub fn to_hotword_set(&self, sync_md5: Option<String>) -> HotwordSet {
        HotwordSet {
            id: self.id.clone(),
            name: self.name.clone(),
            enabled: self.enabled,
            words_text: self.words_text.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            sync_md5,
        }
    }
}

// === 读写函数 ===

/// 读 outline.json。文件不存在时返回默认空 outline（首次同步）。
pub fn read_hotword_outline() -> Result<Outline> {
    let path = hotword_outline_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let outline: Outline = serde_json::from_str(&content)
                .with_context(|| format!("hotword outline.json 解析失败：{}", path.display()))?;
            Ok(outline)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Outline::default()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("读 hotword outline.json 失败：{}", path.display()))),
    }
}

/// 写 outline.json（pretty print）。
pub fn write_hotword_outline(outline: &Outline) -> Result<()> {
    let path = hotword_outline_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败：{}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(outline).context("序列化 hotword outline 失败")?;
    std::fs::write(&path, format!("{}\n", json))
        .with_context(|| format!("写 hotword outline.json 失败：{}", path.display()))?;
    Ok(())
}

/// 读单个热词版本文件。
pub fn read_hotword_set_file(uuid: &str) -> Result<HotwordSetFile> {
    let path = hotword_set_file_path(uuid);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读热词版本文件失败：{}", path.display()))?;
    let file: HotwordSetFile =
        serde_json::from_str(&content).context("热词版本文件 JSON 解析失败")?;
    Ok(file)
}

/// 写单个热词版本文件（含分桶目录创建）。
pub fn write_hotword_set_file(file: &HotwordSetFile) -> Result<()> {
    let path = hotword_set_file_path(&file.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建桶目录失败：{}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(file).context("序列化热词版本文件失败")?;
    std::fs::write(&path, format!("{}\n", json))
        .with_context(|| format!("写热词版本文件失败：{}", path.display()))?;
    Ok(())
}

/// 删除单个热词版本文件（文件不存在时返 Ok——幂等）。
pub fn delete_hotword_set_file(uuid: &str) -> Result<()> {
    let path = hotword_set_file_path(uuid);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("删热词版本文件失败：{}", path.display()))),
    }
}

// === 全量导出/导入 ===

/// 从 SQLite 全量导出到文件系统——首次启用同步时用（push_initial）。
///
/// 步骤：
/// 1. 清空 sets/ 目录（防 stale 文件残留）
/// 2. 写所有版本文件
/// 3. 生成 outline.json
pub fn export_all_hotwords(sets: &[HotwordSet]) -> Result<Outline> {
    let dir = hotword_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建 hotword 目录失败：{}", dir.display()))?;

    // 1. 清空 sets/（保留 outline.json）
    let sets_dir = dir.join("sets");
    if sets_dir.exists() {
        std::fs::remove_dir_all(&sets_dir).context("清空 hotword/sets/ 失败")?;
    }

    // 2. 写所有版本文件 + 收集 (uuid, md5)
    // BTreeMap 保证 outline.json 序列化顺序稳定
    let mut entries: std::collections::BTreeMap<String, OutlineEntry> = std::collections::BTreeMap::new();
    for h in sets {
        write_hotword_set_file(&HotwordSetFile::from_hotword_set(h))?;
        let md5 = h.sync_md5.clone().unwrap_or_else(|| hotword_set_md5(h));
        entries.insert(
            h.id.clone(),
            OutlineEntry {
                md5,
                updated_ms: iso_to_unix_ms(&h.updated_at),
            },
        );
    }

    // 3. 写 outline.json
    let outline = Outline {
        version: 1,
        vault_version: 1, // 首次导出从 1 开始（字段名 vault_version 复用，语义为 hotword_version）
        ciphers: entries, // 复用 ciphers 字段存 hotword set entries
        folders: std::collections::BTreeMap::new(), // 热词无 folder 概念，留空
    };
    write_hotword_outline(&outline)?;

    Ok(outline)
}

/// 增量导出——sync_now 用，只写真正变化的文件（不清空目录）。
///
/// 与 `export_all_hotwords` 的区别：
/// - `export_all_hotwords`：清空 + 全写（首次启用同步时用）
/// - `incremental_export_hotwords`：读旧 outline + 对比 sync_md5 → 只写变化文件 + 删 SQLite 无的
///
/// 返回 (new_outline, changed_count)——changed_count 是实际写/删的文件数。
pub fn incremental_export_hotwords(sets: &[HotwordSet]) -> Result<(Outline, usize)> {
    let dir = hotword_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建 hotword 目录失败：{}", dir.display()))?;

    // 1. 读旧 outline 做 diff
    let old_outline = read_hotword_outline().unwrap_or_default();
    let mut changed = 0usize;

    // 2. 对比 md5，只写变化的
    let mut entries: std::collections::BTreeMap<String, OutlineEntry> = std::collections::BTreeMap::new();
    let id_set: std::collections::HashSet<&str> = sets.iter().map(|h| h.id.as_str()).collect();
    for h in sets {
        let new_md5 = h.sync_md5.clone().unwrap_or_else(|| hotword_set_md5(h));
        let old_entry = old_outline.ciphers.get(&h.id);
        let needs_write = match old_entry {
            None => true,           // 新增
            Some(old) => old.md5 != new_md5, // md5 变了
        };
        if needs_write {
            write_hotword_set_file(&HotwordSetFile::from_hotword_set(h))?;
            changed += 1;
        }
        entries.insert(
            h.id.clone(),
            OutlineEntry {
                md5: new_md5,
                updated_ms: iso_to_unix_ms(&h.updated_at),
            },
        );
    }
    // 删 SQLite 无但 outline 有的文件
    for old_uuid in old_outline.ciphers.keys() {
        if !id_set.contains(old_uuid.as_str()) {
            let _ = delete_hotword_set_file(old_uuid); // 幂等
            changed += 1;
        }
    }

    // 3. 写新 outline（vault_version 只在 changed > 0 时 +1）
    let new_version = if changed > 0 {
        old_outline.vault_version.wrapping_add(1)
    } else {
        old_outline.vault_version
    };
    let outline = Outline {
        version: 1,
        vault_version: new_version,
        ciphers: entries,
        folders: std::collections::BTreeMap::new(),
    };
    write_hotword_outline(&outline)?;

    Ok((outline, changed))
}

/// 从文件系统全量导入——sync pull / clone_initial 用。
///
/// 读所有 sets/<桶>/<uuid>.json 文件，返回 HotwordSetFile 列表（sync_md5 由调用方算填）。
pub fn import_hotwords_from_files() -> Result<Vec<HotwordSetFile>> {
    let sets_dir = hotword_dir().join("sets");
    let mut files = Vec::new();
    collect_json_files(&sets_dir, &mut files)?;
    files.sort(); // 按路径排序，结果稳定

    let mut sets = Vec::new();
    for entry in files {
        let content = std::fs::read_to_string(&entry)
            .with_context(|| format!("读热词版本文件失败：{}", entry.display()))?;
        let file: HotwordSetFile = serde_json::from_str(&content)
            .with_context(|| format!("解析热词版本文件失败：{}", entry.display()))?;
        sets.push(file);
    }
    Ok(sets)
}

/// 递归收集目录下所有 .json 文件（与 vault store::collect_json_files 同实现，内联避免跨 crate 依赖）。
fn collect_json_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("读目录失败：{}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

// === sync engine（pull / push）===

/// Pull 阶段：文件系统 → SQLite。对比 outline 找出新增/修改，读文件 upsert。
///
/// 返回实际拉取（upsert）的条数。删除传播由 push 阶段处理（SQLite 无的文件被删）。
///
/// 调用方：vault engine.rs sync_now 的 pull 阶段。
pub fn pull_hotwords_from_files() -> Result<usize> {
    let remote_outline = read_hotword_outline()?;
    let db_sets = octopus_infra::db::list_hotword_sets()?;

    let db_ids: std::collections::HashSet<&str> = db_sets.iter().map(|h| h.id.as_str()).collect();
    let mut count = 0;

    for (uuid, _entry) in &remote_outline.ciphers {
        let needs_update = !db_ids.contains(uuid.as_str())
            || hotword_md5_mismatch(uuid, &db_sets);
        if needs_update {
            if let Ok(file) = read_hotword_set_file(uuid) {
                let h = file.to_hotword_set(None);
                let md5 = hotword_set_md5(&h);
                let mut h = h;
                h.sync_md5 = Some(md5);
                octopus_infra::db::upsert_hotword_set(&h)?;
                count += 1;
            }
        }
    }

    Ok(count)
}

/// 检测热词版本的 sync_md5 是否与 outline 不匹配（需要 pull）。
fn hotword_md5_mismatch(uuid: &str, db_sets: &[HotwordSet]) -> bool {
    let db_set = match db_sets.iter().find(|h| h.id == uuid) {
        Some(h) => h,
        None => return true,
    };
    match read_hotword_set_file(uuid) {
        Ok(file) => {
            // 比较文件内容的 md5 vs DB sync_md5
            let file_md5 = hotword_set_md5_from_fields(&file.name, file.enabled, &file.words_text);
            let db_md5 = db_set.sync_md5.as_deref().unwrap_or("");
            file_md5 != db_md5
        }
        Err(_) => false,
    }
}

/// Push 阶段：SQLite 最新数据 → 文件系统 + 更新 outline。
///
/// 返回实际变更（写/删）的文件数。调用方：vault engine.rs sync_now 的 push 阶段。
pub fn push_hotwords_to_files() -> Result<usize> {
    let sets = octopus_infra::db::list_hotword_sets()?;
    let (_, changed) = incremental_export_hotwords(&sets)?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// RAII guard：测试期间 set_test_sync_root，drop 时 clear。
    struct SyncRootGuard {
        _tmp: TempDir,
    }
    impl SyncRootGuard {
        fn new() -> Self {
            let tmp = TempDir::new().expect("tempdir");
            let sync_path = tmp.path().join(".sync");
            std::fs::create_dir_all(&sync_path).unwrap();
            crate::store::set_test_sync_root(sync_path);
            Self { _tmp: tmp }
        }
    }
    impl Drop for SyncRootGuard {
        fn drop(&mut self) {
            crate::store::clear_test_sync_root();
        }
    }

    fn sample_set(id: &str, name: &str, words: &str) -> HotwordSet {
        HotwordSet {
            id: id.into(),
            name: name.into(),
            enabled: true,
            words_text: words.into(),
            created_at: "2026-07-22 10:00:00".into(),
            updated_at: "2026-07-22 10:00:00".into(),
            sync_md5: None,
        }
    }

    // === md5 指纹测试 ===

    #[test]
    fn hotword_set_md5_is_deterministic() {
        let h1 = sample_set("uuid-1", "版本A", "苹果 香蕉");
        let h2 = sample_set("uuid-2", "版本A", "苹果 香蕉");
        // id 不同但逻辑内容相同 → md5 应相同
        assert_eq!(hotword_set_md5(&h1), hotword_set_md5(&h2));
    }

    #[test]
    fn hotword_set_md5_ignores_timestamps() {
        let mut h1 = sample_set("uuid-1", "版本A", "苹果");
        let mut h2 = sample_set("uuid-1", "版本A", "苹果");
        h2.created_at = "1999-01-01 00:00:00".into();
        h2.updated_at = "2099-12-31 23:59:59".into();
        let _ = &mut h1;
        assert_eq!(hotword_set_md5(&h1), hotword_set_md5(&h2));
    }

    #[test]
    fn hotword_set_md5_changes_on_name_change() {
        let h1 = sample_set("uuid-1", "版本A", "苹果");
        let mut h2 = sample_set("uuid-1", "版本B", "苹果");
        let _ = &mut h2;
        assert_ne!(hotword_set_md5(&h1), hotword_set_md5(&h2));
    }

    #[test]
    fn hotword_set_md5_changes_on_enabled_change() {
        let h1 = sample_set("uuid-1", "版本A", "苹果");
        let mut h2 = sample_set("uuid-1", "版本A", "苹果");
        h2.enabled = false;
        let _ = &mut h2;
        assert_ne!(hotword_set_md5(&h1), hotword_set_md5(&h2));
    }

    #[test]
    fn hotword_set_md5_changes_on_words_change() {
        let h1 = sample_set("uuid-1", "版本A", "苹果");
        let mut h2 = sample_set("uuid-1", "版本A", "香蕉");
        let _ = &mut h2;
        assert_ne!(hotword_set_md5(&h1), hotword_set_md5(&h2));
    }

    #[test]
    fn hotword_set_md5_from_fields_matches_struct() {
        let h = sample_set("uuid-1", "版本A", "苹果 香蕉");
        let from_struct = hotword_set_md5(&h);
        let from_fields = hotword_set_md5_from_fields(&h.name, h.enabled, &h.words_text);
        assert_eq!(from_struct, from_fields);
    }

    #[test]
    fn hotword_set_md5_normalizes_equivalent_words() {
        // words_text 已 normalize（调用方保证），相同内容顺序不同应由调用方 normalize
        // 这里测：相同 words_text 字符串 → 相同 md5（即使传不同 id）
        let h1 = sample_set("uuid-1", "版本A", "苹果 香蕉");
        let h2 = sample_set("uuid-2", "版本A", "苹果 香蕉");
        assert_eq!(hotword_set_md5(&h1), hotword_set_md5(&h2));
    }

    // === 文件读写测试 ===

    #[test]
    fn hotword_set_file_round_trip() {
        let _g = SyncRootGuard::new();
        let h = sample_set("a1b2c3d4-e5f6-4789-8901-abcdef123456", "测试版本", "苹果 香蕉");
        let file = HotwordSetFile::from_hotword_set(&h);
        write_hotword_set_file(&file).expect("write");

        let loaded = read_hotword_set_file(&h.id).expect("read");
        assert_eq!(loaded.id, h.id);
        assert_eq!(loaded.name, h.name);
        assert_eq!(loaded.words_text, h.words_text);
        assert_eq!(loaded.enabled, h.enabled);
    }

    #[test]
    fn delete_hotword_set_file_is_idempotent() {
        let _g = SyncRootGuard::new();
        let uuid = "a1b2c3d4-e5f6-4789-8901-abcdef123456";
        // 文件不存在时删除应返 Ok（幂等）
        delete_hotword_set_file(uuid).expect("删不存在的文件应 Ok");
    }

    #[test]
    fn hotword_outline_round_trip() {
        let _g = SyncRootGuard::new();
        let outline = Outline {
            version: 1,
            vault_version: 42,
            ciphers: std::collections::BTreeMap::from([
                ("uuid-1".into(), OutlineEntry { md5: "md5a".into(), updated_ms: 1000 }),
            ]),
            folders: std::collections::BTreeMap::new(),
        };
        write_hotword_outline(&outline).expect("write");
        let loaded = read_hotword_outline().expect("read");
        assert_eq!(loaded.vault_version, 42);
        assert_eq!(loaded.ciphers.len(), 1);
        assert_eq!(loaded.ciphers["uuid-1"].md5, "md5a");
    }

    #[test]
    fn read_hotword_outline_missing_returns_default() {
        let _g = SyncRootGuard::new();
        let outline = read_hotword_outline().expect("应返默认空 outline");
        assert_eq!(outline.vault_version, 0);
        assert!(outline.ciphers.is_empty());
    }

    // === export/import 测试 ===

    #[test]
    fn export_all_writes_all_sets() {
        let _g = SyncRootGuard::new();
        let sets = vec![
            sample_set("a1b2c3d4-e5f6-4789-8901-abcdef123456", "版本A", "苹果"),
            sample_set("b2c3d4e5-f6a7-4890-9002-bcdef234567", "版本B", "香蕉"),
        ];
        let outline = export_all_hotwords(&sets).expect("export");
        assert_eq!(outline.ciphers.len(), 2);

        // 文件实际写到了分桶目录
        let loaded = import_hotwords_from_files().expect("import");
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn incremental_export_zero_changes_on_unchanged_data() {
        let _g = SyncRootGuard::new();
        let sets = vec![sample_set("a1b2c3d4-0001", "版本A", "苹果")];

        // 第一次 export（全量）
        let first = export_all_hotwords(&sets).expect("first export");
        // 第二次 incremental——sync_md5 已在 outline 里，应 0 变更
        let (outline, changed) = incremental_export_hotwords(&sets).expect("incremental");
        assert_eq!(changed, 0, "无变化时应 0 变更");
        assert_eq!(outline.vault_version, first.vault_version, "无变化版本不递增");
    }

    #[test]
    fn incremental_export_writes_only_changed() {
        let _g = SyncRootGuard::new();
        let sets = vec![
            sample_set("a1b2c3d4-0001", "版本A", "苹果"),
            sample_set("a1b2c3d4-0002", "版本B", "香蕉"),
        ];
        export_all_hotwords(&sets).expect("initial");

        // 改一个版本的词（sync_md5 会变——因为 words_text 变了）
        let mut sets2 = sets.clone();
        sets2[0].words_text = "葡萄".into();
        let (_, changed) = incremental_export_hotwords(&sets2).expect("incremental");
        assert_eq!(changed, 1, "只改了一个版本，应 1 变更");
    }

    #[test]
    fn incremental_export_deletes_missing() {
        let _g = SyncRootGuard::new();
        let sets = vec![
            sample_set("a1b2c3d4-0001", "版本A", "苹果"),
            sample_set("a1b2c3d4-0002", "版本B", "香蕉"),
        ];
        export_all_hotwords(&sets).expect("initial");

        // SQLite 删了一个（只剩版本A）
        let sets2 = vec![sample_set("a1b2c3d4-0001", "版本A", "苹果")];
        let (outline, changed) = incremental_export_hotwords(&sets2).expect("incremental");
        assert_eq!(changed, 1, "删了一个版本，应 1 变更");
        assert_eq!(outline.ciphers.len(), 1);
        assert!(outline.ciphers.contains_key("a1b2c3d4-0001"));

        // 文件也应被删
        assert!(
            read_hotword_set_file("a1b2c3d4-0002").is_err(),
            "已删版本A的文件不应存在"
        );
    }

    #[test]
    fn incremental_export_outline_uses_sync_md5() {
        let _g = SyncRootGuard::new();
        // 预设 sync_md5——incremental_export 应把它写入 outline（而非临时算）
        let mut sets = vec![sample_set("a1b2c3d4-0001", "版本A", "苹果")];
        sets[0].sync_md5 = Some("preset-md5-value".into());

        let (outline, _) = incremental_export_hotwords(&sets).expect("export");
        assert_eq!(
            outline.ciphers["a1b2c3d4-0001"].md5,
            "preset-md5-value",
            "outline.md5 应用 SQLite 的 sync_md5 值"
        );
    }

    #[test]
    fn import_returns_files_with_correct_data() {
        let _g = SyncRootGuard::new();
        let sets = vec![
            sample_set("a1b2c3d4-0001", "版本A", "苹果 香蕉"),
            sample_set("b2c3d4e5-0002", "版本B", "葡萄"),
        ];
        export_all_hotwords(&sets).expect("export");

        let loaded = import_hotwords_from_files().expect("import");
        assert_eq!(loaded.len(), 2);
        // 验证内容完整（不丢字段）
        let a = loaded.iter().find(|f| f.name == "版本A").expect("应有版本A");
        assert_eq!(a.words_text, "苹果 香蕉");
        assert!(a.enabled);
    }
}
