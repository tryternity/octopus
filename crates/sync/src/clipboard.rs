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

use std::cell::RefCell;
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
use crate::pipeline::{is_tombstone_expired, merge_three_way, MergeReport, SyncEntity};
use crate::store::{iso_to_unix_ms, md5_hex, now_secs, shard_dir, sync_root, write_atomically};

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

// === Thread-local ClipboardKey（SyncEntity trait method 间传递加解密 key）===
//
// `merge_three_way` 泛型骨架按 `SyncEntity` trait method 调用 pull/push，trait method 无法
// 传 `ClipboardKey` 参数。用 thread-local 在 `merge_clipboard_favorites` 入口 set 一次，
// trait method 内 `thread_clipboard_key()` 取——单一 merge 调用内 key 不变，对称 hotword
// 无 key 的简单性。
//
// 不用 `load_or_create_clipboard_key()` 在每个 trait method 内重新加载——避免：
// ① 每次读文件 IO；② load 失败时 trait method 报错语义混乱（key 加载本应是入口职责）。
thread_local! {
    static THREAD_KEY: RefCell<Option<ClipboardKey>> = const { RefCell::new(None) };
}

/// merge 入口 set thread-local key——后续 trait method 内 `thread_clipboard_key()` 取。
///
/// 调用方：仅 [`merge_clipboard_favorites`]（单一来源）。trait method 不应调本函数。
fn set_thread_clipboard_key(key: ClipboardKey) {
    THREAD_KEY.with(|k| *k.borrow_mut() = Some(key));
}

/// 第二十二轮 P3-sync1：merge 结束清 thread-local key——ClipboardKey 含 Zeroizing<[u8;32]>
/// （剪贴板 sync 加密密钥）。原 merge 不 clear，spawn_blocking 线程被 tokio pool 复用时
/// key 残留到下次无关任务（卫生瑕疵，非安全漏洞——同进程内 key 已在 DB，残留不新增暴露面）。
/// clear 独立函数便于 RAII guard 调用。
fn clear_thread_clipboard_key() {
    THREAD_KEY.with(|k| *k.borrow_mut() = None);
}

/// RAII guard：drop 时清 thread-local key，保证 merge 任何路径（含 `?` 早返回 / panic）都 clear。
struct ClipboardKeyGuard;
impl Drop for ClipboardKeyGuard {
    fn drop(&mut self) {
        clear_thread_clipboard_key();
    }
}

/// 取 thread-local key——trait method（`write_file` / `upsert_db_from_file`）解密用。
///
/// 未 set 时 panic（`merge_clipboard_favorites` 必然先 set，调用方契约）。
fn thread_clipboard_key() -> ClipboardKey {
    THREAD_KEY.with(|k| {
        k.borrow()
            .clone()
            .expect("clipboard thread key 未 set——merge_clipboard_favorites 应先调 set_thread_clipboard_key")
    })
}

// === 文件格式（Task 5）===

/// 剪贴板 outline——收藏增量索引（history_id → {md5, updatedMs}）。
///
/// 与 vault `Outline` / hotword `HotwordOutline` 对称——BTreeMap 保序列化顺序稳定，
/// 避免相同输入产生不同 JSON（git 误判变化产生空 commit）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardOutline {
    /// outline 格式版本（当前 1）。
    #[serde(default = "default_outline_version")]
    pub version: u32,
    /// 收藏 entries：history_id → {md5, updatedMs}。
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

/// 单个收藏文件（加密后存盘）——`favorites/<2hex>/<history_id>.json`。
///
/// `encrypted_payload` 是 `FavoritePayload` 经 clipboard.key 加密后的 `v1:<base64>` 字符串。
/// 其余字段（id / is_deleted / updated_at）明文——outline diff + 软删判断需读明文，不解密。
///
/// `id` = history_id（同步锚点——简化 3 字段 schema 后 favorite 不再有独立 uuid）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardFavoriteFile {
    /// 文件格式版本（当前 1）。
    pub version: u32,
    /// 同步锚点 = history_id（= clipboard_history.id，跨设备一致）。
    pub id: String,
    /// 软删标记：0=活跃，>0=删除时刻 epoch 秒（tombstone）。与 hotword 语义一致。
    #[serde(default)]
    pub is_deleted: i64,
    /// 加密后的 FavoritePayload（`v1:<base64(nonce||ct||tag)>`）。
    pub encrypted_payload: String,
    /// 更新时间（SQLite datetime 格式）。
    pub updated_at: String,
}

/// 收藏 payload——被 clipboard.key 加密后存入 `ClipboardFavoriteFile.encrypted_payload`。
///
/// 内嵌一份 `HistoryRowJson` 快照（不是引用历史行 id），加密后跨设备自包含——B 机无需
/// 先同步历史即可还原收藏。`history_row.id` 与外层 `ClipboardFavoriteFile.id` 一致
/// （favorite 简化后 history_id 即同步锚点，无独立 favorite id）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoritePayload {
    /// 被收藏的历史行快照（history_row.id 即 favorite 的同步锚点）。
    pub history_row: HistoryRowJson,
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

/// 历史行内容指纹——md5 of 内容承载字段（id + item_type + content + meta_info）。
///
/// favorite 简化后 history_id 即同步锚点，但 history 行内容可被编辑（用户改文本），
/// 单比 history_id 无法发现内容变化。本函数对内容字段取指纹——outline diff + 冲突比对
/// 用它检测「同一 history_id 的内容是否变了」。
fn history_row_md5(row: &HistoryRowJson) -> String {
    // 第二十九轮补充 P2-C1：原 md5 只含 4 字段（id/item_type/content/meta_info），漏
    // ref_data/segments/is_rich。Image/File 的 content 恒空（实际内容在 ref_data），
    // voice 的 segments（润色/编辑段模型）——这些字段变化时 md5 不变 → outline 不 diff
    // → sync 不 push → 远端拿不到新内容，静默数据不一致。补全 3 字段。
    //
    // 注：补字段后所有已 sync 设备的 md5 会变（新增字段参与计算），首次 sync 触发全量
    // conflict（md5 不等）→ 走「DB 赢」push 分支 → 最终收敛。非数据丢失，一次性全量 push。
    md5_hex(
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            row.id,
            row.item_type,
            row.content,
            row.ref_data.as_deref().unwrap_or(""),
            row.meta_info.as_deref().unwrap_or(""),
            row.is_rich,
            row.segments.as_deref().unwrap_or(""),
        )
        .as_bytes(),
    )
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

    // 第二十三轮 P2-sync1（方案 B 先写后清孤儿）：原实现 remove_dir_all(fav_dir) + 重建，
    // remove→create 间崩溃 → 残破工作区（目录被删空，git 捕获删除传播对端）。现改为只
    // ensure fav_dir 存在（create_dir_all 幂等），写完所有文件后清孤儿（见末尾）。
    // 任何时刻崩溃：要么旧文件还在（写未开始/部分），要么新文件已写入（孤儿残留，
    // 下次 export 清）。永不存在「目录被删空」状态。
    let fav_dir = favorites_dir()?;
    std::fs::create_dir_all(&fav_dir)
        .with_context(|| format!("创建 favorites 目录失败：{}", fav_dir.display()))?;

    // 每个 favorite 写文件 + 进 outline + 收集 keep_keys（含 active + 未超期 tombstone，
    // 两者都写文件，都需保留，孤儿清理时以此判定）
    let mut entries: BTreeMap<String, OutlineEntry> = BTreeMap::new();
    let mut keep_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    // 第三十二轮 P2-4：超期 tombstone 不写文件 + 不进 outline（对齐 hotword
    // export_all_hotwords_with :444 is_tombstone_expired 过滤）。GC 启用后 A 机硬删超期
    // tombstone，若 export 仍写文件 → B 机 pull 复活。超期 tombstone 的文件靠孤儿清理删。
    let now_s = now_secs();
    let retention = <ClipboardFavoriteEntity as SyncEntity>::tombstone_retention_secs();
    for fav in &favs {
        // 超期 tombstone 跳过（不写文件、不进 outline、不进 keep_keys → 孤儿清理删其文件）
        if fav.is_deleted > 0 && retention > 0
            && is_tombstone_expired(retention, fav.is_deleted, now_s)
        {
            continue;
        }
        if fav.is_deleted > 0 {
            // tombstone——不需要 payload（内容已无意义，pull 方只看 is_deleted）。
            // encrypted_payload 留空，md5 用空内容固定值。
            let file = ClipboardFavoriteFile {
                version: 1,
                id: fav.history_id.clone(),
                is_deleted: fav.is_deleted,
                encrypted_payload: String::new(),
                updated_at: fav.updated_at.clone(),
            };
            write_favorite_file(&file)?;

            // tombstone 的 md5 是空内容固定值——不参与内容 diff，仅占位
            let md5 = history_row_md5(&HistoryRowJson {
                id: fav.history_id.clone(),
                item_type: String::new(),
                content: String::new(),
                ref_data: None,
                meta_info: None,
                is_rich: false,
                created_at: String::new(),
                segments: None,
            });
            // 第二十九轮补充 P3-CF9：原 let _ = 吞 DB 写错，改 log warn（不阻断——
            // md5 未回写 DB 会导致下次 merge 误判 conflict 做无效 push + 日志噪声，
            // 但不影响正确性，DB 是真相源 export 会重算）。
            if let Err(e) = octopus_infra::db::set_sync_md5(&fav.history_id, &md5) {
                log::warn!("[sync] 收藏 export set_sync_md5 失败（不阻断）：{}", e);
            }
            entries.insert(
                fav.history_id.clone(),
                OutlineEntry {
                    md5,
                    updated_ms: iso_to_unix_ms(&fav.updated_at),
                },
            );
        } else {
            // active——JOIN clipboard_history，加密完整内容
            let history_row = build_history_row(&fav.history_id, &fav.updated_at)?;

            let payload = FavoritePayload { history_row };
            let payload_json =
                serde_json::to_string(&payload).context("序列化 FavoritePayload 失败")?;
            let encrypted = key
                .encrypt(payload_json.as_bytes())
                .context("加密 FavoritePayload 失败")?;

            let file = ClipboardFavoriteFile {
                version: 1,
                id: fav.history_id.clone(),
                is_deleted: fav.is_deleted,
                encrypted_payload: encrypted,
                updated_at: fav.updated_at.clone(),
            };
            write_favorite_file(&file)?;

            let md5 = history_row_md5(&payload.history_row);
            // export 后把磁盘指纹写回 DB——下次 merge 据此比对冲突（DB md5 vs outline md5）。
            // 第二十九轮补充 P3-CF9：原 let _ = 吞 DB 写错，改 log warn（不阻断——
            // md5 未回写 DB 会导致下次 merge 误判 conflict 做无效 push + 日志噪声，
            // 但不影响正确性，DB 是真相源 export 会重算）。
            if let Err(e) = octopus_infra::db::set_sync_md5(&fav.history_id, &md5) {
                log::warn!("[sync] 收藏 export set_sync_md5 失败（不阻断）：{}", e);
            }

            entries.insert(
                fav.history_id.clone(),
                OutlineEntry {
                    md5,
                    updated_ms: iso_to_unix_ms(&fav.updated_at),
                },
            );
        }
        keep_keys.insert(fav.history_id.clone());
    }

    // 清孤儿：扫 favorites/<2hex>/*.json，删 stem 不在 keep_keys 的文件（DB 已删/未导出
    // 但文件残留）。孤儿不影响 merge pull（pull 按 outline 走），但会让 git 历史堆积 +
    // 浪费磁盘。删除失败不阻断（best-effort，下次 export 再清）。
    cleanup_orphan_favorite_files(&keep_keys)?;

    let outline = ClipboardOutline {
        version: 1,
        favorites: entries,
    };
    write_clipboard_outline(&outline)?;
    Ok(outline)
}

/// 第二十三轮 P2-sync1（方案 B）：清理 favorites/ 目录中不在 keep_keys 的孤儿 .json 文件。
///
/// `favorites/` 下是 `<2hex>/<uuid>.json` 两级分片结构。遍历所有分片子目录，删 stem
/// 不在 keep_keys 的 .json 文件。非 .json 文件跳过（防御）。
///
/// 孤儿产生场景：① DB 删除 favorite 后未 export（下次 export 本函数清）；② 上次 export
/// 写入新文件后崩溃，旧文件残留（自愈）。孤儿不影响正确性——merge pull 按 outline 走，
/// 孤儿不在 outline 不会被 pull；outline 用 write_atomically 最后写，崩溃后要么旧
/// （自愈）要么新（与文件一致）。
fn cleanup_orphan_favorite_files(keep_keys: &std::collections::HashSet<String>) -> Result<()> {
    let fav_dir = favorites_dir()?;
    if !fav_dir.is_dir() {
        return Ok(());
    }
    for shard_entry in std::fs::read_dir(&fav_dir)
        .with_context(|| format!("读 favorites 目录失败：{}", fav_dir.display()))?
    {
        let shard_path = shard_entry?.path();
        if !shard_path.is_dir() {
            continue; // 非目录（如意外文件）跳过
        }
        for file_entry in std::fs::read_dir(&shard_path)
            .with_context(|| format!("读分片目录失败：{}", shard_path.display()))?
        {
            let file_path = file_entry?.path();
            // 只处理 .json 文件（防御：跳过 .tmp 等其他文件）
            if file_path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // 提取 uuid（文件名去 .json）——不在 keep_keys 即孤儿
            if let Some(stem) = file_path.file_stem().and_then(|s| s.to_str()) {
                if !keep_keys.contains(stem) {
                    // 孤儿——删除失败 log warn 不阻断（best-effort）
                    if let Err(e) = std::fs::remove_file(&file_path) {
                        log::warn!(
                            "[sync] 清孤儿 favorite 文件失败 {}：{}",
                            file_path.display(),
                            e
                        );
                    }
                }
            }
        }
    }
    Ok(())
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
/// 2026-08-05 重构：委托到泛型 [`merge_three_way`] 骨架，由 [`ClipboardFavoriteEntity`]
/// impl [`SyncEntity`] 提供具体逻辑。判定顺序（tombstone 单向优先 / 时间戳 / md5 冲突 DB 赢）
/// 与原内联实现完全一致——对称 `hotword::merge_hotwords`。
///
/// 流程：
/// 1. set thread-local key（trait method 内加解密用）
/// 2. `merge_three_way::<ClipboardFavoriteEntity>` 驱动 pull/push/conflict/skip
/// 3. `MergeReport` → `ClipboardMergeReport`（保留原对外 API 形状）
pub fn merge_clipboard_favorites() -> Result<ClipboardMergeReport> {
    let key = load_or_create_clipboard_key()?;
    set_thread_clipboard_key(key);
    // 第二十二轮 P3-sync1：guard 确保 merge 任何路径（Ok/Err/panic）都清 thread-local key。
    let _key_guard = ClipboardKeyGuard;
    let mut report = MergeReport::default();
    let now = now_secs();
    merge_three_way::<ClipboardFavoriteEntity>(&mut report, now)?;
    log::info!(
        "[sync] merge_clipboard_favorites 完成：pulled={} pushed={} conflicts={} skipped={}",
        report.pulled,
        report.pushed,
        report.conflicts,
        report.skipped
    );
    Ok(report.into())
}

impl From<MergeReport> for ClipboardMergeReport {
    /// `MergeReport`（pipeline 通用 4 字段）→ `ClipboardMergeReport`（保留原对外形状）。
    fn from(r: MergeReport) -> Self {
        ClipboardMergeReport {
            pulled: r.pulled,
            pushed: r.pushed,
            conflicts: r.conflicts,
            skipped: r.skipped,
        }
    }
}

/// Pull 单个收藏（文件 → DB）。返回 Ok(true) 成功 upsert，Ok(false) 跳过。
///
/// 2026-08-05 重构：本函数是 `SyncEntity::upsert_db_from_file` 的核心逻辑——接收从文件
/// 解出的 `ClipboardFavorite` 行（已含 sync_md5 / is_deleted），执行 DB 写入。
/// key 从 thread-local 取（merge 入口 set）。
///
/// tombstone（is_deleted>0）：
/// - DB 有且 active → soft_delete_favorite + history.is_favorite=0
/// - DB 无 → 直接 INSERT tombstone favorite（不需要还原 history 内容——
///   tombstone 的唯一目的是传播「此 history_id 已删除」，内容已无意义）
/// - DB 已是 tombstone → skip
///
/// active（is_deleted==0）：
/// - 解密 payload 取 HistoryRowJson
/// - UPSERT history 行
/// - UPSERT favorite（is_deleted=0）
/// - history.is_favorite=1
fn pull_favorite(fav: &octopus_infra::db::ClipboardFavorite) -> Result<bool> {
    let history_id = &fav.history_id;

    if fav.is_deleted > 0 {
        // tombstone
        let existing = octopus_infra::db::load_favorite(history_id)?;
        match existing {
            Some(db_f) if db_f.is_deleted == 0 => {
                // DB active → 软删。第十七轮 P1-2：改用 upsert_favorite_sync 传远程
                // updated_at（原 soft_delete_favorite 硬编 datetime('now') 丢远程时间戳 →
                // 多设备交替 sync ping-pong，vault 第十一轮 P1 同型重现）。与 None/active
                // 两分支对称（都传 fav.updated_at.clone()）。
                let tombstone_fav = octopus_infra::db::ClipboardFavorite {
                    history_id: history_id.to_string(),
                    is_deleted: fav.is_deleted,
                    updated_at: fav.updated_at.clone(),
                    sync_md5: None,
                };
                octopus_infra::db::upsert_favorite_sync(&tombstone_fav)?;
                octopus_infra::db::set_clipboard_is_favorite(&db_f.history_id, false)?;
                log::debug!("[sync] 收藏 pull: {} DB active → 软删", history_id);
                Ok(true)
            }
            None => {
                // DB 无 → 直接 INSERT tombstone favorite（不解密 payload、不还原 history 行）。
                // tombstone 的唯一目的是传播「此 history_id 已删除」，内容已无意义。
                // 如果 history 行恰好存在（is_favorite 可能=1），也要清掉收藏标记。
                let tombstone_fav = octopus_infra::db::ClipboardFavorite {
                    history_id: history_id.to_string(),
                    is_deleted: fav.is_deleted,
                    updated_at: fav.updated_at.clone(),
                    sync_md5: None, // tombstone 不参与内容 diff
                };
                octopus_infra::db::upsert_favorite_sync(&tombstone_fav)?;
                // history 行可能存在且 is_favorite=1（favorites 表记录丢了但 history 标记还在）
                // ——幂等清掉，不存在则 no-op。第二十七轮 P2-1：原 let _ = 吞错，现 log warn
                // （不阻断——favorite tombstone 已写入，history.is_favorite 残留不影响 sync 正确性，
                // 下次 export 会基于 favorite 表重建，UI 列表取 favorite 表为准）。
                if let Err(e) = octopus_infra::db::set_clipboard_is_favorite(history_id, false) {
                    log::warn!(
                        "[sync] 收藏 pull: {} tombstone 后清 history.is_favorite 失败（不阻断）：{}",
                        history_id, e
                    );
                }
                log::debug!("[sync] 收藏 pull: {} DB 无 → INSERT tombstone + 清 history.is_favorite", history_id);
                Ok(true)
            }
            Some(_) => {
                // DB 已是 tombstone——已是终态，跳过
                log::debug!("[sync] 收藏 pull: {} DB 已 tombstone，skip", history_id);
                Ok(false)
            }
        }
    } else {
        // active——需解密 payload 取 HistoryRowJson（file_to_row 已解一次，
        // 但 ClipboardFavorite 不存 payload；这里重读文件解密，与原 pull_favorite 等价）。

        // 第二十一轮 P2-s5：反向复活保护（对齐 hotword pull_set :971-982 + vault P2-2）——
        // DB 已 tombstone（本地删了）+ 远程 active（旧状态/git revert/第三方设备）→ 拒绝 pull，
        // 防删除被远程旧 active 复活。可自愈（用户重删，下次 sync 推 tombstone）。
        if let Some(db_fav) = octopus_infra::db::load_favorite(history_id)? {
            if db_fav.is_deleted > 0 {
                log::info!(
                    "[sync] 收藏 pull: {} 本地已 tombstone，远程 active，拒绝复活",
                    history_id
                );
                return Ok(false);
            }
        }

        let key = thread_clipboard_key();
        let file = read_favorite_file(history_id)?;
        let payload = decrypt_payload(&key, &file.encrypted_payload, history_id)?;
        upsert_history_from_payload(&payload)?;
        let active_fav = octopus_infra::db::ClipboardFavorite {
            history_id: payload.history_row.id.clone(),
            is_deleted: 0,
            updated_at: fav.updated_at.clone(),
            sync_md5: Some(history_row_md5(&payload.history_row)),
        };
        octopus_infra::db::upsert_favorite_sync(&active_fav)?;
        octopus_infra::db::set_clipboard_is_favorite(&payload.history_row.id, true)?;
        log::debug!("[sync] 收藏 pull: {} active → 还原", history_id);
        Ok(true)
    }
}

/// 解密 payload 并校验 history_row.id 与文件 id 一致（防文件 ↔ payload 错配）。
fn decrypt_payload(
    key: &ClipboardKey,
    encrypted: &str,
    expected_history_id: &str,
) -> Result<FavoritePayload> {
    let plaintext = key.decrypt(encrypted)?;
    let payload: FavoritePayload =
        serde_json::from_slice(&plaintext).context("解密后 payload JSON 解析失败")?;
    if payload.history_row.id != expected_history_id {
        anyhow::bail!(
            "payload.history_row.id ({}) 与文件 id ({}) 不符——拒绝错配",
            payload.history_row.id,
            expected_history_id
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
///
/// 2026-08-05 重构：key 从 thread-local 取（merge 入口 set），与 `pull_favorite` 对称。
fn push_favorite(fav: &octopus_infra::db::ClipboardFavorite) -> Result<()> {
    let key = thread_clipboard_key();
    let history_row = build_history_row(&fav.history_id, &fav.updated_at)?;
    let payload = FavoritePayload { history_row };
    let payload_json = serde_json::to_string(&payload)?;
    let encrypted = key.encrypt(payload_json.as_bytes())?;
    let file = ClipboardFavoriteFile {
        version: 1,
        id: fav.history_id.clone(),
        is_deleted: fav.is_deleted,
        encrypted_payload: encrypted,
        updated_at: fav.updated_at.clone(),
    };
    write_favorite_file(&file)
}

/// 读 DB 的 clipboard_history 行构造 `HistoryRowJson`（push + export 共用）。
/// 行不存在时返回占位行（favorite tombstone 仍需传播删除意图）。
fn build_history_row(history_id: &str, fallback_updated_at: &str) -> Result<HistoryRowJson> {
    let row = octopus_infra::db::load_clipboard_history_row(history_id)?;
    match row {
        Some(r) => Ok(HistoryRowJson {
            id: r.id,
            item_type: r.item_type,
            content: r.content,
            ref_data: r.ref_data,
            meta_info: r.meta_info,
            is_rich: r.is_rich,
            created_at: r.created_at,
            segments: r.segments,
        }),
        None => {
            log::warn!(
                "[sync] 收藏 {} 的 history 行不存在，写占位 payload",
                history_id
            );
            Ok(HistoryRowJson {
                id: history_id.into(),
                item_type: "text".into(),
                content: String::new(),
                ref_data: None,
                meta_info: None,
                is_rich: false,
                created_at: fallback_updated_at.into(),
                segments: None,
            })
        }
    }
}

// === SyncEntity impl（2026-08-05，trait 统一 merge 骨架）===

/// clipboard favorite 的 [`SyncEntity`] 标记类型——零大小，仅承载 trait impl。
///
/// `merge_three_way::<ClipboardFavoriteEntity>` 驱动 clipboard favorite 的 3-way merge，
/// 各 method 委托到本模块现有函数（`read_favorite_file` / `pull_favorite` / `push_favorite`
/// / `list_all_favorites` / `export_all_favorites` / `read_clipboard_outline`）。
///
/// - `Row` = `ClipboardFavorite`（infra DB 行）——`list_db_rows` 读 DB / `file_to_row` 从文件解 / `write_file` / `upsert_db_from_file` 共用此类型。
/// - `File` = `ClipboardFavoriteFile`（磁盘序列化格式）。
/// - 加密 key 通过 thread-local 传递（`merge_clipboard_favorites` 入口 set）。
pub struct ClipboardFavoriteEntity;

impl SyncEntity for ClipboardFavoriteEntity {
    type Row = octopus_infra::db::ClipboardFavorite;
    type File = ClipboardFavoriteFile;

    const LABEL: &'static str = "clipboard_favorite";

    /// 30 天（对称 vault 30 天，比 hotword 10 天长——收藏是用户主动收藏的内容，误删后悔窗口长）。
    /// 对应 infra `CLIPBOARD_TOMBSTONE_RETENTION_SECS`。
    fn tombstone_retention_secs() -> i64 {
        octopus_infra::db::CLIPBOARD_TOMBSTONE_RETENTION_SECS
    }

    // ── DB 操作 ──

    fn list_db_rows() -> Result<Vec<Self::Row>> {
        octopus_infra::db::list_all_favorites()
    }

    fn sync_key(row: &Self::Row) -> &str {
        &row.history_id
    }

    fn updated_ms(row: &Self::Row) -> i64 {
        iso_to_unix_ms(&row.updated_at)
    }

    fn is_tombstone(row: &Self::Row) -> bool {
        row.is_deleted > 0
    }

    fn md5_of(row: &Self::Row) -> String {
        row.sync_md5.clone().unwrap_or_default()
    }

    // ── 文件操作 ──

    fn read_file(key: &str) -> Result<Self::File> {
        read_favorite_file(key)
    }

    /// 从文件构建 DB 行——active 项需解密 payload 取 history_row.id + 算 sync_md5；
    /// tombstone 项（is_deleted>0）不参与内容 diff，sync_md5 = None。
    ///
    /// 注意：payload 的 history_row 不在此 upsert 到 DB（那是 `upsert_db_from_file` 的职责）。
    /// 这里只构建 ClipboardFavorite（DB 行镜像），供 pipeline 比较 updated_ms / md5。
    fn file_to_row(file: &Self::File) -> Self::Row {
        if file.is_deleted > 0 {
            // tombstone——sync_md5 = None（不参与内容 diff），与 export_all_favorites tombstone 分支一致
            octopus_infra::db::ClipboardFavorite {
                history_id: file.id.clone(),
                is_deleted: file.is_deleted,
                updated_at: file.updated_at.clone(),
                sync_md5: None,
            }
        } else {
            // active——解密 payload 算 sync_md5（与 export_all_favorites active 分支一致）
            let key = thread_clipboard_key();
            let sync_md5 = match decrypt_payload(&key, &file.encrypted_payload, &file.id) {
                Ok(payload) => Some(history_row_md5(&payload.history_row)),
                Err(e) => {
                    // 解密失败——sync_md5 = None，让 upsert_db_from_file 再试（届时失败会被
                    // pipeline 的 Err → log_warn_skip 捕获）。不在此 bail——file_to_row 无 Result 返回。
                    log::warn!("[sync] clipboard_favorite {} file_to_row 解密失败：{}", file.id, e);
                    None
                }
            };
            octopus_infra::db::ClipboardFavorite {
                history_id: file.id.clone(),
                is_deleted: file.is_deleted,
                updated_at: file.updated_at.clone(),
                sync_md5,
            }
        }
    }

    fn file_is_tombstone(file: &Self::File) -> bool {
        file.is_deleted > 0
    }

    fn file_tombstone_timestamp(file: &Self::File) -> i64 {
        file.is_deleted
    }

    fn write_file(row: &Self::Row) -> Result<()> {
        // 委托到 push_favorite（含加密 + build_history_row + 写盘）
        push_favorite(row)
    }

    // ── merge 操作 ──

    fn upsert_db_from_file(row: &Self::Row) -> Result<bool> {
        pull_favorite(row)
    }

    // ── GC（对称 hotword purge_expired_hotword_tombstones）──

    fn purge_expired_tombstones(now: i64) -> Result<usize> {
        let purged = octopus_infra::db::purge_expired_clipboard_favorites(now)?;
        if purged > 0 {
            log::info!("[sync] clipboard_favorite GC: 清理 {} 条超期 tombstone", purged);
        }
        Ok(purged)
    }

    // ── 导出 ──

    fn export_all() -> Result<()> {
        // export_all_favorites 内部自己 load key（与 trait method 无参约束兼容）
        export_all_favorites().map(|_| ())
    }

    // ── outline ──

    fn read_outline_entries() -> Result<Vec<(String, OutlineEntry)>> {
        let outline = read_clipboard_outline()?;
        Ok(outline.favorites.into_iter().collect())
    }
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
        };
        let json = serde_json::to_string(&payload).unwrap();
        // camelCase 字段名
        assert!(json.contains("\"historyRow\""), "historyRow 应 camelCase");
        assert!(json.contains("\"itemType\""), "itemType 应 camelCase");
        assert!(json.contains("\"isRich\""), "isRich 应 camelCase");
        assert!(json.contains("\"metaInfo\""), "metaInfo 应 camelCase");
        assert!(json.contains("\"refData\""), "refData 应 camelCase");
        // 不应再含 favorite_id / content_hash（简化后 payload 只剩 history_row）
        assert!(
            !json.contains("favoriteId"),
            "简化后 payload 不应含 favoriteId"
        );
        assert!(
            !json.contains("contentHash"),
            "简化后 payload 不应含 contentHash"
        );
        // round-trip
        let parsed: FavoritePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.history_row.id, "hist-uuid");
    }

    /// 端到端：payload 加密 → 写文件 → 读文件 → 解密 → 还原 payload。
    #[test]
    fn encrypt_write_read_decrypt_roundtrip() {
        let _g = SyncRootGuard::new();
        let key = load_or_create_clipboard_key().unwrap();

        let history_id = "hist-uuid";
        let payload = FavoritePayload {
            history_row: HistoryRowJson {
                id: history_id.into(),
                item_type: "text".into(),
                content: "剪贴板收藏内容".into(),
                ref_data: Some(r#"{"src":"safari"}"#.into()),
                meta_info: None,
                is_rich: true,
                created_at: "2026-08-03 10:00:00".into(),
                segments: None,
            },
        };
        let payload_json = serde_json::to_string(&payload).unwrap();
        let encrypted = key.encrypt(payload_json.as_bytes()).unwrap();

        let file = ClipboardFavoriteFile {
            version: 1,
            id: history_id.into(),
            is_deleted: 0,
            encrypted_payload: encrypted,
            updated_at: "2026-08-03 10:00:00".into(),
        };
        write_favorite_file(&file).unwrap();

        let loaded = read_favorite_file(&file.id).unwrap();
        let decrypted = key.decrypt(&loaded.encrypted_payload).unwrap();
        let restored: FavoritePayload = serde_json::from_slice(&decrypted).unwrap();
        assert_eq!(restored.history_row.id, history_id);
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
    /// 简化后 favorite id == history_id（PK 即同步锚点），payload 只含 history_row。
    fn write_remote_favorite(
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
        let md5 = history_row_md5(&history_row);
        let payload = FavoritePayload { history_row };
        let payload_json = serde_json::to_string(&payload).unwrap();
        let encrypted = key.encrypt(payload_json.as_bytes()).unwrap();
        let file = ClipboardFavoriteFile {
            version: 1,
            id: history_id.into(),
            is_deleted,
            encrypted_payload: encrypted,
            updated_at: "2026-08-03 10:00:00".into(),
        };
        write_favorite_file(&file).unwrap();
        // outline entry（key = history_id）
        let mut outline = read_clipboard_outline().unwrap_or_default();
        outline.favorites.insert(
            history_id.into(),
            OutlineEntry {
                md5,
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

        let history_id = "hist-aaaa-0001";
        write_remote_favorite(history_id, "远程文本", 0, 9999999999999);

        let report = merge_clipboard_favorites().expect("merge");
        assert_eq!(report.pulled, 1, "应拉取 1 条远程收藏");

        // DB 应有该 favorite（active）——key = history_id
        let fav = db::load_favorite(history_id).unwrap().expect("favorite 应在 DB");
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
        db::insert_favorite(history_id).expect("insert_favorite");

        let report = merge_clipboard_favorites().expect("merge");
        assert!(report.pushed >= 1, "DB only favorite 应 push 到文件");

        // 文件应存在（id == history_id）
        let file = read_favorite_file(history_id).expect("文件应被写");
        assert_eq!(file.id, history_id);
        assert_eq!(file.is_deleted, 0);

        // outline 应含该 entry（export_all_favorites 重建）
        let outline = read_clipboard_outline().unwrap();
        assert!(
            outline.favorites.contains_key(history_id),
            "outline 应含 push 的 favorite（key = history_id）"
        );

        // 文件能解密还原 payload
        let key = load_or_create_clipboard_key().unwrap();
        let payload = decrypt_payload(&key, &file.encrypted_payload, history_id).unwrap();
        assert_eq!(payload.history_row.content, "本地文本");
    }

    /// 测试 3（核心回归——对称 hotword set/word tombstone 优先 fix）：
    /// 远程是 tombstone（A 机删除后 push），本地 DB 仍 active 且 updated_at 更新
    /// → merge 的 `local_updated > remote_updated` 分支不能把 active 写回文件覆盖 tombstone。
    /// 应走 pull tombstone 路径——DB 变 tombstone + 文件保持 tombstone。
    #[test]
    fn merge_remote_tombstone_not_overwritten_by_local_active_newer() {
        let _g = DbSyncGuard::new();

        let history_id = "hist-cccc-0001";
        // 初始：DB 有 active favorite（updated_at ≈ now）
        insert_history_row(history_id, "会被删除的收藏");
        db::insert_favorite(history_id).expect("insert favorite");
        // export 写 active 文件 + outline
        export_all_favorites().expect("export 初始 active");

        // 远程被 A 机删除后 push：tombstone（is_deleted=1 小时前，未超期）
        // updated_ms=1000（比本地 DB 更早——模拟「A 删除了一个很久没动的收藏，B 最近刚改过」）
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let recent_tombstone = now_secs - 3600;
        write_remote_favorite(history_id, "会被删除的收藏", recent_tombstone, 1000);

        // 前置：本地 DB 仍 active，updated_at 比远程 tombstone 新
        let db_fav = db::load_favorite(history_id).unwrap().expect("favorite 应在 DB");
        assert_eq!(db_fav.is_deleted, 0, "前置：本地 DB 仍 active");
        let local_updated = iso_to_unix_ms(&db_fav.updated_at);
        assert!(
            local_updated > 1000,
            "前置：本地 updated_at 比远程 tombstone 新"
        );

        let _report = merge_clipboard_favorites().expect("merge");

        // 🔴 BUG 复现点：merge 后 DB 应被 tombstone 覆盖（不应复活）
        let after = db::load_favorite(history_id).unwrap().expect("favorite 仍在 DB");
        assert!(
            after.is_deleted > 0,
            "🔴 复活 bug：远程 tombstone 应覆盖本地 active，实际 DB is_deleted={}",
            after.is_deleted
        );

        // 文件也应保持 tombstone
        let file = read_favorite_file(history_id).expect("文件应存在");
        assert!(
            file.is_deleted > 0,
            "🔴 文件应保持 tombstone（不被本地 active 覆盖），实际 is_deleted={}",
            file.is_deleted
        );
    }

    /// 第二十三轮 P2-sync1（方案 B）回归：cleanup_orphan_favorite_files 清理孤儿。
    ///
    /// 手造一个 keep_keys + 分片子目录里塞孤儿 .json + 合法 .json → cleanup 后孤儿删除、
    /// 合法保留。验证「先写后清孤儿」方案：新文件已写入后，旧的/残留的被清。
    #[test]
    fn cleanup_orphan_favorite_files_removes_stale_keeps_valid() {
        let _guard = SyncRootGuard::new();
        let fav_dir = favorites_dir().unwrap();
        // 模拟 3 个文件：keep1（保留）、keep2（保留）、orphan（删除）
        let keep_keys: std::collections::HashSet<String> =
            ["keep1", "keep2"].iter().map(|s| s.to_string()).collect();
        // 分片子目录（用 shard_dir 拿 <2hex>）
        let shard1 = fav_dir.join(shard_dir("keep1"));
        std::fs::create_dir_all(&shard1).unwrap();
        std::fs::write(shard1.join("keep1.json"), "{}").unwrap();
        std::fs::write(shard1.join("orphan.json"), "{}").unwrap(); // 同分片孤儿
        let shard2 = fav_dir.join(shard_dir("keep2"));
        std::fs::create_dir_all(&shard2).unwrap();
        std::fs::write(shard2.join("keep2.json"), "{}").unwrap();

        cleanup_orphan_favorite_files(&keep_keys).unwrap();

        // keep1/keep2 保留，orphan 删除
        assert!(shard1.join("keep1.json").exists(), "keep1 应保留");
        assert!(shard2.join("keep2.json").exists(), "keep2 应保留");
        assert!(!shard1.join("orphan.json").exists(), "orphan 应被清理");
    }

    /// 第二十三轮 P2-sync1（方案 B）回归：cleanup 对空目录/不存在目录不报错（幂等）。
    #[test]
    fn cleanup_orphan_favorite_files_empty_dir_ok() {
        let _guard = SyncRootGuard::new();
        // favorites/ 不存在——cleanup 应 Ok（不报错）
        let keep_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert!(cleanup_orphan_favorite_files(&keep_keys).is_ok());
    }
}
