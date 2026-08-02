//! 热词同步——`.sync/hotword/` 两级 outline 层级 + md5 增量同步（2026-08-01 word 级 merge）。
//!
//! 热词是 `.sync/` 目录扩展的第一个非 vault 数据类型。与 vault 同步的区别：
//! - **明文存储**：热词不含密码等高敏感信息（当前 SQLite 也是明文），文件不加密
//! - **无 meta**：热词没有 vault_meta 那样的全局配置，只有 outline + set meta + word files
//! - **两级 outline**：总 outline 只管词典，词典内 outline 只管词（2026-08-01 重构，
//!   脱离复用 vault `Outline`——`ciphers`/`folders` 字段对热词语义错误）
//!
//! ## 目录结构（两级）
//!
//! ```text
//! ~/.octopus/.sync/hotword/
//! ├── outline.json                 ← 总 outline：只描述词典状态
//! │     { version, hotwordVersion, sets: { <setUuid>: {md5, updatedMs} } }
//! └── <set-uuid>/                  ← 每个词典一个目录（目录名 = 词典 ID）
//!     ├── meta.json                ← 词典元数据（name/enabled/createdAt/updatedAt）
//!     ├── outline.json             ← 本词典的词状态
//!     │     { words: { <wordUuid>: {md5, updatedMs} } }
//!     └── <2hex>/<word-uuid>.json  ← 词文件（按词 UUID 前2位分桶）
//! ```
//!
//! **为什么两级而非扁平**：① 3 万词条拆成 N 个 3 千项的词典 outline，git diff 只碰改动词典；
//! ② 删词典 = `rm -r <set-id>/` 原子完整；③ 语义干净——总 outline 管词典，词状态归属各自词典。
//! **词文件名用 UUID**（=v5(set_id,word)，软删/改拼音不变），内容 MD5 做 outline 变化指纹。
//!
//! ## md5 指纹
//!
//! - **set md5**：`name | enabled`（纯元数据；词变更不再改 set md5——word 有自己的 sync_md5）
//! - **word md5**（长度前缀防 `|` 碰撞，实现在 infra `hotword_word_md5_from_fields`）：
//!   `{set_id_len}|{set_id}|{word_len}|{word}|{pinyin_len}|{pinyin}|{is_deleted}`
//!
//! 详见 spec `2026-08-01-hotword-word-record-design.md` §3 + plan `2026-08-01-hotword-word-level-merge.md`。

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use octopus_infra::db::{self, HotwordSet, HotwordWord};
use octopus_infra::hotword_text::hotword_word_md5_from_fields;

use crate::outline::OutlineEntry;
use crate::store::{iso_to_unix_ms, md5_hex, shard_dir, sync_root, write_atomically};

// === 路径辅助 ===

/// `~/.octopus/.sync/hotword/`——热词数据子目录。
pub fn hotword_dir() -> PathBuf {
    sync_root().join("hotword")
}

/// `~/.octopus/.sync/hotword/outline.json`——增量索引（总，只管词典）。
pub fn hotword_outline_path() -> PathBuf {
    hotword_dir().join("outline.json")
}

/// path traversal 校验——拒绝含 `/` `\` `..` `\0` 或空的 uuid/id（对齐 vault `validate_uuid`）。
///
/// set 目录名（= set_id）与 word 文件名（= word_uuid）都过此校验——远程 outline 的恶意
/// id（如 `../../meta`）会在路径构造入口被拦截，read/write/delete 三路径统一防护。
/// 不强制严格 UUID 格式（测试用简短 id 方便），只拒绝 path traversal 字符。
fn validate_hotword_uuid(uuid: &str) -> Result<()> {
    if uuid.is_empty()
        || uuid.contains('/')
        || uuid.contains('\\')
        || uuid.contains("..")
        || uuid.contains('\0')
    {
        anyhow::bail!(
            "非法 uuid（含路径分隔符 / \\ .. 或空）：{}——拒绝 path traversal",
            uuid
        );
    }
    Ok(())
}

/// 词典目录：`hotword/<set-uuid>/`（每个词典一个目录）。
pub fn hotword_set_dir(set_id: &str) -> Result<PathBuf> {
    validate_hotword_uuid(set_id)?;
    Ok(hotword_dir().join(set_id))
}

/// 词典元数据文件：`hotword/<set-uuid>/meta.json`。
pub fn hotword_meta_file_path(set_id: &str) -> Result<PathBuf> {
    Ok(hotword_set_dir(set_id)?.join("meta.json"))
}

/// 词典内 outline：`hotword/<set-uuid>/outline.json`（只管该词典的词）。
pub fn hotword_set_outline_path(set_id: &str) -> Result<PathBuf> {
    Ok(hotword_set_dir(set_id)?.join("outline.json"))
}

/// 词文件路径：`hotword/<set-uuid>/<2hex>/<word-uuid>.json`（复用 `shard_dir`，按 word_uuid 分桶）。
pub fn hotword_word_file_path(set_id: &str, word_uuid: &str) -> Result<PathBuf> {
    validate_hotword_uuid(set_id)?;
    validate_hotword_uuid(word_uuid)?;
    Ok(hotword_set_dir(set_id)?
        .join(shard_dir(word_uuid))
        .join(format!("{}.json", word_uuid)))
}

// === md5 指纹 ===

/// 词典元数据的身份 md5——只含 id + name（身份标识），不含 enabled/is_deleted 等状态。
///
/// md5 是"这个 set 是谁"的指纹，用于 outline diff 判断 set 是否新增/改名。
/// 不含状态字段（is_deleted/enabled/updated_at）——状态变化靠时间戳比较决定方向，
/// 不应触发 md5 diff（否则删除/启用切换后 md5 变了但 outline 没同步更新 → 不必要的 push/pull）。
pub fn hotword_set_md5(h: &HotwordSet) -> String {
    hotword_set_md5_from_fields(&h.id, &h.name)
}

/// 从基本字段算 md5——用于写命令填 sync_md5（避免重复读完整 row）。
pub fn hotword_set_md5_from_fields(id: &str, name: &str) -> String {
    let input = format!("{}|{}", id, name);
    md5_hex(input.as_bytes())
}

/// 词记录的身份 md5——委托 infra `hotword_word_md5_from_fields`（只含 set_id+word）。
pub fn hotword_word_md5(w: &HotwordWord) -> String {
    hotword_word_md5_from_fields(&w.set_id, &w.word)
}

/// tombstone 是否超期（GC 2026-08-02）——`now_secs - is_deleted > RETENTION`。
/// 活跃（is_deleted=0）永远不超期。用于 export 跳过 + merge pull skip，防跨设备复活。
fn is_tombstone_expired(is_deleted: i64, now_secs: i64) -> bool {
    is_deleted > 0 && now_secs - is_deleted > octopus_infra::db::HOTWORD_TOMBSTONE_RETENTION_SECS
}

// === 文件格式 ===

/// 词典元数据文件内容（明文 JSON）。v57 起只含元数据（词数据在 words/ 目录）。
/// v58 起 version 2——加 is_deleted（tombstone 传播）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotwordSetMeta {
    /// 文件格式版本（当前 2）。version 1 文件（无 is_deleted）反序列化 is_deleted=0（serde default）。
    pub version: u32,
    /// UUID（与 SQLite hotword_sets.id 一致）。
    pub id: String,
    /// 版本名（明文）。
    pub name: String,
    /// 是否勾选生效。
    pub enabled: bool,
    /// 软删标记：0=活跃，>0=删除时刻 epoch 秒（tombstone）。serde default 兼容 version 1 文件。
    #[serde(default)]
    pub is_deleted: i64,
    /// 创建时间（SQLite datetime 格式，跨设备不同但保留用于排序）。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
}

impl HotwordSetMeta {
    /// 从 SQLite 行转换。
    pub fn from_hotword_set(h: &HotwordSet) -> Self {
        Self {
            version: 2,
            id: h.id.clone(),
            name: h.name.clone(),
            enabled: h.enabled,
            is_deleted: h.is_deleted,
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
            is_deleted: self.is_deleted,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            sync_md5,
        }
    }
}

/// 单个词记录的文件内容（明文 JSON，不加密）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotwordWordFile {
    /// 文件格式版本（当前 1）。
    pub version: u32,
    /// 确定性 UUID（= hotword_word_uuid(set_id, word)，跨设备一致）。
    pub id: String,
    /// 所属词典 UUID。
    pub set_id: String,
    /// 词文本。
    pub word: String,
    /// 原始拼音（空格分隔，不经归一化）。
    pub pinyin: String,
    /// 软删标记：0=活跃，>0=删除时刻 epoch 秒（tombstone）。统一语义（GC 2026-08-02，原 bool 0/1）。
    /// version 1 文件 is_deleted=0/1 反序列化为 i64（1=epoch 1 秒=1970 年，超期被 GC；0=活跃）。
    #[serde(default)]
    pub is_deleted: i64,
    /// 创建时间（SQLite datetime 格式，跨设备不同但保留用于排序）。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
}

impl HotwordWordFile {
    /// 从 SQLite 行转换。
    pub fn from_hotword_word(w: &HotwordWord) -> Self {
        Self {
            version: 1,
            id: w.id.clone(),
            set_id: w.set_id.clone(),
            word: w.word.clone(),
            pinyin: w.pinyin.clone(),
            is_deleted: w.is_deleted,
            created_at: w.created_at.clone(),
            updated_at: w.updated_at.clone(),
        }
    }

    /// 转换回 HotwordWord（sync pull 用）。
    pub fn to_hotword_word(&self) -> HotwordWord {
        HotwordWord {
            id: self.id.clone(),
            set_id: self.set_id.clone(),
            word: self.word.clone(),
            pinyin: self.pinyin.clone(),
            is_deleted: self.is_deleted,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

// === 两级 outline 结构 ===

/// 总 outline——只描述词典状态（词状态归属各自词典的 `HotwordSetOutline`）。
///
/// 2026-08-01 重构：脱离复用 vault `Outline`（`ciphers`/`folders` 字段对热词语义错误）。
/// 所有字段 `#[serde(default)]`——旧 outline.json（vault Outline 格式，含 `ciphers`/
/// `folders` 无 `sets`）反序列化时 `sets` 默认空，安全降级（全新库策略：从 DB 重建）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotwordOutline {
    /// outline 格式版本（当前 1）。
    #[serde(default = "default_outline_version")]
    pub version: u32,
    /// hotword outline 整体版本（monotonic 递增，有变化时 +1）。
    #[serde(default)]
    pub hotword_version: u64,
    /// 词典 entries：set_uuid → {md5, updatedMs}。BTreeMap 保序列化顺序稳定。
    #[serde(default)]
    pub sets: BTreeMap<String, OutlineEntry>,
}

fn default_outline_version() -> u32 {
    1
}

impl Default for HotwordOutline {
    fn default() -> Self {
        Self {
            version: 1,
            hotword_version: 0,
            sets: BTreeMap::new(),
        }
    }
}

/// 词典内 outline——只描述该词典的词状态。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotwordSetOutline {
    /// outline 格式版本（当前 1）。
    #[serde(default = "default_outline_version")]
    pub version: u32,
    /// 该词典的 outline 版本（monotonic 递增，有变化时 +1）。
    #[serde(default)]
    pub hotword_version: u64,
    /// 词 entries：word_uuid → {md5, updatedMs}。BTreeMap 保序列化顺序稳定。
    #[serde(default)]
    pub words: BTreeMap<String, OutlineEntry>,
}

impl Default for HotwordSetOutline {
    fn default() -> Self {
        Self {
            version: 1,
            hotword_version: 0,
            words: BTreeMap::new(),
        }
    }
}

// === outline 读写 ===

/// 读总 outline.json。文件不存在时返回默认空 outline（首次同步）。
pub fn read_hotword_outline() -> Result<HotwordOutline> {
    let path = hotword_outline_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let outline: HotwordOutline = serde_json::from_str(&content)
                .with_context(|| format!("hotword outline.json 解析失败：{}", path.display()))?;
            Ok(outline)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HotwordOutline::default()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("读 hotword outline.json 失败：{}", path.display()))),
    }
}

/// 写总 outline.json（原子写，pretty print）。
pub fn write_hotword_outline(outline: &HotwordOutline) -> Result<()> {
    let path = hotword_outline_path();
    let json = serde_json::to_string_pretty(outline).context("序列化 hotword outline 失败")?;
    write_atomically(&path, &format!("{}\n", json))
}

/// 读词典内 outline.json。文件不存在时返回默认空 outline。
pub fn read_hotword_set_outline(set_id: &str) -> Result<HotwordSetOutline> {
    let path = hotword_set_outline_path(set_id)?;
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let outline: HotwordSetOutline = serde_json::from_str(&content).with_context(|| {
                format!("hotword set outline.json 解析失败：{}", path.display())
            })?;
            Ok(outline)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HotwordSetOutline::default()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("读 hotword set outline.json 失败：{}", path.display()))),
    }
}

/// 写词典内 outline.json（原子写）。
pub fn write_hotword_set_outline(set_id: &str, outline: &HotwordSetOutline) -> Result<()> {
    let path = hotword_set_outline_path(set_id)?;
    let json =
        serde_json::to_string_pretty(outline).context("序列化 hotword set outline 失败")?;
    write_atomically(&path, &format!("{}\n", json))
}

// === 文件读写 ===

/// 读词典元数据文件。
pub fn read_hotword_set_file(set_id: &str) -> Result<HotwordSetMeta> {
    let path = hotword_meta_file_path(set_id)?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读热词词典元数据失败：{}", path.display()))?;
    let file: HotwordSetMeta =
        serde_json::from_str(&content).context("热词词典元数据 JSON 解析失败")?;
    Ok(file)
}

/// 写词典元数据文件（原子写，含目录创建）。
pub fn write_hotword_set_file(meta: &HotwordSetMeta) -> Result<()> {
    let path = hotword_meta_file_path(&meta.id)?;
    let json = serde_json::to_string_pretty(meta).context("序列化热词词典元数据失败")?;
    write_atomically(&path, &format!("{}\n", json))
}

/// 删词典元数据文件（文件不存在时返 Ok——幂等）。
pub fn delete_hotword_set_file(set_id: &str) -> Result<()> {
    let path = hotword_meta_file_path(set_id)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("删热词词典元数据失败：{}", path.display()))),
    }
}

/// 读词记录文件。
pub fn read_hotword_word_file(set_id: &str, word_uuid: &str) -> Result<HotwordWordFile> {
    let path = hotword_word_file_path(set_id, word_uuid)?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读热词词文件失败：{}", path.display()))?;
    let file: HotwordWordFile = serde_json::from_str(&content).context("热词词文件 JSON 解析失败")?;
    Ok(file)
}

/// 写词记录文件（原子写，含分桶目录创建）。
pub fn write_hotword_word_file(file: &HotwordWordFile) -> Result<()> {
    let path = hotword_word_file_path(&file.set_id, &file.id)?;
    let json = serde_json::to_string_pretty(file).context("序列化热词词文件失败")?;
    write_atomically(&path, &format!("{}\n", json))
}

/// 删词记录文件（文件不存在时返 Ok——幂等）。
pub fn delete_hotword_word_file(set_id: &str, word_uuid: &str) -> Result<()> {
    let path = hotword_word_file_path(set_id, word_uuid)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("删热词词文件失败：{}", path.display()))),
    }
}

// === 全量导出/导入 ===

/// 从 SQLite 全量导出到文件系统——首次启用同步时用（push_initial）。
///
/// 无参：内部自己读 DB（list_all_hotword_sets + list_all_hotword_words）。
/// v58 起 set 软删——export 含 tombstone（is_deleted>0）传播删除意图。
pub fn export_all_hotwords() -> Result<HotwordOutline> {
    let sets = db::list_all_hotword_sets()?;
    let words = db::list_all_hotword_words()?;
    export_all_hotwords_with(&sets, &words)
}

/// 全量导出核心（接收数据而非读 DB）——测试用（无 DB 隔离场景）。
pub fn export_all_hotwords_with(sets: &[HotwordSet], words: &[HotwordWord]) -> Result<HotwordOutline> {
    let dir = hotword_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建 hotword 目录失败：{}", dir.display()))?;

    // 按 set_id 分组词（一个 set 的词一起写 + 一起进该 set 的 outline）
    let mut words_by_set: std::collections::HashMap<&str, Vec<&HotwordWord>> =
        std::collections::HashMap::new();
    for w in words {
        words_by_set.entry(w.set_id.as_str()).or_default().push(w);
    }

    // 1. 清空所有词典目录（每个 set 一个 <set-id>/ 目录）。保留 outline.json。
    if dir.is_dir() {
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("读 hotword 目录失败：{}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path); // 词典目录——清空重建
            }
        }
    }

    // 2. 写每个词典 + 收集总 outline 的 set entries
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut set_entries: BTreeMap<String, OutlineEntry> = BTreeMap::new();
    for h in sets {
        // GC 2026-08-02：超期 set tombstone 不写文件 + 不进 outline（清空步骤已删其目录）
        if is_tombstone_expired(h.is_deleted, now_secs) {
            continue;
        }
        // 2a. 词典目录 + meta.json
        write_hotword_set_file(&HotwordSetMeta::from_hotword_set(h))?;

        // 2b. 该词典的词文件 + 词典内 outline
        let set_words = words_by_set.get(h.id.as_str()).cloned().unwrap_or_default();
        let mut word_entries: BTreeMap<String, OutlineEntry> = BTreeMap::new();
        for w in &set_words {
            // GC：超期 word tombstone 不写文件 + 不进 outline
            if is_tombstone_expired(w.is_deleted, now_secs) {
                continue;
            }
            write_hotword_word_file(&HotwordWordFile::from_hotword_word(w))?;
            let md5 = hotword_word_md5(w);
            word_entries.insert(
                w.id.clone(),
                OutlineEntry {
                    md5,
                    updated_ms: iso_to_unix_ms(&w.updated_at),
                },
            );
        }
        let set_outline = HotwordSetOutline {
            version: 1,
            hotword_version: 1, // 首次导出从 1 开始
            words: word_entries,
        };
        write_hotword_set_outline(&h.id, &set_outline)?;

        // 2c. 总 outline 的 set entry
        let set_md5 = h.sync_md5.clone().unwrap_or_else(|| hotword_set_md5(h));
        set_entries.insert(
            h.id.clone(),
            OutlineEntry {
                md5: set_md5,
                updated_ms: iso_to_unix_ms(&h.updated_at),
            },
        );
    }

    // 3. 写总 outline.json
    let outline = HotwordOutline {
        version: 1,
        hotword_version: 1, // 首次导出从 1 开始
        sets: set_entries,
    };
    write_hotword_outline(&outline)?;

    Ok(outline)
}

/// 增量导出——sync_now 用，只写真正变化的文件（不清空目录）。
///
/// 无参：内部自己读 DB。返回 (new_outline, changed_count)。
/// v58 起 set 软删——含 tombstone 词典（is_deleted>0）传播删除意图。
pub fn incremental_export_hotwords() -> Result<(HotwordOutline, usize)> {
    let sets = db::list_all_hotword_sets()?;
    let words = db::list_all_hotword_words()?;
    incremental_export_hotwords_with(&sets, &words)
}

/// 增量导出核心（接收数据而非读 DB）——测试用（无 DB 隔离场景）。
///
/// 分两层 diff：
/// - set 层（总 outline）：新增/改 md5 的词典 → 写 meta.json + 该词典词全量重建
/// - word 层（词典内 outline）：逐词 diff md5，只写变化的词文件
pub fn incremental_export_hotwords_with(
    sets: &[HotwordSet],
    words: &[HotwordWord],
) -> Result<(HotwordOutline, usize)> {
    let dir = hotword_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("创建 hotword 目录失败：{}", dir.display()))?;

    // 读旧总 outline（解析错降级全量重建——与原 HW1 修复对齐，防 stale 文件残留复活）
    let old_outline = match read_hotword_outline() {
        Ok(o) => o,
        Err(e) => {
            log::warn!(
                "[hotword-sync] outline.json 解析失败，降级为全量重建：{}",
                e
            );
            let outline = export_all_hotwords_with(sets, words)?;
            return Ok((outline, sets.len()));
        }
    };

    let mut words_by_set: std::collections::HashMap<&str, Vec<&HotwordWord>> =
        std::collections::HashMap::new();
    for w in words {
        words_by_set.entry(w.set_id.as_str()).or_default().push(w);
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut changed = 0usize;
    let mut new_set_entries: BTreeMap<String, OutlineEntry> = BTreeMap::new();
    let id_set: std::collections::HashSet<&str> = sets.iter().map(|h| h.id.as_str()).collect();

    // set 层 diff
    for h in sets {
        // GC 2026-08-02：超期 set tombstone 不写文件 + 不进 outline + 删其目录
        if is_tombstone_expired(h.is_deleted, now_secs) {
            if let Ok(set_dir) = hotword_set_dir(&h.id) {
                let _ = std::fs::remove_dir_all(&set_dir); // 幂等
            }
            changed += 1;
            continue;
        }
        let new_md5 = h.sync_md5.clone().unwrap_or_else(|| hotword_set_md5(h));
        let old_entry = old_outline.sets.get(&h.id);
        let set_needs_rebuild = match old_entry {
            None => true,                    // 新增词典
            Some(old) => old.md5 != new_md5, // md5 变了
        };
        if set_needs_rebuild {
            changed += 1;
        }

        // 写 meta.json（新增或 md5 变时重写；幂等，无变化也不报错）
        write_hotword_set_file(&HotwordSetMeta::from_hotword_set(h))?;

        // 词典内词层 diff（每个词典都跑——即使 set 未变，词可能单独变）
        let set_words = words_by_set.get(h.id.as_str()).cloned().unwrap_or_default();
        let old_set_outline = read_hotword_set_outline(&h.id).unwrap_or_default();
        let mut new_word_entries: BTreeMap<String, OutlineEntry> = BTreeMap::new();
        let mut set_outline_changed = false;

        for w in &set_words {
            // GC：超期 word tombstone 不写文件 + 不进 outline
            if is_tombstone_expired(w.is_deleted, now_secs) {
                continue;
            }
            let new_wmd5 = hotword_word_md5(w);
            let old_wentry = old_set_outline.words.get(&w.id);
            let word_needs_write = match old_wentry {
                None => true,
                Some(old) => old.md5 != new_wmd5,
            };
            if word_needs_write {
                write_hotword_word_file(&HotwordWordFile::from_hotword_word(w))?;
                changed += 1;
                set_outline_changed = true;
            }
            new_word_entries.insert(
                w.id.clone(),
                OutlineEntry {
                    md5: new_wmd5,
                    updated_ms: iso_to_unix_ms(&w.updated_at),
                },
            );
        }

        // 词典内 outline 增量版本号
        let new_set_version = if set_outline_changed {
            old_set_outline.hotword_version.wrapping_add(1)
        } else {
            old_set_outline.hotword_version
        };
        let set_outline = HotwordSetOutline {
            version: 1,
            hotword_version: new_set_version,
            words: new_word_entries,
        };
        write_hotword_set_outline(&h.id, &set_outline)?;

        new_set_entries.insert(
            h.id.clone(),
            OutlineEntry {
                md5: new_md5,
                updated_ms: iso_to_unix_ms(&h.updated_at),
            },
        );
    }

    // 删 SQLite 无但 outline 有的词典目录。
    // ⚠️ 保护（对齐原 set 级保护）：DB 空但 .sync outline 有数据时跳过删除——防止空 DB 覆盖。
    let db_empty = sets.is_empty();
    let sync_has_data = !old_outline.sets.is_empty();
    if db_empty && sync_has_data {
        log::warn!(
            "[sync] DB 无热词但 .sync outline 有数据（sets={}）——跳过删除，防止空 DB 覆盖",
            old_outline.sets.len()
        );
    } else {
        for old_set_id in old_outline.sets.keys() {
            if !id_set.contains(old_set_id.as_str()) {
                if let Ok(set_dir) = hotword_set_dir(old_set_id) {
                    let _ = std::fs::remove_dir_all(&set_dir); // 幂等
                }
                changed += 1;
            }
        }
    }

    // ⚠️ 保护延续：db_empty && sync_has_data 时，保留旧 outline 不覆盖。
    if db_empty && sync_has_data {
        log::warn!(
            "[sync] DB 无热词——保留旧 outline 不覆盖（sets={}），词典文件也未删",
            old_outline.sets.len()
        );
        return Ok((old_outline, 0));
    }

    // 总 outline 版本号（有变化时 +1）
    let new_version = if changed > 0 {
        old_outline.hotword_version.wrapping_add(1)
    } else {
        old_outline.hotword_version
    };
    let outline = HotwordOutline {
        version: 1,
        hotword_version: new_version,
        sets: new_set_entries,
    };
    write_hotword_outline(&outline)?;

    Ok((outline, changed))
}

/// 从文件系统全量导入词典元数据——sync pull / clone_initial 用。
///
/// 扫描 hotword/ 下每个词典目录的 meta.json，返回 HotwordSetMeta 列表。
pub fn import_hotwords_from_files() -> Result<Vec<HotwordSetMeta>> {
    let dir = hotword_dir();
    let mut metas = Vec::new();
    if !dir.is_dir() {
        return Ok(metas);
    }
    // 每个词典目录（直接子目录）的 meta.json
    let mut set_dirs: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("读 hotword 目录失败：{}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            set_dirs.push(path);
        }
    }
    set_dirs.sort(); // 按路径排序，结果稳定

    for set_dir in set_dirs {
        let meta_path = set_dir.join("meta.json");
        if !meta_path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&meta_path)
            .with_context(|| format!("读词典元数据失败：{}", meta_path.display()))?;
        let meta: HotwordSetMeta = serde_json::from_str(&content)
            .with_context(|| format!("解析词典元数据失败：{}", meta_path.display()))?;
        metas.push(meta);
    }
    Ok(metas)
}

// === sync engine（pull / push）===

/// Pull 阶段（set 层）：文件系统 → SQLite。对比 outline 找出新增/修改，读文件 upsert。
///
/// 返回实际拉取的词典条数。**本函数无方向感知**（已被 [`merge_hotwords`] 取代，常规
/// sync_now 不再调用）。保留供首次 clone 场景 + 未来参考。
pub fn pull_hotwords_from_files() -> Result<usize> {
    let remote_outline = read_hotword_outline()?;
    let db_sets = db::list_all_hotword_sets()?; // 含 tombstone（is_deleted>0）——merge 需感知软删态

    let db_ids: std::collections::HashSet<&str> = db_sets.iter().map(|h| h.id.as_str()).collect();
    let mut count = 0;

    for (uuid, entry) in &remote_outline.sets {
        let needs_update = !db_ids.contains(uuid.as_str())
            || hotword_set_md5_mismatch(uuid, &entry.md5, &db_sets);
        if needs_update {
            match read_hotword_set_file(uuid) {
                Ok(file) => {
                    let h = file.to_hotword_set(None);
                    let md5 = hotword_set_md5(&h);
                    let mut h = h;
                    h.sync_md5 = Some(md5);
                    match db::upsert_hotword_set(&h) {
                        Ok(()) => count += 1,
                        Err(e) => {
                            log::warn!(
                                "[sync] 热词词典 {} pull 跳过（可能 name 冲突）：{}",
                                uuid, e
                            );
                        }
                    }
                }
                Err(e) => {
                    log::warn!(
                        "[sync] 热词词典 {} 文件读取失败，已跳过：{}",
                        uuid, e
                    );
                }
            }
        }
    }

    Ok(count)
}

/// 用 outline.md5 对比 DB sync_md5（与 vault cipher_md5_mismatch 对齐）。
fn hotword_set_md5_mismatch(uuid: &str, outline_md5: &str, db_sets: &[HotwordSet]) -> bool {
    match db_sets.iter().find(|h| h.id == uuid) {
        None => true,
        Some(h) => h.sync_md5.as_deref().unwrap_or("") != outline_md5,
    }
}

/// Push 阶段（set 层）：SQLite 最新数据 → 文件系统 + 更新 outline。
///
/// 返回实际变更（写/删）的文件数。调用方：vault engine.rs sync_now 的 push 阶段（NoUpstream
/// 首次推送分支）+ enable_sync 首次启用同步路径。
pub fn push_hotwords_to_files() -> Result<usize> {
    let (_, changed) = incremental_export_hotwords()?;
    Ok(changed)
}

// === merge engine（2026-08-01，取代 pull+push 两步）===

/// merge_hotwords 的结果报告（对称于 vault `MergeReport`，但独立于 vault crate——
/// sync 不能依赖 vault，依赖方向是 vault → sync）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HotwordMergeReport {
    /// 远程 → DB（远程 updated_at 更新 / DB 无）
    pub pulled: usize,
    /// DB → 远程（DB updated_at 更新 / outline 无 / 冲突 DB 赢）
    pub pushed: usize,
    /// updated_at 相等 + md5 不等的冲突数（DB 赢）
    pub conflicts: usize,
    /// 文件读取失败等跳过数
    pub skipped: usize,
}

/// 3-way merge：DB ↔ 文件系统按 `updated_at` 最新赢，相等时 md5 比对 DB 赢。
///
/// 分两阶段：
/// 1. **set 层 merge**（词典元数据）：遍历总 outline，逐词典 3-way 判定
/// 2. **word 层 merge**（词数据）：对每个词典读其 outline + DB 该词典的词，逐词 3-way 判定
///
/// merge 完后从 DB 最新状态重建所有文件 + outline（DB 是单一真相源）。
pub fn merge_hotwords() -> Result<HotwordMergeReport> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let remote_outline = read_hotword_outline()?;
    let db_sets = db::list_all_hotword_sets()?; // 含 tombstone（is_deleted>0）——merge 需感知软删态
    let db_by_id: std::collections::HashMap<&str, &HotwordSet> =
        db_sets.iter().map(|h| (h.id.as_str(), h)).collect();
    let mut report = HotwordMergeReport::default();

    // === 阶段 1：set 层 merge（词典元数据）===
    for (uuid, entry) in &remote_outline.sets {
        let remote_updated = entry.updated_ms;
        match db_by_id.get(uuid.as_str()) {
            None => {
                // DB 无 → pull（读 meta 文件 upsert，回填 sync_md5）
                match pull_set(uuid, now_secs) {
                    Ok(true) => report.pulled += 1,
                    Ok(false) => report.skipped += 1,
                    Err(e) => {
                        log::warn!("[sync] 热词 merge: 词典 {} 文件读取失败，已跳过：{}", uuid, e);
                        report.skipped += 1;
                    }
                }
            }
            Some(db_h) => {
                let local_updated = iso_to_unix_ms(&db_h.updated_at);
                if remote_updated > local_updated {
                    // 远程更新 → pull 覆盖 DB
                    match pull_set(uuid, now_secs) {
                        Ok(true) => report.pulled += 1,
                        Ok(false) => report.skipped += 1,
                        Err(e) => {
                            log::warn!("[sync] 热词 merge: 词典 {} pull 跳过：{}", uuid, e);
                            report.skipped += 1;
                        }
                    }
                } else if local_updated > remote_updated {
                    // DB 更新 → push 覆盖文件
                    write_hotword_set_file(&HotwordSetMeta::from_hotword_set(db_h))?;
                    report.pushed += 1;
                } else {
                    // 时间戳相等 → md5 比对，冲突 DB 赢
                    let db_md5 = db_h
                        .sync_md5
                        .clone()
                        .unwrap_or_else(|| hotword_set_md5(db_h));
                    if db_md5 != entry.md5 {
                        write_hotword_set_file(&HotwordSetMeta::from_hotword_set(db_h))?;
                        report.pushed += 1;
                        report.conflicts += 1;
                    }
                }
            }
        }
    }

    // set：DB 有 + outline 无 → push（写 meta 文件）
    for db_h in &db_sets {
        if !remote_outline.sets.contains_key(&db_h.id) {
            write_hotword_set_file(&HotwordSetMeta::from_hotword_set(db_h))?;
            report.pushed += 1;
        }
    }

    // === 阶段 2：word 层 merge（每个词典的词数据）===
    // 对每个 DB 存在的词典，读其 outline + DB 该词典的词，逐词 3-way merge。
    // 远程词典在阶段 1 已 pull 到 DB，故这里以 DB 词典为准遍历即可覆盖。
    let latest_sets = db::list_all_hotword_sets()?; // 含 tombstone——word merge 也要覆盖软删词典的词
    for set in &latest_sets {
        merge_hotword_words(&set.id, &mut report, now_secs)?;
    }

    // === 阶段 3：从 DB 最新状态重建所有文件 + outline（DB 是单一真相源）===
    export_all_hotwords()?;

    log::info!(
        "[sync] merge_hotwords 完成：pulled={} pushed={} conflicts={} skipped={}",
        report.pulled,
        report.pushed,
        report.conflicts,
        report.skipped
    );
    Ok(report)
}

/// Pull 单个词典（读 meta 文件 → upsert DB，回填 sync_md5）。
/// 返回 Ok(true) 表示成功 upsert，Ok(false) 表示 name 冲突/超期 tombstone 跳过。
/// GC 2026-08-02：超期 tombstone（is_deleted>0 且超 RETENTION）不 pull——防 GC 后跨设备复活。
fn pull_set(uuid: &str, now_secs: i64) -> Result<bool> {
    match read_hotword_set_file(uuid) {
        Ok(file) => {
            // 超期 tombstone 不复活——本机已 GC（或即将 GC），不 pull 回来
            if is_tombstone_expired(file.is_deleted, now_secs) {
                log::debug!("[sync] 热词 merge: 词典 {} 超期 tombstone，skip pull", uuid);
                return Ok(false);
            }
            let h = file.to_hotword_set(None);
            let md5 = hotword_set_md5(&h);
            let mut h = h;
            h.sync_md5 = Some(md5);
            match db::upsert_hotword_set(&h) {
                Ok(()) => Ok(true),
                Err(e) => {
                    log::warn!(
                        "[sync] 热词 merge: 词典 {} pull 跳过（可能 name 冲突）：{}",
                        uuid,
                        e
                    );
                    Ok(false)
                }
            }
        }
        Err(e) => Err(e),
    }
}

/// 单个词典的 word 级 3-way merge（对称 vault cipher merge）。
/// 读词典内 outline（远程）+ DB 该词典的词（本地），逐词判定。
fn merge_hotword_words(
    set_id: &str,
    report: &mut HotwordMergeReport,
    now_secs: i64,
) -> Result<()> {
    let remote_outline = read_hotword_set_outline(set_id)?;
    let db_words = db::list_all_hotword_words()?;
    let db_words: Vec<&HotwordWord> = db_words.iter().filter(|w| w.set_id == set_id).collect();
    let db_by_id: std::collections::HashMap<&str, &HotwordWord> =
        db_words.iter().map(|w| (w.id.as_str(), *w)).collect();

    // word：outline 有 → 3-way 判定
    for (uuid, entry) in &remote_outline.words {
        let remote_updated = entry.updated_ms;
        match db_by_id.get(uuid.as_str()) {
            None => {
                // DB 无 → pull（读词文件 upsert，回填 sync_md5）
                match pull_word(set_id, uuid, now_secs) {
                    Ok(true) => report.pulled += 1,
                    Ok(false) => report.skipped += 1,
                    Err(e) => {
                        log::warn!(
                            "[sync] 热词 merge: 词 {} 文件读取失败，已跳过：{}",
                            uuid, e
                        );
                        report.skipped += 1;
                    }
                }
            }
            Some(db_w) => {
                let local_updated = iso_to_unix_ms(&db_w.updated_at);
                if remote_updated > local_updated {
                    // 远程更新 → pull 覆盖 DB（软删传播：is_deleted=true 的词 pull 后 DB 也变软删）
                    match pull_word(set_id, uuid, now_secs) {
                        Ok(true) => report.pulled += 1,
                        Ok(false) => report.skipped += 1,
                        Err(e) => {
                            log::warn!("[sync] 热词 merge: 词 {} pull 跳过：{}", uuid, e);
                            report.skipped += 1;
                        }
                    }
                } else if local_updated > remote_updated {
                    // DB 更新 → push 覆盖文件
                    write_hotword_word_file(&HotwordWordFile::from_hotword_word(db_w))?;
                    report.pushed += 1;
                } else {
                    // 时间戳相等 → word 不可变（id=f(set_id,word)，不可改名）→ 无冲突，跳过
                }
            }
        }
    }

    // word：DB 有 + outline 无 → push（写词文件）
    for db_w in &db_words {
        if !remote_outline.words.contains_key(&db_w.id) {
            write_hotword_word_file(&HotwordWordFile::from_hotword_word(db_w))?;
            report.pushed += 1;
        }
    }

    Ok(())
}

/// Pull 单个词（读词文件 → upsert DB，回填 sync_md5）。
/// 返回 Ok(true) 表示成功 upsert，Ok(false) 不应发生（词无 name 冲突）或超期 tombstone skip。
/// GC 2026-08-02：超期 tombstone 不 pull（防 GC 后跨设备复活）。
fn pull_word(set_id: &str, uuid: &str, now_secs: i64) -> Result<bool> {
    match read_hotword_word_file(set_id, uuid) {
        Ok(file) => {
            if is_tombstone_expired(file.is_deleted, now_secs) {
                log::debug!("[sync] 热词 merge: 词 {} 超期 tombstone，skip pull", uuid);
                return Ok(false);
            }
            let w = file.to_hotword_word();
            match db::upsert_hotword_word(&w) {
                Ok(()) => Ok(true),
                Err(e) => {
                    log::warn!("[sync] 热词 merge: 词 {} upsert 跳过：{}", uuid, e);
                    Ok(false)
                }
            }
        }
        Err(e) => Err(e),
    }
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

    fn sample_set(id: &str, name: &str) -> HotwordSet {
        HotwordSet {
            id: id.into(),
            name: name.into(),
            enabled: true,
            created_at: "2026-07-22 10:00:00".into(),
            updated_at: "2026-07-22 10:00:00".into(),
            sync_md5: None,
            is_deleted: 0,
        }
    }

    // === md5 指纹测试 ===

    #[test]
    fn hotword_set_md5_same_id_name_deterministic() {
        let h1 = sample_set("uuid-1", "版本A");
        let h2 = sample_set("uuid-1", "版本A");
        assert_eq!(hotword_set_md5(&h1), hotword_set_md5(&h2));
    }

    #[test]
    fn hotword_set_md5_ignores_timestamps_and_state() {
        // md5 只含 id+name，不含 created_at/updated_at/enabled/is_deleted
        let mut h1 = sample_set("uuid-1", "版本A");
        let mut h2 = sample_set("uuid-1", "版本A");
        h2.created_at = "1999-01-01 00:00:00".into();
        h2.updated_at = "2099-12-31 23:59:59".into();
        h2.enabled = false;
        h2.is_deleted = 99999;
        let _ = &mut h1;
        assert_eq!(hotword_set_md5(&h1), hotword_set_md5(&h2),
            "md5 应不含时间戳/enabled/is_deleted——状态变化靠时间戳比较");
    }

    #[test]
    fn hotword_set_md5_changes_on_name_change() {
        let h1 = sample_set("uuid-1", "版本A");
        let mut h2 = sample_set("uuid-1", "版本B");
        let _ = &mut h2;
        assert_ne!(hotword_set_md5(&h1), hotword_set_md5(&h2));
    }

    #[test]
    fn hotword_set_md5_changes_on_id_change() {
        let h1 = sample_set("uuid-1", "版本A");
        let h2 = sample_set("uuid-2", "版本A");
        assert_ne!(hotword_set_md5(&h1), hotword_set_md5(&h2));
    }

    #[test]
    fn hotword_set_md5_from_fields_matches_struct() {
        let h = sample_set("uuid-1", "版本A");
        let from_struct = hotword_set_md5(&h);
        let from_fields = hotword_set_md5_from_fields(&h.id, &h.name);
        assert_eq!(from_struct, from_fields);
    }

    // === word md5 测试 ===

    fn sample_word(set_id: &str, word: &str, pinyin: &str, is_deleted: i64) -> HotwordWord {
        HotwordWord {
            id: octopus_infra::hotword_text::hotword_word_uuid(set_id, word),
            set_id: set_id.into(),
            word: word.into(),
            pinyin: pinyin.into(),
            is_deleted,
            created_at: "2026-07-22 10:00:00".into(),
            updated_at: "2026-07-22 10:00:00".into(),
        }
    }

    #[test]
    fn hotword_word_md5_is_deterministic() {
        let w1 = sample_word("set-1", "八爪鱼", "ba zhao yu", 0);
        let w2 = sample_word("set-1", "八爪鱼", "ba zhao yu", 0);
        assert_eq!(hotword_word_md5(&w1), hotword_word_md5(&w2));
    }

    #[test]
    fn hotword_word_md5_ignores_timestamps() {
        let mut w1 = sample_word("set-1", "八爪鱼", "ba zhao yu", 0);
        let mut w2 = sample_word("set-1", "八爪鱼", "ba zhao yu", 0);
        w2.created_at = "1999-01-01 00:00:00".into();
        w2.updated_at = "2099-12-31 23:59:59".into();
        let _ = &mut w1;
        assert_eq!(hotword_word_md5(&w1), hotword_word_md5(&w2));
    }

    #[test]
    fn hotword_word_md5_stable_on_is_deleted() {
        // md5 不含 is_deleted——软删前后身份指纹不变，状态靠 updated_at 比较
        let w1 = sample_word("set-1", "八爪鱼", "ba zhao yu", 0);
        let w2 = sample_word("set-1", "八爪鱼", "ba zhao yu", 1700000000);
        assert_eq!(
            hotword_word_md5(&w1),
            hotword_word_md5(&w2),
            "软删 is_deleted 变化不应改变 md5（身份指纹）"
        );
    }

    #[test]
    fn hotword_word_md5_changes_on_word() {
        let w1 = sample_word("set-1", "八爪鱼", "ba zhao yu", 0);
        let w2 = sample_word("set-1", "浮窗", "fu chuang", 0);
        assert_ne!(hotword_word_md5(&w1), hotword_word_md5(&w2));
    }

    /// 长度前缀防 `|` 碰撞：`{a}|{b}` vs `{a|b}|{}` 不应产生相同 md5。
    #[test]
    fn hotword_word_md5_pipe_collision_safe() {
        // 场景：set_id="x", word="a|b" 与 set_id="x|a", word="b" 的拼接不同
        let m1 = hotword_word_md5_from_fields("x", "a|b");
        let m2 = hotword_word_md5_from_fields("x|a", "b");
        assert_ne!(m1, m2, "长度前缀应防 | 碰撞");
    }

    // === 文件读写测试 ===

    #[test]
    fn hotword_set_file_round_trip() {
        let _g = SyncRootGuard::new();
        let id = "a1b2c3d4-e5f6-4789-8901-abcdef123456";
        let h = sample_set(id, "测试版本");
        let meta = HotwordSetMeta::from_hotword_set(&h);
        write_hotword_set_file(&meta).expect("write");

        let loaded = read_hotword_set_file(id).expect("read");
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.name, "测试版本");
        assert!(loaded.enabled);
    }

    #[test]
    fn delete_hotword_set_file_is_idempotent() {
        let _g = SyncRootGuard::new();
        let id = "a1b2c3d4-e5f6-4789-8901-abcdef123456";
        delete_hotword_set_file(id).expect("删不存在的文件应 Ok");
    }

    #[test]
    fn hotword_word_file_round_trip() {
        let _g = SyncRootGuard::new();
        let set_id = "a1b2c3d4-e5f6-4789-8901-abcdef123456";
        let w = sample_word(set_id, "八爪鱼", "ba zhao yu", 0);
        let file = HotwordWordFile::from_hotword_word(&w);
        write_hotword_word_file(&file).expect("write");

        let loaded = read_hotword_word_file(set_id, &w.id).expect("read");
        assert_eq!(loaded.id, w.id);
        assert_eq!(loaded.word, "八爪鱼");
        assert_eq!(loaded.pinyin, "ba zhao yu");
        assert_eq!(loaded.is_deleted, 0);
    }

    #[test]
    fn delete_hotword_word_file_is_idempotent() {
        let _g = SyncRootGuard::new();
        let set_id = "a1b2c3d4-0001";
        let word_uuid = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
        delete_hotword_word_file(set_id, word_uuid).expect("删不存在的文件应 Ok");
    }

    /// path traversal 防护：含 `..` `/` `\` 的 id 应被拒。
    #[test]
    fn hotword_paths_reject_traversal() {
        for evil in ["../../meta", "..\\..\\meta", "a/../../b", "a\\b", "", "with/slash"] {
            assert!(
                hotword_set_dir(evil).is_err(),
                "set_dir 应拒绝 path traversal：{}",
                evil
            );
            assert!(
                hotword_meta_file_path(evil).is_err(),
                "meta_path 应拒绝 path traversal：{}",
                evil
            );
            assert!(
                hotword_word_file_path(evil, "safe-uuid").is_err(),
                "word_file_path(set) 应拒绝 path traversal：{}",
                evil
            );
            assert!(
                hotword_word_file_path("safe-set", evil).is_err(),
                "word_file_path(word) 应拒绝 path traversal：{}",
                evil
            );
        }
    }

    #[test]
    fn hotword_outline_round_trip() {
        let _g = SyncRootGuard::new();
        let outline = HotwordOutline {
            version: 1,
            hotword_version: 42,
            sets: BTreeMap::from([(
                "uuid-1".into(),
                OutlineEntry {
                    md5: "md5a".into(),
                    updated_ms: 1000,
                },
            )]),
        };
        write_hotword_outline(&outline).expect("write");
        let loaded = read_hotword_outline().expect("read");
        assert_eq!(loaded.hotword_version, 42);
        assert_eq!(loaded.sets.len(), 1);
        assert_eq!(loaded.sets["uuid-1"].md5, "md5a");
    }

    #[test]
    fn hotword_set_outline_round_trip() {
        let _g = SyncRootGuard::new();
        let set_id = "a1b2c3d4-0001";
        let outline = HotwordSetOutline {
            version: 1,
            hotword_version: 7,
            words: BTreeMap::from([(
                "word-uuid-1".into(),
                OutlineEntry {
                    md5: "md5w".into(),
                    updated_ms: 2000,
                },
            )]),
        };
        write_hotword_set_outline(set_id, &outline).expect("write");
        let loaded = read_hotword_set_outline(set_id).expect("read");
        assert_eq!(loaded.hotword_version, 7);
        assert_eq!(loaded.words.len(), 1);
        assert_eq!(loaded.words["word-uuid-1"].md5, "md5w");
    }

    #[test]
    fn read_hotword_outline_missing_returns_default() {
        let _g = SyncRootGuard::new();
        let outline = read_hotword_outline().expect("应返默认空 outline");
        assert_eq!(outline.hotword_version, 0);
        assert!(outline.sets.is_empty());
    }

    // === export/import 测试 ===

    #[test]
    fn export_all_writes_all_sets() {
        let _g = SyncRootGuard::new();
        let sets = vec![
            sample_set("a1b2c3d4-e5f6-4789-8901-abcdef123456", "版本A"),
            sample_set("b2c3d4e5-f6a7-4890-9002-bcdef234567", "版本B"),
        ];
        let outline = export_all_hotwords_with(&sets, &[]).expect("export");
        assert_eq!(outline.sets.len(), 2);

        let loaded = import_hotwords_from_files().expect("import");
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn incremental_export_zero_changes_on_unchanged_data() {
        let _g = SyncRootGuard::new();
        let sets = vec![sample_set("a1b2c3d4-0001", "版本A")];

        let first = export_all_hotwords_with(&sets, &[]).expect("first export");
        let (outline, changed) = incremental_export_hotwords_with(&sets, &[]).expect("incremental");
        assert_eq!(changed, 0, "无变化时应 0 变更");
        assert_eq!(
            outline.hotword_version, first.hotword_version,
            "无变化版本不递增"
        );
    }

    #[test]
    fn incremental_export_writes_only_changed() {
        let _g = SyncRootGuard::new();
        let sets = vec![
            sample_set("a1b2c3d4-0001", "版本A"),
            sample_set("a1b2c3d4-0002", "版本B"),
        ];
        export_all_hotwords_with(&sets, &[]).expect("initial");

        let mut sets2 = sets.clone();
        sets2[0].name = "版本A改".into();
        let (_, changed) = incremental_export_hotwords_with(&sets2, &[]).expect("incremental");
        assert_eq!(changed, 1, "只改了一个版本，应 1 变更");
    }

    #[test]
    fn incremental_export_deletes_missing() {
        let _g = SyncRootGuard::new();
        let sets = vec![
            sample_set("a1b2c3d4-0001", "版本A"),
            sample_set("a1b2c3d4-0002", "版本B"),
        ];
        export_all_hotwords_with(&sets, &[]).expect("initial");

        let sets2 = vec![sample_set("a1b2c3d4-0001", "版本A")];
        let (outline, changed) = incremental_export_hotwords_with(&sets2, &[]).expect("incremental");
        assert_eq!(changed, 1, "删了一个版本，应 1 变更");
        assert_eq!(outline.sets.len(), 1);
        assert!(outline.sets.contains_key("a1b2c3d4-0001"));

        assert!(
            read_hotword_set_file("a1b2c3d4-0002").is_err(),
            "已删版本的 meta 文件不应存在"
        );
    }

    /// 回归守护（2026-07-27 sync 覆盖 bug）：DB 完全空 + .sync outline 有数据时，
    /// 不删除 .sync 文件——防止清库后空 DB 覆盖 .sync 已有热词。
    #[test]
    fn incremental_export_protects_sync_data_when_db_empty() {
        let _g = SyncRootGuard::new();
        let sets = vec![sample_set("a1b2c3d4-0001", "版本A")];
        export_all_hotwords_with(&sets, &[]).expect("initial");
        assert!(read_hotword_set_file("a1b2c3d4-0001").is_ok());

        let (_outline, changed) = incremental_export_hotwords_with(&[], &[]).expect("empty");
        assert_eq!(changed, 0, "DB 空 + .sync 有数据时不应删任何文件");
        assert!(
            read_hotword_set_file("a1b2c3d4-0001").is_ok(),
            "DB 空时 .sync 的热词文件应保留（防止覆盖）"
        );
    }

    #[test]
    fn import_returns_files_with_correct_data() {
        let _g = SyncRootGuard::new();
        let sets = vec![
            sample_set("a1b2c3d4-0001", "版本A"),
            sample_set("b2c3d4e5-0002", "版本B"),
        ];
        export_all_hotwords_with(&sets, &[]).expect("export");

        let loaded = import_hotwords_from_files().expect("import");
        assert_eq!(loaded.len(), 2);
        let a = loaded.iter().find(|f| f.name == "版本A").expect("应有版本A");
        assert!(a.enabled);
    }

    // === sync engine 集成测试（pull / push） ===

    use octopus_infra::db;

    /// 测试辅助：空格分隔的词 → Vec<String>，调 add_words_to_set。
    fn add_words(set_id: &str, words: &str) {
        let ws: Vec<String> = words.split_whitespace().map(|s| s.to_string()).collect();
        db::add_words_to_set(set_id, &ws).unwrap();
    }

    /// DB + sync_root 双隔离 guard——pull/push 测试用。
    struct DbSyncGuard {
        _tmp: TempDir,
    }
    impl DbSyncGuard {
        fn new() -> Self {
            let tmp = TempDir::new().expect("tempdir");
            let sync_path = tmp.path().join(".sync");
            std::fs::create_dir_all(&sync_path).unwrap();
            crate::store::set_test_sync_root(sync_path);
            let conn = rusqlite::Connection::open_in_memory().unwrap();
            db::set_test_db(conn);
            Self { _tmp: tmp }
        }
    }
    impl Drop for DbSyncGuard {
        fn drop(&mut self) {
            crate::store::clear_test_sync_root();
        }
    }

    /// A 机 export → B 机 pull：B 应看到 A 的热词版本。
    #[test]
    fn pull_imports_sets_exported_by_other_device() {
        let _g = DbSyncGuard::new();
        let initial = db::list_hotword_sets().unwrap();
        for h in &initial {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id_a = "aaaaaaaa-0001";
        db::insert_hotword_set(id_a, "A机版本").unwrap();
        add_words(id_a, "苹果 香蕉");
        export_all_hotwords().expect("A export");

        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }
        assert!(db::list_hotword_sets().unwrap().is_empty(), "B 机初始应空");

        let pulled = pull_hotwords_from_files().expect("B pull");
        assert_eq!(pulled, 1, "应拉取 1 个版本");

        let b_sets = db::list_hotword_sets().unwrap();
        assert_eq!(b_sets.len(), 1);
        assert_eq!(b_sets[0].name, "A机版本");
        assert_eq!(b_sets[0].id, id_a);
        assert!(b_sets[0].sync_md5.is_some(), "pull 后应有 sync_md5");
    }

    /// 双向同步：A 改 name + B 加词 → 双方 push/pull 后数据一致。
    #[test]
    fn bidirectional_sync_converges() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id = "bbbbbbbb-0001";
        db::insert_hotword_set(id, "原版本").unwrap();
        add_words(id, "苹果");

        export_all_hotwords().expect("initial export");

        db::rename_hotword_set(id, "A机改名").unwrap();
        push_hotwords_to_files().expect("A push");

        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }
        pull_hotwords_from_files().expect("B pull");
        let b_sets = db::list_hotword_sets().unwrap();
        assert_eq!(b_sets[0].name, "A机改名", "B 应看到 A 改的 name");
    }

    /// 删除传播（v58 软删语义）：A 软删热词版本 → push → B pull 后版本也变软删（tombstone 传播）。
    /// 软删后：版本2 仍在 DB（is_deleted>0）+ meta.json 仍在文件（is_deleted>0 tombstone），
    /// list_hotword_sets 过滤掉（用户看不见），但不再是「文件消失」式硬删。
    #[test]
    fn delete_propagates_through_sync() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id1 = "cccccccc-0001";
        let id2 = "cccccccc-0002";
        db::insert_hotword_set(id1, "版本1").unwrap();
        db::insert_hotword_set(id2, "版本2").unwrap();

        export_all_hotwords().expect("initial");

        // A 软删版本2（is_deleted=时间戳，tombstone）+ push
        db::delete_hotword_set(id2).unwrap();
        push_hotwords_to_files().expect("A push after delete");

        // 模拟 B 机：硬清活跃集（仅版本1 活跃，版本2 已软删不在 list 里）→ pull
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }
        pull_hotwords_from_files().expect("B pull");

        // B 的 list_hotword_sets 只剩版本1（版本2 是 tombstone，is_deleted>0 被过滤）
        let b_sets = db::list_hotword_sets().unwrap();
        assert_eq!(b_sets.len(), 1, "B 活跃版本应只 1 个（版本1）");
        assert_eq!(b_sets[0].id, id1);

        // 版本2 的 tombstone 文件仍在（is_deleted>0）——软删不删文件
        let tombstone = read_hotword_set_file(id2).expect("tombstone meta.json 应存在（软删不删文件）");
        assert!(tombstone.is_deleted > 0, "tombstone meta 应 is_deleted>0");
    }

    #[test]
    fn push_twice_second_time_zero_changes() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        db::insert_hotword_set("dddddddd-0001", "版本").unwrap();
        add_words("dddddddd-0001", "苹果");

        let first = push_hotwords_to_files().expect("first push");
        assert!(first > 0, "首次应有变更");

        let second = push_hotwords_to_files().expect("second push");
        assert_eq!(second, 0, "无变化时应 0 变更");
    }

    // === 边界场景补充 ===

    #[test]
    fn export_empty_set_list_is_safe() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }
        assert!(db::list_hotword_sets().unwrap().is_empty());

        let outline = export_all_hotwords_with(&[], &[]).expect("export empty");
        assert!(outline.sets.is_empty());

        let (outline2, changed) = incremental_export_hotwords_with(&[], &[]).expect("incremental empty");
        assert!(outline2.sets.is_empty());
        assert_eq!(changed, 0);

        let pushed = push_hotwords_to_files().expect("push empty");
        assert_eq!(pushed, 0);

        let pulled = pull_hotwords_from_files().expect("pull empty");
        assert_eq!(pulled, 0);
    }

    #[test]
    fn enabled_toggle_propagates_through_sync() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id = "eeeeeeee-0001";
        db::insert_hotword_set(id, "版本").unwrap();
        add_words(id, "苹果");
        let md5 = hotword_set_md5(&db::get_hotword_set(id).unwrap());
        db::update_hotword_set_sync_md5(id, &md5).unwrap();
        assert!(db::get_hotword_set(id).unwrap().enabled);

        export_all_hotwords().expect("export enabled=true");

        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }
        pull_hotwords_from_files().expect("pull");
        assert!(db::get_hotword_set(id).unwrap().enabled);

        db::toggle_hotword_set(id, false).unwrap();
        let md5 = hotword_set_md5(&db::get_hotword_set(id).unwrap());
        db::update_hotword_set_sync_md5(id, &md5).unwrap();
        push_hotwords_to_files().expect("push disabled");

        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }
        pull_hotwords_from_files().expect("pull again");
        assert!(!db::get_hotword_set(id).unwrap().enabled);
    }

    #[test]
    fn pull_same_name_different_uuid_does_not_conflict() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id_a = "ffffffff-aaaa";
        db::insert_hotword_set(id_a, "同名版本").unwrap();
        add_words(id_a, "苹果");
        export_all_hotwords().expect("export A");

        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id_b = "ffffffff-bbbb";
        db::insert_hotword_set(id_b, "同名版本").unwrap();
        add_words(id_b, "香蕉");

        let pulled = pull_hotwords_from_files().expect("pull 不应 panic");
        assert_eq!(pulled, 0, "name 冲突的版本应被跳过，pulled=0");

        let sets = db::list_hotword_sets().unwrap();
        assert_eq!(sets.len(), 1);
        assert!(sets.iter().any(|h| h.id == id_b));
        assert!(!sets.iter().any(|h| h.id == id_a));
    }

    #[test]
    fn pull_skips_corrupted_set_file() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id_ok = "11111111-0001";
        db::insert_hotword_set(id_ok, "正常版本").unwrap();
        export_all_hotwords().expect("export");

        // 伪造一个损坏的词典目录（额外 set，meta.json 写无效 JSON）
        let corrupt_dir = hotword_set_dir("22222222-corrupt").unwrap();
        std::fs::create_dir_all(&corrupt_dir).unwrap();
        std::fs::write(corrupt_dir.join("meta.json"), "{ this is not valid json }").unwrap();

        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }
        // pull 只读 outline 列出的 set——伪造目录不在 outline 里，不会读到，不阻断
        let pulled = pull_hotwords_from_files().expect("pull 不应因损坏文件 panic");
        assert_eq!(pulled, 1, "只应拉取正常版本");
        assert!(db::get_hotword_set(id_ok).is_ok(), "正常版本应在 DB");
    }

    #[test]
    fn pull_function_direction_blind_by_design() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id = "dddddddd-0001";
        db::insert_hotword_set(id, "测试集").unwrap();
        export_all_hotwords().expect("export 旧版本");

        db::rename_hotword_set(id, "测试集改").unwrap();
        let md5 = hotword_set_md5(&db::get_hotword_set(id).unwrap());
        db::update_hotword_set_sync_md5(id, &md5).unwrap();

        let _pulled = pull_hotwords_from_files().expect("pull");

        let after = db::get_hotword_set(id).unwrap();
        assert_eq!(
            after.name, "测试集",
            "pull 无方向感知，会用旧文件覆盖新 DB name（设计契约；常规 sync 用 merge_hotwords 避免此行为）"
        );
    }

    #[test]
    fn push_exports_local_new_data_when_outline_stale() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id = "eeeeeeee-0001";
        db::insert_hotword_set(id, "测试集").unwrap();
        export_all_hotwords().expect("export 旧版本");

        db::rename_hotword_set(id, "测试集改").unwrap();
        let md5 = hotword_set_md5(&db::get_hotword_set(id).unwrap());
        db::update_hotword_set_sync_md5(id, &md5).unwrap();

        let pushed = push_hotwords_to_files().expect("push");
        assert_eq!(pushed, 1, "应导出 1 个变化的版本");

        let file = read_hotword_set_file(id).expect("read file");
        assert_eq!(file.name, "测试集改");
    }

    #[test]
    fn incremental_export_version_increments_only_on_change() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let sets = vec![sample_set("33333333-0001", "版本A")];
        let (outline1, changed1) = incremental_export_hotwords_with(&sets, &[]).expect("first");
        assert!(changed1 > 0);
        let v1 = outline1.hotword_version;

        let (outline2, changed2) = incremental_export_hotwords_with(&sets, &[]).expect("second");
        assert_eq!(changed2, 0);
        assert_eq!(outline2.hotword_version, v1, "无变化版本不递增");

        let sets2 = vec![sample_set("33333333-0001", "版本A改")];
        let (outline3, changed3) = incremental_export_hotwords_with(&sets2, &[]).expect("third");
        assert!(changed3 > 0);
        assert!(outline3.hotword_version > v1, "有变化版本应递增");
    }

    // === merge_hotwords set 级测试 ===
    //
    // 时间戳策略：DB 的 updated_at = datetime('now') ≈ 当前毫秒。构造「远程更新」用
    // 远未来 updated_ms（如 9999999999999）；「远程更旧」用 updated_ms: 1。

    /// 辅助：手写一份总 outline + 词典 meta，模拟「远程仓库」状态。
    /// is_deleted 默认 0（活跃）；tombstone 测试传 >0（删除时刻 epoch 秒）。
    fn write_remote_set(id: &str, name: &str, updated_ms: i64) {
        write_remote_set_with(id, name, updated_ms, 0);
    }

    /// 辅助：write_remote_set 的 tombstone 版（可指定 is_deleted）。
    fn write_remote_set_with(id: &str, name: &str, updated_ms: i64, is_deleted: i64) {
        let meta = HotwordSetMeta {
            version: 2,
            id: id.into(),
            name: name.into(),
            enabled: true,
            is_deleted,
            created_at: "2026-07-22 10:00:00".into(),
            updated_at: "2026-07-22 10:00:00".into(),
        };
        write_hotword_set_file(&meta).unwrap();
        // 词典内 outline（空词）
        write_hotword_set_outline(id, &HotwordSetOutline::default()).unwrap();
        let mut outline = read_hotword_outline().unwrap_or_default();
        let md5 = hotword_set_md5_from_fields(id, name);
        outline.sets.insert(
            id.into(),
            OutlineEntry {
                md5,
                updated_ms,
            },
        );
        write_hotword_outline(&outline).unwrap();
    }

    /// merge（set 层）：远程 updated_ms 较新 → pull 覆盖 DB。
    #[test]
    fn merge_pulls_remote_newer_set() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id = "merge-aaaa-0001";
        db::insert_hotword_set(id, "旧名").unwrap();
        export_all_hotwords().expect("export 旧版本");

        write_remote_set(id, "新名", 9999999999999);

        let report = merge_hotwords().expect("merge");
        assert_eq!(report.pulled, 1, "应拉取 1 条远程更新");

        let after = db::get_hotword_set(id).unwrap();
        assert_eq!(after.name, "新名", "DB name 应被远程新版本覆盖");
    }

    /// merge（核心回归）：本地新加词、outline 仍是旧的 → DB 不被旧文件覆盖，且文件被更新。
    #[test]
    fn merge_keeps_local_newer_set_not_overwritten() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id = "merge-bbbb-0001";
        db::insert_hotword_set(id, "测试集").unwrap();
        export_all_hotwords().expect("export 旧版本");

        db::rename_hotword_set(id, "测试集改").unwrap();
        let md5 = hotword_set_md5(&db::get_hotword_set(id).unwrap());
        db::update_hotword_set_sync_md5(id, &md5).unwrap();

        let mut stale_outline = read_hotword_outline().unwrap();
        stale_outline.sets.insert(
            id.into(),
            OutlineEntry {
                md5: hotword_set_md5_from_fields(id, "测试集"),
                updated_ms: 1,
            },
        );
        write_hotword_outline(&stale_outline).unwrap();

        let report = merge_hotwords().expect("merge");

        let after = db::get_hotword_set(id).unwrap();
        assert_eq!(
            after.name, "测试集改",
            "本地新 name 不应被旧 outline 覆盖（merge 方向感知）"
        );
        assert!(report.pushed >= 1, "本地更新应 push 到文件");
    }

    /// merge：DB 有、outline 无 → push 写文件 + outline 重建含该条目。
    #[test]
    fn merge_pushes_db_only_set() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id = "merge-cccc-0001";
        db::insert_hotword_set(id, "仅本地").unwrap();
        add_words(id, "苹果");
        write_hotword_outline(&HotwordOutline::default()).unwrap();

        let report = merge_hotwords().expect("merge");
        assert!(report.pushed >= 1, "DB only set 应 push 到文件");

        let _file = read_hotword_set_file(id).expect("文件应存在");

        let outline = read_hotword_outline().unwrap();
        assert!(outline.sets.contains_key(id), "outline 应含新 push 条目");
    }

    /// merge：updated_ms 相等 + md5 不等（内容冲突）→ DB 赢（push DB 到文件）。
    #[test]
    fn merge_db_wins_on_equal_timestamp_md5_conflict() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let id = "merge-dddd-0001";
        db::insert_hotword_set(id, "新名").unwrap();
        let db_updated_ms = iso_to_unix_ms(&db::get_hotword_set(id).unwrap().updated_at);

        write_remote_set(id, "旧名", db_updated_ms);

        let report = merge_hotwords().expect("merge");
        assert!(report.conflicts >= 1, "应记录 1 次冲突（name 不同 + 时间戳相等）");

        let file = read_hotword_set_file(id).expect("read file");
        assert_eq!(file.name, "新名", "冲突时 DB 赢——文件 name 应为 DB 的「新名」");
    }

    // === merge_hotwords word 级测试（核心——4+1 场景）===
    //
    // 对称 set 级 merge 测试。write_remote_word 手写词文件 + 词典 outline 模拟远程状态。

    /// 辅助：手写一个远程词文件 + 词典 outline 条目，模拟「远程仓库」某词状态。
    /// set_id 需已存在于 DB（word merge 遍历 DB 词典）。
    fn write_remote_word(set_id: &str, word: &str, pinyin: &str, is_deleted: i64, updated_ms: i64) {
        let word_uuid = octopus_infra::hotword_text::hotword_word_uuid(set_id, word);
        let file = HotwordWordFile {
            version: 1,
            id: word_uuid.clone(),
            set_id: set_id.into(),
            word: word.into(),
            pinyin: pinyin.into(),
            is_deleted,
            created_at: "2026-07-22 10:00:00".into(),
            updated_at: "2026-07-22 10:00:00".into(),
        };
        write_hotword_word_file(&file).unwrap();
        let mut outline = read_hotword_set_outline(set_id).unwrap_or_default();
        let md5 = hotword_word_md5_from_fields(set_id, word);
        outline.words.insert(
            word_uuid,
            OutlineEntry {
                md5,
                updated_ms,
            },
        );
        write_hotword_set_outline(set_id, &outline).unwrap();
    }

    /// 测试辅助：某 set 的全部词（含软删，按 word 排序），用于断言。
    fn all_words_in_set(set_id: &str) -> Vec<(String, i64)> {
        db::list_all_hotword_words()
            .unwrap()
            .into_iter()
            .filter(|w| w.set_id == set_id)
            .map(|w| (w.word, w.is_deleted))
            .collect()
    }

    /// word merge 场景 1：远程加了 DB 没有的新词（updated_ms 远未来）→ DB pull 到该词。
    #[test]
    fn merge_pulls_remote_newer_word() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let set_id = "word-aaaa-0001";
        db::insert_hotword_set(set_id, "测试词典").unwrap();
        // 远程加词「八爪鱼」
        write_remote_word(set_id, "八爪鱼", "ba zhao yu", 0, 9999999999999);

        let report = merge_hotwords().expect("merge");

        // DB 应 pull 到「八爪鱼」
        let words = all_words_in_set(set_id);
        assert!(
            words.iter().any(|(w, d)| w == "八爪鱼" && *d == 0),
            "DB 应 pull 到远程新词「八爪鱼」: {:?}",
            words
        );
        assert!(report.pulled >= 1, "应记录至少 1 次 pull");
    }

    /// word merge 场景 2：本地加了新词、词典 outline 仍是旧的 → DB 不被覆盖，文件被更新。
    #[test]
    fn merge_keeps_local_newer_word_not_overwritten() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let set_id = "word-bbbb-0001";
        db::insert_hotword_set(set_id, "测试词典").unwrap();
        add_words(set_id, "八爪鱼");
        export_all_hotwords().expect("export 初始");

        // 本地再加一词「浮窗」（词典 outline 仍是旧的）
        add_words(set_id, "浮窗");

        // 手写一份旧词典 outline（「浮窗」无 entry，updated_ms=1 远旧）
        let word_uuid = octopus_infra::hotword_text::hotword_word_uuid(set_id, "八爪鱼");
        let stale = HotwordSetOutline {
            version: 1,
            hotword_version: 0,
            words: BTreeMap::from([(
                word_uuid,
                OutlineEntry {
                    md5: hotword_word_md5_from_fields(set_id, "八爪鱼"),
                    updated_ms: 1,
                },
            )]),
        };
        write_hotword_set_outline(set_id, &stale).unwrap();

        let report = merge_hotwords().expect("merge");

        // 「浮窗」不应丢失
        let words = all_words_in_set(set_id);
        assert!(
            words.iter().any(|(w, d)| w == "浮窗" && *d == 0),
            "本地新词「浮窗」不应被旧 outline 覆盖: {:?}",
            words
        );
        assert!(report.pushed >= 1, "本地新词应 push 到文件");
    }

    /// word merge 场景 3：DB 有词、词典 outline 无 → push 写词文件 + outline 重建含。
    #[test]
    fn merge_pushes_db_only_word() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let set_id = "word-cccc-0001";
        db::insert_hotword_set(set_id, "测试词典").unwrap();
        add_words(set_id, "苹果");
        // 词典 outline 为空（不 export 该词）
        write_hotword_set_outline(set_id, &HotwordSetOutline::default()).unwrap();

        let report = merge_hotwords().expect("merge");
        assert!(report.pushed >= 1, "DB only 词应 push 到文件");

        // outline 重建后含该词
        let outline = read_hotword_set_outline(set_id).unwrap();
        let word_uuid = octopus_infra::hotword_text::hotword_word_uuid(set_id, "苹果");
        assert!(
            outline.words.contains_key(&word_uuid),
            "词典 outline 应含 push 的词"
        );
    }

    /// word merge 场景 4：软删跨设备传播——A 软删词（is_deleted=1, updated_ms 新）→ B merge 后该词变软删。
    #[test]
    fn merge_soft_delete_propagates() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let set_id = "word-dddd-0001";
        db::insert_hotword_set(set_id, "测试词典").unwrap();
        add_words(set_id, "八爪鱼");
        export_all_hotwords().expect("export 初始（is_deleted=0）");

        // 远程（A 机）软删「八爪鱼」——is_deleted=当前秒（未超期 tombstone，应传播）, updated_ms 远未来
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        write_remote_word(set_id, "八爪鱼", "ba zhao yu", now_secs, 9999999999999);

        merge_hotwords().expect("merge");

        // B 机 DB 的「八爪鱼」应变软删（is_deleted>0）
        let w = db::get_hotword_word(set_id, "八爪鱼").unwrap().unwrap();
        assert!(
            w.is_deleted > 0,
            "软删应跨设备传播：B 机「八爪鱼」应 is_deleted>0，实际 is_deleted={}",
            w.is_deleted
        );
    }

    /// word merge 场景 5：updated_ms 相等 + md5 不等（内容冲突）→ DB 赢（push DB 到文件）。
    /// 注：v58 起 md5 只含 set_id+word（不含 pinyin/is_deleted），拼音差异不再触发 md5 冲突。
    /// 同词必然同拼音（拼音从词派生），故此场景改为「同 uuid 不同词文本」模拟冲突。
    #[test]
    fn merge_db_wins_on_equal_timestamp_word_conflict() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let set_id = "word-eeee-0001";
        db::insert_hotword_set(set_id, "测试词典").unwrap();
        add_words(set_id, "八爪鱼");
        let db_updated_ms = iso_to_unix_ms(
            &db::get_hotword_word(set_id, "八爪鱼").unwrap().unwrap().updated_at,
        );

        // 远程写同 uuid 但不同词文本（md5 不同），updated_ms 与 DB 相等 → 冲突 DB 赢
        let word_uuid = octopus_infra::hotword_text::hotword_word_uuid(set_id, "八爪鱼");
        write_remote_word(set_id, "九爪鱼", "jiu zhao yu", 0, db_updated_ms);
        // 修正 outline 的 word uuid 映射（write_remote_word 用 word 算 uuid，需覆盖为 DB 的 uuid）
        let mut outline = read_hotword_set_outline(set_id).unwrap_or_default();
        outline.words.remove(&octopus_infra::hotword_text::hotword_word_uuid(set_id, "九爪鱼"));
        outline.words.insert(word_uuid.clone(), OutlineEntry {
            md5: hotword_word_md5_from_fields(set_id, "九爪鱼"),
            updated_ms: db_updated_ms,
        });
        write_hotword_set_outline(set_id, &outline).unwrap();

        let report = merge_hotwords().expect("merge");
        assert!(report.conflicts >= 1 || report.pushed >= 1, "应记录 word 冲突或 push");

        // 文件应被 DB 内容覆盖——读回验证
        let file = read_hotword_word_file(set_id, &word_uuid).expect("read file");
        assert_eq!(
            file.word, "八爪鱼",
            "冲突 DB 赢——文件 word 应为 DB 的「八爪鱼」"
        );
    }

    // === set 级软删 tombstone 传播（v58，对称 word 级 merge_soft_delete_propagates）===

    /// set 级软删跨设备传播：A 软删集（is_deleted=时间戳，updated_ms 新）→ B merge 后该集也变软删。
    /// 对称 word 级 merge_soft_delete_propagates（1891-1915）。
    #[test]
    fn merge_set_soft_delete_propagates() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let set_id = "set-softdel-0001";
        db::insert_hotword_set(set_id, "待删词典").unwrap();
        add_words(set_id, "八爪鱼");
        export_all_hotwords().expect("export 初始（is_deleted=0）");

        // 远程（A 机）软删该集——is_deleted=时间戳, updated_ms 远未来（比 DB 的 now 新）
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(1800000000);
        write_remote_set_with(set_id, "待删词典", 9999999999999, now_secs);

        merge_hotwords().expect("merge");

        // B 机 DB 的该集应变软删（is_deleted>0）
        let s = db::get_hotword_set(set_id).unwrap();
        assert!(
            s.is_deleted > 0,
            "set 级软删应跨设备传播：B 机该集应 is_deleted>0，实际 {}",
            s.is_deleted
        );
        // list_hotword_sets 过滤掉（用户看不见）
        assert!(
            db::list_hotword_sets().unwrap().iter().all(|x| x.id != set_id),
            "软删的集不应出现在 list_hotword_sets"
        );
    }

    // === GC 年龄过滤（2026-08-02，防超期 tombstone 跨设备复活）===

    /// 超期 set tombstone 不 pull（防复活）：A GC 删了超期 tombstone → B merge 时即使旧 outline
    /// 有该 tombstone（读 meta.json 发现 is_deleted 超期）→ skip，不 pull 回 DB。
    #[test]
    fn merge_expired_set_tombstone_not_resurrected() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let set_id = "gc-aaaa-0001";
        db::insert_hotword_set(set_id, "待GC词典").unwrap();
        export_all_hotwords().expect("export 初始");

        // 模拟 A 机 GC：硬删 DB tombstone（这里直接不建 tombstone，模拟 GC 后 DB 无该集）
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        // 但远程 outline + meta.json 仍有该集的**超期** tombstone（is_deleted=远过去，如 1000 秒=1970 年）
        write_remote_set_with(set_id, "待GC词典", 9999999999999, 1000);

        merge_hotwords().expect("merge");

        // B 机不应 pull 回这个超期 tombstone——DB 里不应有该集
        assert!(
            db::get_hotword_set(set_id).is_err(),
            "超期 tombstone 不应被 pull 复活"
        );
        // export 后 outline 也不应含（merge 末尾 export 过滤超期）
        let outline = read_hotword_outline().unwrap();
        assert!(
            !outline.sets.contains_key(set_id),
            "超期 tombstone 不应进 outline"
        );
    }

    /// 超期 word tombstone 不 pull（防复活）+ export 不含。
    #[test]
    fn merge_expired_word_tombstone_not_resurrected() {
        let _g = DbSyncGuard::new();
        for h in db::list_hotword_sets().unwrap() {
            let _ = db::hard_delete_hotword_set(&h.id);
        }

        let set_id = "gc-bbbb-0001";
        db::insert_hotword_set(set_id, "测试词典").unwrap();
        add_words(set_id, "八爪鱼");
        export_all_hotwords().expect("export 初始");

        // 模拟 A 机 GC：硬删「八爪鱼」word tombstone（这里直接硬删，模拟 GC 后 DB 无该词）
        db::hard_delete_hotword_set(set_id).unwrap();
        db::insert_hotword_set(set_id, "测试词典").unwrap();

        // 远程有「八爪鱼」的**超期** word tombstone（is_deleted=1000=1970 年，超期）
        write_remote_word(set_id, "八爪鱼", "ba zhao yu", 1000, 9999999999999);

        merge_hotwords().expect("merge");

        // B 机不应 pull 回超期 word tombstone
        assert!(
            db::get_hotword_word(set_id, "八爪鱼").unwrap().is_none(),
            "超期 word tombstone 不应被 pull 复活"
        );
    }
}
