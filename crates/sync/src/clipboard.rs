//! 剪贴板收藏同步——`.sync/clipboard/` outline + favorites 文件 + clipboard.key 加密。
//!
//! 剪贴板收藏是 `.sync/` 目录扩展的第二个非 vault 数据类型（继 hotword 后）。与
//! vault/hotword 同步的区别：
//! - **对称加密**：收藏内容可能含敏感文本（用户复制的密码片段等），用 clipboard.key
//!   （32B AES-256-GCM，hex 存文件 0600）对称加密后写入 favorites 文件。
//! - **收藏 = 历史行的引用**：`FavoritePayload` 内嵌一份 `HistoryRowJson` 快照（不是引用
//!   历史行 id），加密后跨设备自包含——B 机无需先同步历史即可还原收藏。
//!
//! ## 目录结构
//!
//! ```text
//! ~/.octopus/.sync/clipboard/
//! ├── clipboard.key              ← 32B AES-256-GCM key（hex 编码，0600 权限，先防君子）
//! ├── outline.json               ← 增量索引：favorite_uuid → {md5, updatedMs}
//! └── favorites/
//!     └── <2hex>/<uuid>.json     ← 单个收藏文件（按 favorite uuid 前 2 hex 分片）
//! ```
//!
//! **分片**：与 vault/hotword 一致——`favorites/<前2hex>/<uuid>.json`，256 桶分散文件
//! 避免 git diff 单一大目录 + ls-tree 毫秒级。**uuid 用 favorite uuid v4 字符串**（Task 2
//! 已切到 UUID v4 string ids）。
//!
//! ## 加密原语内联说明
//!
//! clipboard 的 AES-256-GCM 加密在本 crate 内联（[`ClipboardKey`] + [`encrypt`]/[`decrypt`]），
//! **不**依赖 `octopus_vault::crypto::DerivedKey`——因为依赖方向是 `sync ← vault`
//! （vault 已依赖 sync），sync 反向依赖 vault 会形成循环。
//! 密文格式与 vault `DerivedKey` **byte-compatible**：都是 `v1:<base64(nonce[12B]||ct||tag[16B])>`，
//! 未来若 clipboard.key 迁到 vault 体系，已加密文件无需重新加密。与 `store::write_atomically`
//! 同模式（vault 的 private 函数跨 crate 拿不到 → sync 内联一份）。
//!
//! 详见 plan：`docs/superpowers/plans/2026-08-03-clipboard-favorite-sync.md`（Task 4+5）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{ensure, Context, Result};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::outline::OutlineEntry;
use crate::store::{iso_to_unix_ms, md5_hex, shard_dir, sync_root, write_atomically};

// === 路径辅助 ===

/// `~/.octopus/.sync/clipboard/`——剪贴板数据子目录。
pub fn clipboard_dir() -> Result<PathBuf> {
    Ok(sync_root().join("clipboard"))
}

/// `~/.octopus/.sync/clipboard/clipboard.key`——AES-256-GCM 对称密钥（hex 存文件）。
pub fn clipboard_key_path() -> Result<PathBuf> {
    Ok(clipboard_dir()?.join("clipboard.key"))
}

/// `~/.octopus/.sync/clipboard/favorites/`——收藏文件根目录（其下按 2hex 分片）。
fn favorites_dir() -> Result<PathBuf> {
    Ok(clipboard_dir()?.join("favorites"))
}

/// `~/.octopus/.sync/clipboard/outline.json`——收藏增量索引。
fn outline_path() -> Result<PathBuf> {
    Ok(clipboard_dir()?.join("outline.json"))
}

/// 收藏文件路径：`clipboard/favorites/<2hex>/<uuid>.json`（复用 `shard_dir`，按 uuid 分桶）。
///
/// 与 vault/hotword 一致——uuid 前 2 hex 字符作分片子目录，分散文件避免单一大目录。
fn favorite_file_path(uuid: &str) -> Result<PathBuf> {
    let dir = favorites_dir()?;
    Ok(dir.join(shard_dir(uuid)).join(format!("{uuid}.json")))
}

// === AES-256-GCM 加密原语（内联——byte-compatible with vault DerivedKey）===

/// 密文前缀（与 vault `symmetric::CIPHERTEXT_PREFIX` 一致）。
const CIPHERTEXT_PREFIX: &str = "v1:";
/// GCM nonce 长度（12B，标准）。
const NONCE_LEN: usize = 12;

/// 32B AES-256-GCM 对称密钥——clipboard.key 加载后的内存形态。
///
/// Drop 时自动清零（`Zeroizing` 包装）。**byte-compatible with vault `DerivedKey`**：
/// 同样的 32B key + 同样的 `v1:<base64(nonce||ct||tag)>` 密文格式，已加密文件未来可
/// 无缝迁到 vault 体系。本 crate 内联而非依赖 vault，因依赖方向是 sync ← vault。
#[derive(Clone, Debug)]
pub struct ClipboardKey(Zeroizing<[u8; 32]>);

impl ClipboardKey {
    /// 从已知 32B 数组构造（用于加载 clipboard.key）。
    pub fn from_raw(arr: [u8; 32]) -> Self {
        ClipboardKey(Zeroizing::new(arr))
    }

    /// 原始 32B 字节引用。
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 加密，返回 `v1:<base64(nonce[12B]||ct||tag[16B])>`。
    ///
    /// nonce 每次 OS 熵源随机生成（不复用），AES-GCM 自带 16B 认证 tag。
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<String> {
        let cipher = Aes256Gcm::new_from_slice(self.as_bytes())
            .context("AES-256-GCM key 长度需为 32 字节")?;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .context("AES-256-GCM 加密失败")?;
        let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);
        Ok(format!(
            "{}{}",
            CIPHERTEXT_PREFIX,
            data_encoding::BASE64.encode(&combined)
        ))
    }

    /// 解密 `v1:` 前缀密文。返 `Zeroizing<Vec<u8>>`（明文离开作用域自动清零）。
    pub fn decrypt(&self, ciphertext: &str) -> Result<Zeroizing<Vec<u8>>> {
        let ct_str = ciphertext
            .strip_prefix(CIPHERTEXT_PREFIX)
            .context("密文格式不符（缺 v1: 前缀）")?;
        let combined = data_encoding::BASE64
            .decode(ct_str.as_bytes())
            .context("Base64 解码失败")?;
        ensure!(
            combined.len() > NONCE_LEN,
            "密文长度不足（缺 nonce）：{} bytes",
            combined.len()
        );
        let (nonce_bytes, ct) = combined.split_at(NONCE_LEN);
        let cipher = Aes256Gcm::new_from_slice(self.as_bytes())
            .context("AES-256-GCM key 长度需为 32 字节")?;
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ct)
            .context("AES-256-GCM 解密失败：密文可能已损坏或 key 不匹配")?;
        Ok(Zeroizing::new(plaintext))
    }
}

// === clipboard.key 管理（Task 4）===

/// 用 OS 熵源生成随机字节（CSPRNG）——内联避免依赖 vault::crypto::util。
fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf
}

/// 加载或创建 clipboard.key（32B AES-256-GCM key，hex 编码存文件）。
///
/// 文件不存在时用 OS 熵源生成 32B 随机，hex 编码写入文件（unix 权限 0600，先防君子）。
/// 文件存在时读取 hex 解码为 32B——若长度/格式不符返回错误（拒绝降级使用损坏 key）。
///
/// **安全模型**：本文件保护的是「同步 git repo 被他人 clone 后看明文收藏」的场景
/// （git remote 通常是私有库，但本地 `.sync` 可能被备份/同步软件外泄）。0600 权限
/// 只防本机其他用户读，不防本机 root/具备用户权限的进程——这是「先防君子」的折中。
/// 真正的端到端加密仍由 vault 的主密码体系承担（clipboard.key 不跨设备派生，
/// 而是每设备独立生成 → 跨设备需重新加密传播，Task 6+ 处理）。
pub fn load_or_create_clipboard_key() -> Result<ClipboardKey> {
    let path = clipboard_key_path()?;
    if let Ok(hex_str) = std::fs::read_to_string(&path) {
        let bytes =
            hex::decode(hex_str.trim()).context("clipboard.key hex decode 失败（文件损坏）")?;
        anyhow::ensure!(
            bytes.len() == 32,
            "clipboard.key 必须 32 字节，实际 {}",
            bytes.len()
        );
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        return Ok(ClipboardKey::from_raw(arr));
    }
    // 生成新 key（OS 熵源 CSPRNG）
    let key_bytes = random_bytes(32);
    let hex_str = hex::encode(&key_bytes);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建 clipboard.key 父目录失败：{}", parent.display()))?;
    }
    std::fs::write(&path, &hex_str)
        .with_context(|| format!("写 clipboard.key 失败：{}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("设置 clipboard.key 权限 0600 失败：{}", path.display()))?;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&key_bytes);
    Ok(ClipboardKey::from_raw(arr))
}

// === 文件格式（Task 5）===

/// 剪贴板 outline——收藏增量索引（uuid → {md5, updatedMs}）。
///
/// 与 vault `Outline` / hotword `HotwordOutline` 对称——BTreeMap 保序列化顺序稳定，
/// 避免相同输入产生不同 JSON（git 误判变化产生空 commit）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardOutline {
    /// outline 格式版本（当前 1）。
    #[serde(default = "default_outline_version")]
    pub version: u32,
    /// 收藏 entries：favorite_uuid → {md5, updatedMs}。
    #[serde(default)]
    pub favorites: BTreeMap<String, OutlineEntry>,
}

impl Default for ClipboardOutline {
    fn default() -> Self {
        Self {
            version: default_outline_version(),
            favorites: BTreeMap::new(),
        }
    }
}

fn default_outline_version() -> u32 {
    1
}

/// 单个收藏文件（加密后存盘）——`favorites/<2hex>/<uuid>.json`。
///
/// `encrypted_payload` 是 `FavoritePayload` 经 clipboard.key 加密后的 `v1:<base64>` 字符串。
/// 其余字段（id / is_deleted / 时间戳）明文——outline diff + 软删判断需读明文，不解密。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardFavoriteFile {
    /// 文件格式版本（当前 1）。
    pub version: u32,
    /// 收藏 UUID（v4 字符串，clipboard_history.favorite_id）。
    pub id: String,
    /// 软删标记：0=活跃，>0=删除时刻 epoch 秒（tombstone）。与 hotword 语义一致。
    #[serde(default)]
    pub is_deleted: i64,
    /// 加密后的 FavoritePayload（`v1:<base64(nonce||ct||tag)>`）。
    pub encrypted_payload: String,
    /// 创建时间（SQLite datetime 格式）。
    pub created_at: String,
    /// 更新时间。
    pub updated_at: String,
}

/// 收藏 payload——被 clipboard.key 加密后存入 `ClipboardFavoriteFile.encrypted_payload`。
///
/// 内嵌一份 `HistoryRowJson` 快照（不是引用历史行 id），加密后跨设备自包含——B 机无需
/// 先同步历史即可还原收藏。`favorite_id` 重复存一份（解密后用于校验文件 ↔ payload 对应）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoritePayload {
    /// 被收藏的历史行快照。
    pub history_row: HistoryRowJson,
    /// 收藏 UUID（与外层 ClipboardFavoriteFile.id 一致，解密后用于校验）。
    pub favorite_id: String,
    /// 内容指纹（md5/sha 等，用于去重或一致性校验）。
    pub content_hash: String,
}

/// 历史行 JSON 快照——clipboard_history row 的可序列化镜像。
///
/// 字段名对齐 SQLite schema（camelCase 后 `item_type` / `ref_data` / `meta_info` / `is_rich`）。
/// 所有可选字段允许 null（与 SQLite schema 一致——某些字段对新行尚未填）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRowJson {
    /// 历史行 UUID v4（与 favorite 引用的 history row id 一致）。
    pub id: String,
    /// 行类型（text/image/files/...）。
    pub item_type: String,
    /// 文本内容（或图片/文件路径，依 item_type）。
    pub content: String,
    /// 引用数据（image base64 / files JSON 等），可空。
    pub ref_data: Option<String>,
    /// 元信息 JSON（源 app/格式等），可空。
    pub meta_info: Option<String>,
    /// 是否富文本（HTML/RTF 等带格式）。
    #[serde(default)]
    pub is_rich: bool,
    /// 创建时间（SQLite datetime 格式）。
    pub created_at: String,
    /// 分段信息（长文本切分等），可空。
    pub segments: Option<String>,
}

// === outline 读写 ===

/// 读 clipboard outline.json。文件不存在时返回默认空 outline（首次同步）。
pub fn read_clipboard_outline() -> Result<ClipboardOutline> {
    let path = outline_path()?;
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let outline: ClipboardOutline = serde_json::from_str(&content)
                .with_context(|| format!("clipboard outline.json 解析失败：{}", path.display()))?;
            Ok(outline)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ClipboardOutline::default()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("读 clipboard outline.json 失败：{}", path.display()))),
    }
}

/// 写 clipboard outline.json（原子写，pretty print）。
pub fn write_clipboard_outline(outline: &ClipboardOutline) -> Result<()> {
    let path = outline_path()?;
    let json = serde_json::to_string_pretty(outline).context("序列化 clipboard outline 失败")?;
    write_atomically(&path, &format!("{}\n", json))
}

// === favorite 文件读写 ===

/// 读单个收藏文件：`favorites/<2hex>/<uuid>.json`。
pub fn read_favorite_file(uuid: &str) -> Result<ClipboardFavoriteFile> {
    let path = favorite_file_path(uuid)?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读收藏文件失败：{}", path.display()))?;
    let file: ClipboardFavoriteFile =
        serde_json::from_str(&content).context("收藏文件 JSON 解析失败")?;
    Ok(file)
}

/// 写单个收藏文件（原子写，含分片目录创建）。
pub fn write_favorite_file(file: &ClipboardFavoriteFile) -> Result<()> {
    let path = favorite_file_path(&file.id)?;
    let json = serde_json::to_string_pretty(file).context("序列化收藏文件失败")?;
    write_atomically(&path, &format!("{}\n", json))
}

/// 删单个收藏文件（文件不存在时返 Ok——幂等，best-effort）。
pub fn delete_favorite_file(uuid: &str) -> Result<()> {
    let path = favorite_file_path(uuid)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("删收藏文件失败：{}", path.display()))),
    }
}

// === Task 6: 全量导出（DB → .sync 文件）===

/// 收藏的身份 md5——只含 `id | history_id`（身份标识），不含 is_deleted / sync_md5 / 时间戳。
///
/// 与 hotword_set_md5 同语义——md5 是「这个 favorite 是谁」的指纹，用于 outline diff 判断
/// 新增/重指向（history_id 改了）。状态变化（is_deleted / content 改动）靠时间戳比较决定方向，
/// 不触发 md5 diff（否则软删后 md5 变了但 outline 没同步更新 → 不必要的 push/pull）。
pub fn favorite_md5(id: &str, history_id: &str) -> String {
    md5_hex(format!("{}|{}", id, history_id).as_bytes())
}

/// 内容指纹——md5 of content bytes。存入 `FavoritePayload.content_hash`。
///
/// 跨设备一致性校验用：A 机写入「内容 X」，B 机 pull 后算 md5 应等于 content_hash。
/// 不进 outline（outline.md5 是身份指纹 `id|history_id`）——content_hash 在加密 payload 内。
fn content_hash(content: &str) -> String {
    md5_hex(content.as_bytes())
}

/// 从 DB 全量导出到 `.sync/clipboard/`——DB 是单一真相源，文件系统重建。
///
/// 流程：
/// 1. 加载 clipboard key
/// 2. `list_all_favorites`（含 tombstone）
/// 3. 清空 favorites/ 目录后重建
/// 4. 每个 favorite：JOIN clipboard_history 取行 → 构建 FavoritePayload → 加密 → 写文件 → 进 outline
/// 5. 写 outline.json
///
/// 与 `hotword::export_all_hotwords` 对称——merge 末尾调本函数重建所有文件（DB 是真相源）。
pub fn export_all_favorites() -> Result<ClipboardOutline> {
    let key = load_or_create_clipboard_key()?;
    let favs = octopus_infra::db::list_all_favorites()?; // 含 tombstone

    // 确保 clipboard/ 目录存在
    let clip_dir = clipboard_dir()?;
    std::fs::create_dir_all(&clip_dir)
        .with_context(|| format!("创建 clipboard 目录失败：{}", clip_dir.display()))?;

    // 清空 favorites/ 子目录（含分片子目录），保留 clipboard.key + outline.json
    let fav_dir = favorites_dir()?;
    if fav_dir.is_dir() {
        std::fs::remove_dir_all(&fav_dir)
            .with_context(|| format!("清空 favorites/ 失败：{}", fav_dir.display()))?;
    }
    std::fs::create_dir_all(&fav_dir)
        .with_context(|| format!("重建 favorites/ 失败：{}", fav_dir.display()))?;

    // 每个 favorite 写文件 + 进 outline
    let mut entries: BTreeMap<String, OutlineEntry> = BTreeMap::new();
    for fav in &favs {
        // JOIN clipboard_history——拉不到行也写文件（payload 含 history_row 快照可能空）
        let row = octopus_infra::db::load_clipboard_history_row(&fav.history_id)
            .with_context(|| format!("读历史行失败 history_id={}", fav.history_id))?;

        let history_row = match row {
            Some(r) => HistoryRowJson {
                id: r.id,
                item_type: r.item_type,
                content: r.content,
                ref_data: r.ref_data,
                meta_info: r.meta_info,
                is_rich: r.is_rich,
                created_at: r.created_at,
                segments: r.segments,
            },
            None => {
                // history 行已不在 DB（用户清了历史但 favorite tombstone 还在）。
                // 用最小占位行写文件——favorite 的 tombstone 仍需传播删除意图。
                log::warn!(
                    "[sync] 收藏 {} 引用的历史行 {} 在 DB 中找不到——写占位 payload",
                    fav.id,
                    fav.history_id
                );
                HistoryRowJson {
                    id: fav.history_id.clone(),
                    item_type: "text".into(),
                    content: String::new(),
                    ref_data: None,
                    meta_info: None,
                    is_rich: false,
                    created_at: fav.created_at.clone(),
                    segments: None,
                }
            }
        };

        let content_hash_val = content_hash(&history_row.content);
        let payload = FavoritePayload {
            history_row,
            favorite_id: fav.id.clone(),
            content_hash: content_hash_val,
        };
        let payload_json = serde_json::to_string(&payload).context("序列化 FavoritePayload 失败")?;
        let encrypted = key
            .encrypt(payload_json.as_bytes())
            .context("加密 FavoritePayload 失败")?;

        let file = ClipboardFavoriteFile {
            version: 1,
            id: fav.id.clone(),
            is_deleted: fav.is_deleted,
            encrypted_payload: encrypted,
            created_at: fav.created_at.clone(),
            updated_at: fav.updated_at.clone(),
        };
        write_favorite_file(&file)?;

        entries.insert(
            fav.id.clone(),
            OutlineEntry {
                md5: favorite_md5(&fav.id, &fav.history_id),
                updated_ms: iso_to_unix_ms(&fav.updated_at),
            },
        );
    }

    let outline = ClipboardOutline {
        version: 1,
        favorites: entries,
    };
    write_clipboard_outline(&outline)?;
    Ok(outline)
}

// === Task 7: 双向 merge（DB ↔ .sync 文件）===

/// merge 报告——对称 `hotword::HotwordMergeReport`（独立于 vault crate，sync 不能依赖 vault）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardMergeReport {
    /// 远程 → DB（远程 updated_at 更新 / DB 无 / 远程 tombstone 单向优先）
    pub pulled: usize,
    /// DB → 远程（DB updated_at 更新 / outline 无 / 冲突 DB 赢）
    pub pushed: usize,
    /// updated_at 相等 + md5 不等的冲突数（DB 赢）
    pub conflicts: usize,
    /// 文件读取失败等跳过数
    pub skipped: usize,
}

/// 3-way merge：DB ↔ 文件系统按 `updated_at` 最新赢，相等 md5 比对 DB 赢。
///
/// 对称 `hotword::merge_hotwords`，含 tombstone 单向优先 fix（对称 set/word 级修复，
/// 防本地 active 写回文件覆盖远程 tombstone → 第三台机器复活）。
///
/// 流程：
/// 1. 遍历 remote outline 的 favorite：DB 无 → pull；DB 有 → tombstone 优先 / 时间戳比较 / md5 冲突
/// 2. DB 有 + outline 无 → push
/// 3. 末尾 `export_all_favorites` 从 DB 最新状态重建所有文件 + outline
pub fn merge_clipboard_favorites() -> Result<ClipboardMergeReport> {
    let remote_outline = read_clipboard_outline()?;
    let db_favs = octopus_infra::db::list_all_favorites()?; // 含 tombstone
    let db_by_id: std::collections::HashMap<&str, &octopus_infra::db::ClipboardFavorite> = db_favs
        .iter()
        .map(|f| (f.id.as_str(), f))
        .collect();
    let mut report = ClipboardMergeReport::default();
    let key = load_or_create_clipboard_key()?;

    // === 阶段 1：outline 有 → 3-way 判定 ===
    for (uuid, entry) in &remote_outline.favorites {
        let remote_updated = entry.updated_ms;
        match db_by_id.get(uuid.as_str()) {
            None => {
                // DB 无 → pull
                match pull_favorite(uuid, &key) {
                    Ok(true) => report.pulled += 1,
                    Ok(false) => report.skipped += 1,
                    Err(e) => {
                        log::warn!("[sync] 收藏 merge: {} 文件读取失败，已跳过：{}", uuid, e);
                        report.skipped += 1;
                    }
                }
            }
            Some(db_f) => {
                // 🔴 tombstone 单向优先 fix（对称 hotword set/word 修复）：删除是单向终态——
                // 远程已 tombstone（is_deleted>0）时，无论本地时间戳多新都应 pull tombstone 到 DB，
                // 不把本地 active 写回文件覆盖远程 tombstone（否则第三台机器当新收藏复活）。
                let remote_is_tombstone = read_favorite_file(uuid)
                    .map(|f| f.is_deleted > 0)
                    .unwrap_or(false);
                let local_updated = iso_to_unix_ms(&db_f.updated_at);
                if remote_is_tombstone {
                    match pull_favorite(uuid, &key) {
                        Ok(true) => report.pulled += 1,
                        Ok(false) => report.skipped += 1,
                        Err(e) => {
                            log::warn!(
                                "[sync] 收藏 merge: {} tombstone pull 跳过：{}",
                                uuid, e
                            );
                            report.skipped += 1;
                        }
                    }
                } else if remote_updated > local_updated {
                    // 远程更新 → pull 覆盖 DB
                    match pull_favorite(uuid, &key) {
                        Ok(true) => report.pulled += 1,
                        Ok(false) => report.skipped += 1,
                        Err(e) => {
                            log::warn!("[sync] 收藏 merge: {} pull 跳过：{}", uuid, e);
                            report.skipped += 1;
                        }
                    }
                } else if local_updated > remote_updated {
                    // DB 更新 → push 覆盖文件
                    match push_favorite(db_f, &key) {
                        Ok(()) => report.pushed += 1,
                        Err(e) => {
                            log::warn!("[sync] 收藏 merge: {} push 跳过：{}", uuid, e);
                            report.skipped += 1;
                        }
                    }
                } else {
                    // 时间戳相等 → md5 比对，冲突 DB 赢
                    let db_md5 = favorite_md5(&db_f.id, &db_f.history_id);
                    if db_md5 != entry.md5 {
                        match push_favorite(db_f, &key) {
                            Ok(()) => {
                                report.pushed += 1;
                                report.conflicts += 1;
                            }
                            Err(e) => {
                                log::warn!("[sync] 收藏 merge: {} 冲突 push 跳过：{}", uuid, e);
                                report.skipped += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    // === 阶段 2：DB 有 + outline 无 → push ===
    for db_f in &db_favs {
        if !remote_outline.favorites.contains_key(&db_f.id) {
            match push_favorite(db_f, &key) {
                Ok(()) => report.pushed += 1,
                Err(e) => {
                    log::warn!("[sync] 收藏 merge: DB only {} push 跳过：{}", db_f.id, e);
                    report.skipped += 1;
                }
            }
        }
    }

    // === 阶段 3：从 DB 最新状态重建所有文件 + outline ===
    export_all_favorites()?;

    log::info!(
        "[sync] merge_clipboard_favorites 完成：pulled={} pushed={} conflicts={} skipped={}",
        report.pulled,
        report.pushed,
        report.conflicts,
        report.skipped
    );
    Ok(report)
}

/// Pull 单个收藏（文件 → DB）。返回 Ok(true) 成功 upsert，Ok(false) 跳过。
///
/// tombstone（is_deleted>0）：
/// - DB 有且 active → soft_delete_favorite + history.is_favorite=0
/// - DB 无 → 解密 payload，UPSERT history 行 + UPSERT favorite（is_deleted>0） + history.is_favorite=0
///
/// active（is_deleted==0）：
/// - 解密 payload 取 HistoryRowJson
/// - UPSERT history 行
/// - UPSERT favorite（is_deleted=0）
/// - history.is_favorite=1
fn pull_favorite(uuid: &str, key: &ClipboardKey) -> Result<bool> {
    let file = read_favorite_file(uuid)?;

    if file.is_deleted > 0 {
        // tombstone
        let existing = octopus_infra::db::load_favorite(uuid)?;
        match existing {
            Some(fav) if fav.is_deleted == 0 => {
                // DB active → 软删
                octopus_infra::db::soft_delete_favorite(uuid, file.is_deleted)?;
                octopus_infra::db::set_clipboard_is_favorite(&fav.history_id, false)?;
                log::debug!("[sync] 收藏 pull: {} DB active → 软删", uuid);
                Ok(true)
            }
            None => {
                // DB 无 → 解密 payload 还原行 + tombstone favorite
                let payload = decrypt_payload(key, &file.encrypted_payload, uuid)?;
                upsert_history_from_payload(&payload)?;
                let mut fav = octopus_infra::db::ClipboardFavorite {
                    id: payload.favorite_id.clone(),
                    history_id: payload.history_row.id.clone(),
                    is_deleted: file.is_deleted,
                    created_at: file.created_at.clone(),
                    updated_at: file.updated_at.clone(),
                    sync_md5: Some(favorite_md5(&payload.favorite_id, &payload.history_row.id)),
                };
                let _ = &mut fav; // 占位，未来扩展
                octopus_infra::db::upsert_favorite_sync(&fav)?;
                octopus_infra::db::set_clipboard_is_favorite(&payload.history_row.id, false)?;
                log::debug!("[sync] 收藏 pull: {} DB 无 → 还原 tombstone", uuid);
                Ok(true)
            }
            Some(_) => {
                // DB 已是 tombstone——已是终态，跳过
                log::debug!("[sync] 收藏 pull: {} DB 已 tombstone，skip", uuid);
                Ok(false)
            }
        }
    } else {
        // active
        let payload = decrypt_payload(key, &file.encrypted_payload, uuid)?;
        upsert_history_from_payload(&payload)?;
        let fav = octopus_infra::db::ClipboardFavorite {
            id: payload.favorite_id.clone(),
            history_id: payload.history_row.id.clone(),
            is_deleted: 0,
            created_at: file.created_at.clone(),
            updated_at: file.updated_at.clone(),
            sync_md5: Some(favorite_md5(&payload.favorite_id, &payload.history_row.id)),
        };
        octopus_infra::db::upsert_favorite_sync(&fav)?;
        octopus_infra::db::set_clipboard_is_favorite(&payload.history_row.id, true)?;
        log::debug!("[sync] 收藏 pull: {} active → 还原", uuid);
        Ok(true)
    }
}

/// 解密 payload 并校验 favorite_id 一致（防文件 ↔ payload 错配）。
fn decrypt_payload(
    key: &ClipboardKey,
    encrypted: &str,
    expected_fav_id: &str,
) -> Result<FavoritePayload> {
    let plaintext = key.decrypt(encrypted)?;
    let payload: FavoritePayload =
        serde_json::from_slice(&plaintext).context("解密后 payload JSON 解析失败")?;
    if payload.favorite_id != expected_fav_id {
        anyhow::bail!(
            "payload.favorite_id ({}) 与文件 id ({}) 不符——拒绝错配",
            payload.favorite_id,
            expected_fav_id
        );
    }
    Ok(payload)
}

/// 把 payload 内的 history_row UPSERT 进 DB。
fn upsert_history_from_payload(payload: &FavoritePayload) -> Result<()> {
    let r = &payload.history_row;
    octopus_infra::db::upsert_clipboard_history_sync(
        &r.id,
        &r.item_type,
        &r.content,
        r.ref_data.as_deref(),
        r.meta_info.as_deref(),
        r.is_rich,
        &r.created_at,
        r.segments.as_deref(),
    )
}

/// Push 单个收藏（DB → 文件）——读 history 行构建 payload 加密写文件。
fn push_favorite(
    fav: &octopus_infra::db::ClipboardFavorite,
    key: &ClipboardKey,
) -> Result<()> {
    let row = octopus_infra::db::load_clipboard_history_row(&fav.history_id)?;
    let history_row = match row {
        Some(r) => HistoryRowJson {
            id: r.id,
            item_type: r.item_type,
            content: r.content,
            ref_data: r.ref_data,
            meta_info: r.meta_info,
            is_rich: r.is_rich,
            created_at: r.created_at,
            segments: r.segments,
        },
        None => {
            log::warn!(
                "[sync] push 收藏 {}: history 行 {} 不存在，写占位 payload",
                fav.id,
                fav.history_id
            );
            HistoryRowJson {
                id: fav.history_id.clone(),
                item_type: "text".into(),
                content: String::new(),
                ref_data: None,
                meta_info: None,
                is_rich: false,
                created_at: fav.created_at.clone(),
                segments: None,
            }
        }
    };

    let payload = FavoritePayload {
        history_row,
        favorite_id: fav.id.clone(),
        content_hash: content_hash(&fav.history_id),
    };
    let payload_json = serde_json::to_string(&payload)?;
    let encrypted = key.encrypt(payload_json.as_bytes())?;
    let file = ClipboardFavoriteFile {
        version: 1,
        id: fav.id.clone(),
        is_deleted: fav.is_deleted,
        encrypted_payload: encrypted,
        created_at: fav.created_at.clone(),
        updated_at: fav.updated_at.clone(),
    };
    write_favorite_file(&file)
}

// === 测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// RAII guard：测试期间 set_test_sync_root，drop 时 clear（与 hotword 测试同模式）。
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

    // === Task 4: 加解密测试 ===

    /// ClipboardKey encrypt/decrypt roundtrip——v1: 前缀 + 往返一致。
    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = ClipboardKey::from_raw([42u8; 32]);
        let plaintext = r#"{"content":"hello"}"#;
        let encrypted = key.encrypt(plaintext.as_bytes()).unwrap();
        assert!(encrypted.starts_with("v1:"), "密文应以 v1: 前缀开头");
        let decrypted = key.decrypt(&encrypted).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&decrypted),
            plaintext,
            "解密结果应与原文一致"
        );
    }

    /// nonce 不复用——同 key 同明文加密两次密文不同（但都能解出来）。
    #[test]
    fn encrypt_nonce_is_unique() {
        let key = ClipboardKey::from_raw([7u8; 32]);
        let c1 = key.encrypt(b"same").unwrap();
        let c2 = key.encrypt(b"same").unwrap();
        assert_ne!(c1, c2, "nonce 随机 → 同明文密文应不同");
        assert_eq!(&*key.decrypt(&c1).unwrap(), b"same");
        assert_eq!(&*key.decrypt(&c2).unwrap(), b"same");
    }

    /// 错误 key 解密失败（AES-GCM tag 校验不过）。
    #[test]
    fn decrypt_wrong_key_fails() {
        let k1 = ClipboardKey::from_raw([1u8; 32]);
        let k2 = ClipboardKey::from_raw([2u8; 32]);
        let ct = k1.encrypt(b"secret").unwrap();
        assert!(k2.decrypt(&ct).is_err(), "错误 key 解密应失败");
    }

    /// clipboard.key 首次创建：生成 32B hex 文件 + 0600 权限 + 返回有效 ClipboardKey。
    #[test]
    fn clipboard_key_creates_when_missing() {
        let _g = SyncRootGuard::new();
        let key = load_or_create_clipboard_key().expect("首次创建 key");
        // 文件应存在 + 是 64 字符 hex（32B）
        let path = clipboard_key_path().unwrap();
        let hex_str = std::fs::read_to_string(&path).unwrap();
        assert_eq!(hex_str.trim().len(), 64, "32B hex 应是 64 字符");
        assert!(
            hex_str.trim().chars().all(|c| c.is_ascii_hexdigit()),
            "应全是 hex 字符"
        );

        // 返回的 key 能加解密
        let ct = key.encrypt(b"test").unwrap();
        assert_eq!(&*key.decrypt(&ct).unwrap(), b"test");

        // unix 权限 0600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "clipboard.key 权限应是 0600，实际 {:o}",
                mode
            );
        }
    }

    /// clipboard.key 二次加载：复用已存在文件（hex 一致），不重新生成。
    #[test]
    fn clipboard_key_reuses_existing() {
        let _g = SyncRootGuard::new();
        let key1 = load_or_create_clipboard_key().unwrap();
        let key2 = load_or_create_clipboard_key().unwrap();
        // 两次应是同一 key（文件复用，不重新生成）
        assert_eq!(key1.as_bytes(), key2.as_bytes(), "二次加载应复用同一 key");
    }

    /// clipboard.key 损坏（长度不符）：返回错误，不降级。
    #[test]
    fn clipboard_key_rejects_corrupt_length() {
        let _g = SyncRootGuard::new();
        let path = clipboard_key_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "abcd").unwrap(); // 2B hex，非 32B
        let err = load_or_create_clipboard_key().unwrap_err();
        assert!(
            err.to_string().contains("32"),
            "错误信息应提及长度 32：{}",
            err
        );
    }

    // === Task 5: 文件读写测试 ===

    #[test]
    fn clipboard_outline_round_trip() {
        let _g = SyncRootGuard::new();
        let outline = ClipboardOutline {
            version: 1,
            favorites: BTreeMap::from([(
                "a1b2c3d4-e5f6-4789-8901-abcdef123456".into(),
                OutlineEntry {
                    md5: "md5a".into(),
                    updated_ms: 1000,
                },
            )]),
        };
        write_clipboard_outline(&outline).expect("write");
        let loaded = read_clipboard_outline().expect("read");
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.favorites.len(), 1);
        assert_eq!(loaded.favorites["a1b2c3d4-e5f6-4789-8901-abcdef123456"].md5, "md5a");
        assert_eq!(
            loaded.favorites["a1b2c3d4-e5f6-4789-8901-abcdef123456"].updated_ms,
            1000
        );
    }

    #[test]
    fn read_clipboard_outline_missing_returns_default() {
        let _g = SyncRootGuard::new();
        let outline = read_clipboard_outline().expect("应返默认空 outline");
        assert_eq!(outline.version, 1);
        assert!(outline.favorites.is_empty());
    }

    fn sample_favorite_file(uuid: &str) -> ClipboardFavoriteFile {
        ClipboardFavoriteFile {
            version: 1,
            id: uuid.into(),
            is_deleted: 0,
            encrypted_payload: "v1:dummybase64".into(),
            created_at: "2026-08-03 10:00:00".into(),
            updated_at: "2026-08-03 10:00:00".into(),
        }
    }

    #[test]
    fn favorite_file_round_trip() {
        let _g = SyncRootGuard::new();
        let uuid = "a1b2c3d4-e5f6-4789-8901-abcdef123456";
        let file = sample_favorite_file(uuid);
        write_favorite_file(&file).expect("write");

        let loaded = read_favorite_file(uuid).expect("read");
        assert_eq!(loaded.id, uuid);
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.encrypted_payload, "v1:dummybase64");
        assert_eq!(loaded.is_deleted, 0);
    }

    /// 收藏文件按 <2hex> 分片：uuid 前 2 hex 字符作子目录。
    #[test]
    fn favorite_file_is_sharded_by_first_2_hex() {
        let _g = SyncRootGuard::new();
        let uuid = "a1b2c3d4-e5f6-4789-8901-abcdef123456";
        let file = sample_favorite_file(uuid);
        write_favorite_file(&file).expect("write");

        // shard_dir("a1b2c3d4-...") = "a1"（filter hex 取前 2）
        let shard_root = favorites_dir().unwrap().join("a1");
        let file_path = shard_root.join(format!("{uuid}.json"));
        assert!(file_path.exists(), "收藏文件应在 <2hex>/<uuid>.json 路径下");
    }

    #[test]
    fn delete_favorite_file_is_idempotent() {
        let _g = SyncRootGuard::new();
        let uuid = "a1b2c3d4-e5f6-4789-8901-abcdef123456";
        delete_favorite_file(uuid).expect("删不存在的文件应 Ok（幂等）");
    }

    #[test]
    fn delete_favorite_file_removes_existing() {
        let _g = SyncRootGuard::new();
        let uuid = "a1b2c3d4-e5f6-4789-8901-abcdef123456";
        write_favorite_file(&sample_favorite_file(uuid)).expect("write");
        assert!(read_favorite_file(uuid).is_ok());
        delete_favorite_file(uuid).expect("delete");
        assert!(read_favorite_file(uuid).is_err(), "删后读取应失败");
    }

    // === FavoritePayload / HistoryRowJson 序列化 ===

    #[test]
    fn favorite_payload_serializes_camel_case() {
        let payload = FavoritePayload {
            history_row: HistoryRowJson {
                id: "hist-uuid".into(),
                item_type: "text".into(),
                content: "hello".into(),
                ref_data: None,
                meta_info: None,
                is_rich: false,
                created_at: "2026-08-03 10:00:00".into(),
                segments: None,
            },
            favorite_id: "fav-uuid".into(),
            content_hash: "md5hash".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        // camelCase 字段名
        assert!(json.contains("\"historyRow\""), "historyRow 应 camelCase");
        assert!(json.contains("\"favoriteId\""), "favoriteId 应 camelCase");
        assert!(json.contains("\"contentHash\""), "contentHash 应 camelCase");
        assert!(json.contains("\"itemType\""), "itemType 应 camelCase");
        assert!(json.contains("\"isRich\""), "isRich 应 camelCase");
        assert!(json.contains("\"metaInfo\""), "metaInfo 应 camelCase");
        assert!(json.contains("\"refData\""), "refData 应 camelCase");
        // round-trip
        let parsed: FavoritePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.favorite_id, "fav-uuid");
        assert_eq!(parsed.history_row.id, "hist-uuid");
    }

    /// 端到端：payload 加密 → 写文件 → 读文件 → 解密 → 还原 payload。
    #[test]
    fn encrypt_write_read_decrypt_roundtrip() {
        let _g = SyncRootGuard::new();
        let key = load_or_create_clipboard_key().unwrap();

        let payload = FavoritePayload {
            history_row: HistoryRowJson {
                id: "hist-uuid".into(),
                item_type: "text".into(),
                content: "剪贴板收藏内容".into(),
                ref_data: Some(r#"{"src":"safari"}"#.into()),
                meta_info: None,
                is_rich: true,
                created_at: "2026-08-03 10:00:00".into(),
                segments: None,
            },
            favorite_id: "a1b2c3d4-e5f6-4789-8901-abcdef123456".into(),
            content_hash: "md5abc".into(),
        };
        let payload_json = serde_json::to_string(&payload).unwrap();
        let encrypted = key.encrypt(payload_json.as_bytes()).unwrap();

        let file = ClipboardFavoriteFile {
            version: 1,
            id: payload.favorite_id.clone(),
            is_deleted: 0,
            encrypted_payload: encrypted,
            created_at: "2026-08-03 10:00:00".into(),
            updated_at: "2026-08-03 10:00:00".into(),
        };
        write_favorite_file(&file).unwrap();

        let loaded = read_favorite_file(&file.id).unwrap();
        let decrypted = key.decrypt(&loaded.encrypted_payload).unwrap();
        let restored: FavoritePayload = serde_json::from_slice(&decrypted).unwrap();
        assert_eq!(restored.favorite_id, payload.favorite_id);
        assert_eq!(restored.history_row.content, "剪贴板收藏内容");
        assert!(restored.history_row.is_rich);
    }

    // === Task 7: merge_clipboard_favorites 测试 ===
    //
    // 复用 hotword 测试模式——DB + sync_root 双隔离 guard，DB 用 in-memory SQLite。

    use octopus_infra::db;

    /// DB + sync_root 双隔离 guard——merge 测试用（对称 hotword DbSyncGuard）。
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

    /// 测试辅助：在 DB 插入一个 clipboard_history 行（item_type='text'）。
    fn insert_history_row(id: &str, content: &str) {
        db::upsert_clipboard_history_sync(
            id,
            "text",
            content,
            None,
            None,
            false,
            "2026-08-03 10:00:00",
            None,
        )
        .expect("insert_history_row");
    }

    /// 测试辅助：手写一份远程收藏文件（加密 + 写盘）+ outline entry，模拟「远程仓库」状态。
    /// `is_deleted` = 0 → active；>0 → tombstone（删除时刻 epoch 秒）。
    fn write_remote_favorite(
        fav_id: &str,
        history_id: &str,
        content: &str,
        is_deleted: i64,
        updated_ms: i64,
    ) {
        let key = load_or_create_clipboard_key().expect("key");
        let history_row = HistoryRowJson {
            id: history_id.into(),
            item_type: "text".into(),
            content: content.into(),
            ref_data: None,
            meta_info: None,
            is_rich: false,
            created_at: "2026-08-03 10:00:00".into(),
            segments: None,
        };
        let payload = FavoritePayload {
            history_row,
            favorite_id: fav_id.into(),
            content_hash: md5_hex(content.as_bytes()),
        };
        let payload_json = serde_json::to_string(&payload).unwrap();
        let encrypted = key.encrypt(payload_json.as_bytes()).unwrap();
        let file = ClipboardFavoriteFile {
            version: 1,
            id: fav_id.into(),
            is_deleted,
            encrypted_payload: encrypted,
            created_at: "2026-08-03 10:00:00".into(),
            updated_at: "2026-08-03 10:00:00".into(),
        };
        write_favorite_file(&file).unwrap();
        // outline entry
        let mut outline = read_clipboard_outline().unwrap_or_default();
        outline.favorites.insert(
            fav_id.into(),
            OutlineEntry {
                md5: favorite_md5(fav_id, history_id),
                updated_ms,
            },
        );
        write_clipboard_outline(&outline).unwrap();
    }

    /// 测试 1：远程有收藏、DB 空 → merge 后 DB 拉到 favorite + history 行。
    #[test]
    fn merge_pulls_remote_favorite_to_empty_db() {
        let _g = DbSyncGuard::new();
        // 确保 DB 无任何 favorite
        assert!(db::list_all_favorites().unwrap().is_empty());

        let fav_id = "pull-aaaa-0001";
        let history_id = "hist-aaaa-0001";
        write_remote_favorite(fav_id, history_id, "远程文本", 0, 9999999999999);

        let report = merge_clipboard_favorites().expect("merge");
        assert_eq!(report.pulled, 1, "应拉取 1 条远程收藏");

        // DB 应有该 favorite（active）
        let fav = db::load_favorite(fav_id).unwrap().expect("favorite 应在 DB");
        assert_eq!(fav.history_id, history_id);
        assert_eq!(fav.is_deleted, 0, "应为 active");

        // history 行也应被还原
        let row = db::load_clipboard_history_row(history_id)
            .unwrap()
            .expect("history 行应被 pull 还原");
        assert_eq!(row.content, "远程文本");

        // history.is_favorite 应 = 1（active favorite 还原后置位）
        // 直接查 DB 验证
        let is_fav: i64 = db::with_db(|conn| {
            conn.query_row(
                "SELECT is_favorite FROM clipboard_history WHERE id = ?1",
                rusqlite::params![history_id],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap();
        assert_eq!(is_fav, 1, "history.is_favorite 应 = 1");
    }

    /// 测试 2：DB 有收藏、outline 空（文件未写） → merge 后文件被写。
    #[test]
    fn merge_pushes_local_only_favorite() {
        let _g = DbSyncGuard::new();
        // 确保 outline 为空
        write_clipboard_outline(&ClipboardOutline::default()).unwrap();

        let history_id = "hist-bbbb-0001";
        insert_history_row(history_id, "本地文本");
        db::insert_favorite("push-bbbb-0001", history_id).expect("insert_favorite");

        let report = merge_clipboard_favorites().expect("merge");
        assert!(report.pushed >= 1, "DB only favorite 应 push 到文件");

        // 文件应存在
        let file = read_favorite_file("push-bbbb-0001").expect("文件应被写");
        assert_eq!(file.id, "push-bbbb-0001");
        assert_eq!(file.is_deleted, 0);

        // outline 应含该 entry（export_all_favorites 重建）
        let outline = read_clipboard_outline().unwrap();
        assert!(
            outline.favorites.contains_key("push-bbbb-0001"),
            "outline 应含 push 的 favorite"
        );

        // 文件能解密还原 payload
        let key = load_or_create_clipboard_key().unwrap();
        let payload = decrypt_payload(&key, &file.encrypted_payload, "push-bbbb-0001").unwrap();
        assert_eq!(payload.history_row.content, "本地文本");
    }

    /// 测试 3（核心回归——对称 hotword set/word tombstone 优先 fix）：
    /// 远程是 tombstone（A 机删除后 push），本地 DB 仍 active 且 updated_at 更新
    /// → merge 的 `local_updated > remote_updated` 分支不能把 active 写回文件覆盖 tombstone。
    /// 应走 pull tombstone 路径——DB 变 tombstone + 文件保持 tombstone。
    #[test]
    fn merge_remote_tombstone_not_overwritten_by_local_active_newer() {
        let _g = DbSyncGuard::new();

        let fav_id = "revive-cccc-0001";
        let history_id = "hist-cccc-0001";
        // 初始：DB 有 active favorite（updated_at ≈ now）
        insert_history_row(history_id, "会被删除的收藏");
        db::insert_favorite(fav_id, history_id).expect("insert favorite");
        // export 写 active 文件 + outline
        export_all_favorites().expect("export 初始 active");

        // 远程被 A 机删除后 push：tombstone（is_deleted=1 小时前，未超期）
        // updated_ms=1000（比本地 DB 更早——模拟「A 删除了一个很久没动的收藏，B 最近刚改过」）
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let recent_tombstone = now_secs - 3600;
        write_remote_favorite(fav_id, history_id, "会被删除的收藏", recent_tombstone, 1000);

        // 前置：本地 DB 仍 active，updated_at 比远程 tombstone 新
        let db_fav = db::load_favorite(fav_id).unwrap().expect("favorite 应在 DB");
        assert_eq!(db_fav.is_deleted, 0, "前置：本地 DB 仍 active");
        let local_updated = iso_to_unix_ms(&db_fav.updated_at);
        assert!(
            local_updated > 1000,
            "前置：本地 updated_at 比远程 tombstone 新"
        );

        let _report = merge_clipboard_favorites().expect("merge");

        // 🔴 BUG 复现点：merge 后 DB 应被 tombstone 覆盖（不应复活）
        let after = db::load_favorite(fav_id).unwrap().expect("favorite 仍在 DB");
        assert!(
            after.is_deleted > 0,
            "🔴 复活 bug：远程 tombstone 应覆盖本地 active，实际 DB is_deleted={}",
            after.is_deleted
        );

        // 文件也应保持 tombstone
        let file = read_favorite_file(fav_id).expect("文件应存在");
        assert!(
            file.is_deleted > 0,
            "🔴 文件应保持 tombstone（不被本地 active 覆盖），实际 is_deleted={}",
            file.is_deleted
        );
    }
}
