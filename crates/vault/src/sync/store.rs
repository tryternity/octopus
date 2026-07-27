//! 文件存储：`~/.octopus/.sync/vault/` 下 meta.json / outline.json /
//! ciphers/<桶>/<uuid>.json / folders/<uuid>.json 的读写。
//!
//! **加密层零改动**——store.rs 接收已加密的 VaultCipher / VaultFolder 行（storage
//! 模块已在 SQLite 层加密），文件里的 `encrypted.*` 字段就是 `v1:` 前缀密文，
//! 与 SQLite 中格式完全一致。
//!
//! 文件结构（详见 sync/mod.rs 模块注释）：
//! - meta.json：vault_meta 同步字段（KDF 参数 + protected_user_vault_key +
//!   app_key_sync_enc + security_stamp）
//! - outline.json：uuid → md5 增量索引
//! - ciphers/<前2hex>/<uuid>.json：单 cipher 加密 blob
//! - folders/<uuid>.json：folder 加密 blob（folder 也分桶，与 cipher 一致）
//!
//! 2026-07-22 抽离：通用路径/hash 工具（sync_root / shard_dir / sha256_hex /
//! md5_hex / iso_to_unix_ms / 测试隔离 thread_local）已搬到 `octopus_sync::store`。
//! 本模块只保留 vault 业务数据文件格式 + 路径。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use octopus_infra::db::{VaultCipher, VaultFolder, VaultMeta};

use octopus_sync::outline::{Outline, OutlineEntry};
// 通用工具：sync_root（git repo 根）/ shard_dir（分桶）/ iso_to_unix_ms（时间转换）
use octopus_sync::store as sync_store;

// === 测试隔离（薄封装，转发到 octopus_sync::store） ===
//
// 旧代码用 `set_test_vault_root` / `clear_test_vault_root` 做测试隔离——这两个函数
// 内部转发到 `octopus_sync::store::set_test_sync_root` / `clear_test_sync_root`，
// 保持 vault crate 旧测试代码改动最小（只改 import）。

/// 测试专用：设置临时 vault 数据根（转发到 sync crate 的 sync_root override）。
#[cfg(test)]
pub fn set_test_vault_root(path: PathBuf) {
    sync_store::set_test_sync_root(path);
}

/// 测试专用：清除 vault 数据根 override（转发到 sync crate）。
#[cfg(test)]
pub fn clear_test_vault_root() {
    sync_store::clear_test_sync_root();
}

// === vault 业务路径（基于 sync crate 的 sync_root） ===

/// `~/.octopus/.sync/vault/`——vault 数据子目录（meta/outline/ciphers/folders）。
///
/// git repo 在 `octopus_sync::store::sync_root()`（`.sync/`），不在 vault 子目录——
/// 这样 hotword/prompts 等其他同步数据也在同一个 git repo 下。
pub fn vault_dir() -> PathBuf {
    sync_store::sync_root().join("vault")
}

/// 兼容别名——大量旧代码引用 `vault_root()`，逐步迁移到 `vault_dir()`。
/// 语义等同 vault_dir()（vault 数据目录，非 git repo 根）。
pub fn vault_root() -> PathBuf {
    vault_dir()
}

/// `~/.octopus/.sync/vault/meta.json`——vault_meta 同步字段。
pub fn meta_path() -> PathBuf {
    vault_root().join("meta.json")
}

/// `~/.octopus/.sync/vault/outline.json`——增量索引。
pub fn outline_path() -> PathBuf {
    vault_root().join("outline.json")
}

/// cipher 文件路径：`ciphers/<前2hex>/<uuid>.json`（shard_dir 来自 sync crate）。
/// E-PATH-TRAVERSAL-OUTLINE-UUID 修复（2026-07-26）：校验 uuid 不含 path traversal 字符。
///
/// 之前 cipher_file_path/folder_file_path 的 `format!("{}.json", uuid)` 原样拼接 uuid，
/// 远程 outline 的恶意 uuid（如 `../../meta`）可触发 path traversal——read 路径读
/// traversal 文件、delete 路径删任意 .json 文件。shard_dir 只 sanitize 分片目录，
/// 不 sanitize 文件名。
///
/// 本 chokepoint 在路径构造入口校验，read/delete/write 三路径统一拦截。
/// 不强制严格 UUID 格式（测试用简短 id 方便），只拒绝 path traversal 字符。
fn validate_uuid(uuid: &str) -> Result<()> {
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

pub fn cipher_file_path(uuid: &str) -> Result<PathBuf> {
    validate_uuid(uuid)?;
    Ok(vault_root()
        .join("ciphers")
        .join(sync_store::shard_dir(uuid))
        .join(format!("{}.json", uuid)))
}

/// folder 文件路径：`folders/<前2hex>/<uuid>.json`（folder 也分桶）。
pub fn folder_file_path(uuid: &str) -> Result<PathBuf> {
    validate_uuid(uuid)?;
    Ok(vault_root()
        .join("folders")
        .join(sync_store::shard_dir(uuid))
        .join(format!("{}.json", uuid)))
}

// === 数据结构 ===

/// meta.json 内容——只含同步所需字段（不含 app_key_local_enc / K_machine 相关）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetaFile {
    pub version: u32,
    pub kdf_type: i64,
    /// base64 编码的 kdf_salt（JSON 中 byte array 用 base64 比 Vec<u8> 数字数组更紧凑）。
    pub kdf_salt: String,
    pub kdf_iterations: i64,
    pub kdf_memory_kib: i64,
    pub kdf_parallelism: i64,
    pub protected_user_vault_key: String,
    pub app_key_sync_enc: String,
    pub security_stamp: String,
    pub equivalent_domains: String,
}

impl MetaFile {
    /// 从 VaultMeta（SQLite 行）转换。
    ///
    /// 跳过 `app_key_local_enc`（K_machine 加密，不可同步）+ `public_key` /
    /// `protected_private_key`（MVP 未用）+ 时间戳（meta.json 由 git 管理时间）。
    pub fn from_vault_meta(meta: &VaultMeta) -> Self {
        use base64::Engine;
        Self {
            version: 1,
            kdf_type: meta.kdf_type,
            kdf_salt: base64::engine::general_purpose::STANDARD.encode(&meta.kdf_salt),
            kdf_iterations: meta.kdf_iterations,
            kdf_memory_kib: meta.kdf_memory_kib,
            kdf_parallelism: meta.kdf_parallelism,
            protected_user_vault_key: meta.protected_user_vault_key.clone(),
            app_key_sync_enc: meta.app_key_sync_enc.clone(),
            security_stamp: meta.security_stamp.clone(),
            equivalent_domains: meta.equivalent_domains.clone(),
        }
    }

    /// 转回 VaultMeta 的同步字段（不含 local_enc / public_key 等本机字段——
    /// 上层 upsert 时从本机 vault_meta 保留这些）。
    ///
    /// #9 修复（2026-07-24）：base64 salt 解码失败不再 `unwrap_or_default()` 吞错
    /// （空 Vec 静默通过 → Argon2 用空 salt 派生 → 解 protected_user_vault_key 失败
    /// → 误导用户反复输错密码，根因实为 salt 解码失败）。现在返 `Result`，解码失败
    /// 显式报错。
    pub fn to_sync_fields(&self) -> Result<MetaSyncFields> {
        use base64::Engine;
        let salt = base64::engine::general_purpose::STANDARD
            .decode(&self.kdf_salt)
            .with_context(|| format!("meta.json kdf_salt base64 解码失败：{}", self.kdf_salt))?;
        Ok(MetaSyncFields {
            kdf_type: self.kdf_type,
            kdf_salt: salt,
            kdf_iterations: self.kdf_iterations,
            kdf_memory_kib: self.kdf_memory_kib,
            kdf_parallelism: self.kdf_parallelism,
            protected_user_vault_key: self.protected_user_vault_key.clone(),
            app_key_sync_enc: self.app_key_sync_enc.clone(),
            security_stamp: self.security_stamp.clone(),
            equivalent_domains: self.equivalent_domains.clone(),
        })
    }
}

/// meta.json 解析出的同步字段（to_sync_fields 的结构化返回，#9 修复）。
///
/// 之前用 9-tuple 返回，字段位置容易搞混；改 struct 让代码自文档化。
#[derive(Debug)]
pub struct MetaSyncFields {
    pub kdf_type: i64,
    pub kdf_salt: Vec<u8>,
    pub kdf_iterations: i64,
    pub kdf_memory_kib: i64,
    pub kdf_parallelism: i64,
    pub protected_user_vault_key: String,
    pub app_key_sync_enc: String,
    pub security_stamp: String,
    pub equivalent_domains: String,
}

/// cipher 文件——encrypted 字段是 user_vault_key 加密的 v1: 前缀密文（与 SQLite 一致）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CipherFile {
    pub version: u32,
    pub id: String,
    pub encrypted: CipherEncStrings,
    pub plaintext_meta: CipherPlaintextMeta,
}

/// cipher 加密字段——与 SQLite vault_ciphers 表的敏感字段一一对应。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CipherEncStrings {
    pub name: String,
    pub notes: Option<String>,
    pub data: String,
    pub fields: Option<String>,
    pub password_history: Option<String>,
}

/// cipher 非敏感元数据（明文存储——这些字段在 SQLite 里也是明文）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CipherPlaintextMeta {
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub atype: i64,
    pub reprompt: i64,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl CipherFile {
    /// 从 VaultCipher（SQLite 行）转换——字段一一对应，无加解密。
    pub fn from_vault_cipher(row: &VaultCipher) -> Self {
        Self {
            version: 1,
            id: row.id.clone(),
            encrypted: CipherEncStrings {
                name: row.name.clone(),
                notes: row.notes.clone(),
                data: row.data.clone(),
                fields: row.fields.clone(),
                password_history: row.password_history.clone(),
            },
            plaintext_meta: CipherPlaintextMeta {
                folder_id: row.folder_id.clone(),
                favorite: row.favorite,
                atype: row.atype,
                reprompt: row.reprompt,
                deleted_at: row.deleted_at.clone(),
                created_at: row.created_at.clone(),
                updated_at: row.updated_at.clone(),
            },
        }
    }

    /// 转回 VaultCipher（用于 import 回 SQLite）。
    ///
    /// 注意：id / folder_id / 时间戳都从文件取——同步场景下这些字段需要保持
    /// 跨设备一致。
    pub fn to_vault_cipher(&self) -> VaultCipher {
        VaultCipher {
            id: self.id.clone(),
            folder_id: self.plaintext_meta.folder_id.clone(),
            favorite: self.plaintext_meta.favorite,
            atype: self.plaintext_meta.atype,
            name: self.encrypted.name.clone(),
            notes: self.encrypted.notes.clone(),
            data: self.encrypted.data.clone(),
            fields: self.encrypted.fields.clone(),
            password_history: self.encrypted.password_history.clone(),
            reprompt: self.plaintext_meta.reprompt,
            deleted_at: self.plaintext_meta.deleted_at.clone(),
            sync_md5: None, // 由调用方算 md5 填入（pull 时 fingerprint::cipher_md5）
            created_at: self.plaintext_meta.created_at.clone(),
            updated_at: self.plaintext_meta.updated_at.clone(),
        }
    }
}

/// folder 文件——结构比 cipher 简单（只有 name 加密）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FolderFile {
    pub version: u32,
    pub id: String,
    pub encrypted_name: String,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl FolderFile {
    pub fn from_vault_folder(row: &VaultFolder) -> Self {
        Self {
            version: 1,
            id: row.id.clone(),
            encrypted_name: row.name.clone(),
            sort_order: row.sort_order,
            created_at: row.created_at.clone(),
            updated_at: row.updated_at.clone(),
        }
    }

    pub fn to_vault_folder(&self) -> VaultFolder {
        VaultFolder {
            id: self.id.clone(),
            name: self.encrypted_name.clone(),
            sort_order: self.sort_order,
            sync_md5: None, // 由调用方算 md5 填入
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

// === 读写函数 ===

/// 原子写文件（L4 修复，2026-07-24）。
///
/// 模式：temp file + write_all + sync_all + rename（POSIX rename 原子）。
/// 复用 keychain.rs:314-388 的模式，但**不设 0600**——sync 文件需正常权限
/// （git 同步场景，其他设备读取；与现有 std::fs::write 的 umask 行为一致）。
///
/// temp 文件用 `.<name>.tmp` 前缀（隐藏 + 不匹配 walk_json_files 的 .json 扫描），
/// 保证与目标同目录（同卷，rename 原子）。
fn write_atomically(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败：{}", parent.display()))?;
    }
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("data");
    let tmp_path = path.with_file_name(format!(".{}.tmp", file_name));

    #[cfg(unix)]
    {
        use std::io::Write;
        {
            let mut f = std::fs::File::create(&tmp_path)
                .with_context(|| format!("创建临时文件失败：{}", tmp_path.display()))?;
            f.write_all(content.as_bytes())
                .with_context(|| format!("写入临时文件失败：{}", tmp_path.display()))?;
            f.sync_all()
                .with_context(|| format!("fsync 临时文件失败：{}", tmp_path.display()))?;
        }
        std::fs::rename(&tmp_path, path).with_context(|| {
            format!("原子替换失败：{} -> {}", tmp_path.display(), path.display())
        })?;
        // N3 修复（2026-07-24）：rename 后 fsync 父目录——POSIX 下目录项更新
        // 需 fsync 才能扛断电，否则断电恰在 rename 后可能丢 rename（恢复后看到旧版本）。
        if let Some(parent) = path.parent() {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all(); // 目录 fsync 失败不阻断（best-effort）
            }
        }
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        {
            let mut f = std::fs::File::create(&tmp_path)
                .with_context(|| format!("创建临时文件失败：{}", tmp_path.display()))?;
            f.write_all(content.as_bytes())
                .with_context(|| format!("写入临时文件失败：{}", tmp_path.display()))?;
            f.sync_all()
                .with_context(|| format!("fsync 临时文件失败：{}", tmp_path.display()))?;
        }
        std::fs::rename(&tmp_path, path).with_context(|| {
            format!("原子替换失败：{} -> {}", tmp_path.display(), path.display())
        })?;
        // Windows: MoveFileEx(REPLACE_EXISTING) 已保证可见性，无需目录 fsync
    }
    Ok(())
}

/// 读 meta.json。
pub fn read_meta_file() -> Result<MetaFile> {
    let path = meta_path();
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读 meta.json 失败：{}", path.display()))?;
    let meta: MetaFile =
        serde_json::from_str(&content).context("meta.json JSON 解析失败")?;
    Ok(meta)
}

/// 写 meta.json（原子写，L4 修复）。
pub fn write_meta_file(meta: &MetaFile) -> Result<()> {
    let json = serde_json::to_string_pretty(meta).context("序列化 meta.json 失败")?;
    write_atomically(&meta_path(), &format!("{}\n", json))
}

/// 读 outline.json。文件不存在时返回默认空 outline（首次同步）。
pub fn read_outline_file() -> Result<Outline> {
    let path = outline_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let outline: Outline = serde_json::from_str(&content)
                .with_context(|| format!("outline.json JSON 解析失败：{}", path.display()))?;
            Ok(outline)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Outline::default()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("读 outline.json 失败：{}", path.display()))),
    }
}

/// 写 outline.json（原子写，L4 修复）。
pub fn write_outline_file(outline: &Outline) -> Result<()> {
    let json = serde_json::to_string_pretty(outline).context("序列化 outline.json 失败")?;
    write_atomically(&outline_path(), &format!("{}\n", json))
}

/// 读单个 cipher 文件。
pub fn read_cipher_file(uuid: &str) -> Result<CipherFile> {
    let path = cipher_file_path(uuid)?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读 cipher 文件失败：{}", path.display()))?;
    let cipher: CipherFile =
        serde_json::from_str(&content).context("cipher 文件 JSON 解析失败")?;
    Ok(cipher)
}

/// 写单个 cipher 文件（原子写，L4 修复）。
pub fn write_cipher_file(cipher: &VaultCipher) -> Result<()> {
    let path = cipher_file_path(&cipher.id)?;
    let file = CipherFile::from_vault_cipher(cipher);
    let json = serde_json::to_string_pretty(&file).context("序列化 cipher 文件失败")?;
    write_atomically(&path, &format!("{}\n", json))
}

/// 删除单个 cipher 文件（同步删除场景）。
pub fn delete_cipher_file(uuid: &str) -> Result<()> {
    let path = cipher_file_path(uuid)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("删 cipher 文件失败：{}", path.display()))),
    }
}

/// 读单个 folder 文件。
pub fn read_folder_file(uuid: &str) -> Result<FolderFile> {
    let path = folder_file_path(uuid)?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读 folder 文件失败：{}", path.display()))?;
    let folder: FolderFile =
        serde_json::from_str(&content).context("folder 文件 JSON 解析失败")?;
    Ok(folder)
}

/// 写单个 folder 文件（原子写，L4 修复）。
pub fn write_folder_file(folder: &VaultFolder) -> Result<()> {
    let path = folder_file_path(&folder.id)?;
    let file = FolderFile::from_vault_folder(folder);
    let json = serde_json::to_string_pretty(&file).context("序列化 folder 文件失败")?;
    write_atomically(&path, &format!("{}\n", json))
}

/// 删除单个 folder 文件。
pub fn delete_folder_file(uuid: &str) -> Result<()> {
    let path = folder_file_path(uuid)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("删 folder 文件失败：{}", path.display()))),
    }
}

// sha256_hex 已搬到 `octopus_sync::store::sha256_hex`（通用 hash 工具）

// === 全量导出/导入 ===

/// 从 SQLite 全量导出到文件系统——首次启用同步时用（push_initial）。
///
/// 步骤：
/// 1. 清空 ciphers/ 和 folders/ 目录（防 stale 文件残留）
/// 2. 写 meta.json
/// 3. 写所有 cipher / folder 文件
/// 4. 生成 outline.json（扫所有文件算 sha256）
pub fn export_all_to_files(
    meta: &VaultMeta,
    ciphers: &[VaultCipher],
    folders: &[VaultFolder],
) -> Result<Outline> {
    let root = vault_root();
    std::fs::create_dir_all(&root)
        .with_context(|| format!("创建 vault 目录失败：{}", root.display()))?;

    // 1. 清空 ciphers/ 和 folders/（保留 .git/ + meta.json + outline.json）
    let ciphers_dir = root.join("ciphers");
    let folders_dir = root.join("folders");
    if ciphers_dir.exists() {
        std::fs::remove_dir_all(&ciphers_dir).context("清空 ciphers/ 失败")?;
    }
    if folders_dir.exists() {
        std::fs::remove_dir_all(&folders_dir).context("清空 folders/ 失败")?;
    }

    // 2. 写 meta.json
    let meta_file = MetaFile::from_vault_meta(meta);
    write_meta_file(&meta_file)?;

    // 3. 写所有 cipher / folder 文件 + 收集 (uuid, md5)
    // BTreeMap 保证 outline.json 序列化顺序稳定（避免每次 sync 产生空 commit）
    let mut cipher_entries: std::collections::BTreeMap<String, OutlineEntry> = std::collections::BTreeMap::new();
    for c in ciphers {
        write_cipher_file(c)?;
        // md5 从 cipher.sync_md5 取（写命令时算好），fallback 临时算
        let md5 = c.sync_md5.clone().unwrap_or_else(|| crate::sync::fingerprint::cipher_md5(c));
        cipher_entries.insert(
            c.id.clone(),
            OutlineEntry {
                md5,
                updated_ms: sync_store::iso_to_unix_ms(&c.updated_at),
            },
        );
    }

    let mut folder_entries: std::collections::BTreeMap<String, OutlineEntry> = std::collections::BTreeMap::new();
    for f in folders {
        write_folder_file(f)?;
        let md5 = f.sync_md5.clone().unwrap_or_else(|| crate::sync::fingerprint::folder_md5(f));
        folder_entries.insert(
            f.id.clone(),
            OutlineEntry {
                md5,
                updated_ms: sync_store::iso_to_unix_ms(&f.updated_at),
            },
        );
    }

    // 4. 写 outline.json
    let outline = Outline {
        version: 1,
        vault_version: 1, // 首次导出从 1 开始
        ciphers: cipher_entries,
        folders: folder_entries,
    };
    write_outline_file(&outline)?;

    Ok(outline)
}

/// 增量导出——sync_now 用，只写真正变化的文件（不清空目录）。
///
/// 与 `export_all_to_files` 的区别：
/// - `export_all_to_files`：清空目录 + 全写（首次启用同步时用，目录为空无对比基础）
/// - `incremental_export`：读旧 outline + 对比 sync_md5 → 只写变化文件 + 删 SQLite 无的
///
/// outline.sha 字段值改用 SQLite 的 `sync_md5`（md5 内容指纹），而不是文件字节 sha256。
/// 这样跨设备同一条 cipher 的 outline.sha 相同（md5 是逻辑内容指纹）。
///
/// 返回 (new_outline, changed_count)——changed_count 是实际写/删的文件数。
pub fn incremental_export(
    meta: &VaultMeta,
    ciphers: &[VaultCipher],
    folders: &[VaultFolder],
) -> Result<(Outline, usize)> {
    let root = vault_root();
    std::fs::create_dir_all(&root)
        .with_context(|| format!("创建 vault 目录失败：{}", root.display()))?;

    // 1. 写 meta.json（每次都写——meta 变更频率极低，重写无浪费）
    let meta_file = MetaFile::from_vault_meta(meta);
    write_meta_file(&meta_file)?;

    // 2. 读旧 outline 做 diff
    // M8 修复（2026-07-24）：outline.json 解析失败时不再 unwrap_or_default 吞成空——
    // 那样删除循环遍历空 outline → 不执行 → SQLite 已删的 cipher 文件永久残留 → clone 复活。
    // Q1 修复（2026-07-24）：read_outline_file 已把 NotFound 转成 Ok(default)，
    // 故此处 Err 必为解析错/IO 错——直接降级全量重建，无需再判 NotFound（删死分支）。
    let old_outline = match read_outline_file() {
        Ok(o) => o,
        Err(e) => {
            log::warn!(
                "[sync] outline.json 解析失败，降级为全量重建（清理所有 stale 文件）：{}",
                e
            );
            // 全量重建——所有文件都算 changed
            let outline = export_all_to_files(meta, ciphers, folders)?;
            return Ok((outline, ciphers.len() + folders.len()));
        }
    };
    let mut changed = 0usize;

    // 3. cipher：对比 md5，只写变化的
    let mut cipher_entries: std::collections::BTreeMap<String, OutlineEntry> = std::collections::BTreeMap::new();
    let cipher_id_set: std::collections::HashSet<&str> = ciphers.iter().map(|c| c.id.as_str()).collect();
    for c in ciphers {
        let new_md5 = c.sync_md5.clone().unwrap_or_else(|| {
            // sync_md5 为 None（旧库迁移未回填）——临时算
            crate::sync::fingerprint::cipher_md5(c)
        });
        let old_entry = old_outline.ciphers.get(&c.id);
        let needs_write = match old_entry {
            None => true, // 新增
            Some(old) => old.md5 != new_md5, // md5 变了
        };
        if needs_write {
            write_cipher_file(c)?;
            changed += 1;
        }
        cipher_entries.insert(
            c.id.clone(),
            OutlineEntry {
                md5: new_md5,
                updated_ms: sync_store::iso_to_unix_ms(&c.updated_at),
            },
        );
    }
    // 删 SQLite 无但 outline 有的 cipher 文件。
    // ⚠️ 保护（2026-07-27 sync 覆盖 bug 修复）：当 DB 完全空（0 cipher + 0 folder）
    // 且 .sync outline 有数据时，跳过删除——这种状态几乎肯定是异常（清库/迁移），
    // 不应把空状态传播到 .sync 覆盖已有数据。用户真想清空应 disable_sync 或逐条软删。
    let db_all_empty = ciphers.is_empty() && folders.is_empty();
    let sync_has_data = !old_outline.ciphers.is_empty() || !old_outline.folders.is_empty();
    if db_all_empty && sync_has_data {
        log::warn!(
            "[sync] DB 完全空但 .sync outline 有数据（ciphers={}, folders={}）——跳过删除，防止空 DB 覆盖。如需清空请 disable_sync",
            old_outline.ciphers.len(),
            old_outline.folders.len()
        );
        // 仍然写新 outline（反映 DB 当前状态），但不删文件
        // —— 实际上 DB 空 → cipher_entries/folder_entries 都是空 map
        //    → 写空 outline 会与未删的文件不一致，但 git commit 时 outline 是「DB 视角」
        //    下次 pull 会从文件重新填充 outline。优先保数据不丢。
    } else {
        for old_uuid in old_outline.ciphers.keys() {
            if !cipher_id_set.contains(old_uuid.as_str()) {
                let _ = delete_cipher_file(old_uuid); // 文件可能已不存在，忽略错误
                changed += 1;
            }
        }
    }

    // 4. folder：同
    let mut folder_entries: std::collections::BTreeMap<String, OutlineEntry> = std::collections::BTreeMap::new();
    let folder_id_set: std::collections::HashSet<&str> = folders.iter().map(|f| f.id.as_str()).collect();
    for f in folders {
        let new_md5 = f.sync_md5.clone().unwrap_or_else(|| {
            crate::sync::fingerprint::folder_md5(f)
        });
        let old_entry = old_outline.folders.get(&f.id);
        let needs_write = match old_entry {
            None => true,
            Some(old) => old.md5 != new_md5,
        };
        if needs_write {
            write_folder_file(f)?;
            changed += 1;
        }
        folder_entries.insert(
            f.id.clone(),
            OutlineEntry {
                md5: new_md5,
                updated_ms: sync_store::iso_to_unix_ms(&f.updated_at),
            },
        );
    }
    // folder 删除保护（同 cipher，行 610-629）——db_all_empty && sync_has_data 时已跳过。
    if !(db_all_empty && sync_has_data) {
        for old_uuid in old_outline.folders.keys() {
            if !folder_id_set.contains(old_uuid.as_str()) {
                let _ = delete_folder_file(old_uuid);
                changed += 1;
            }
        }
    }

    // 5. 写新 outline
    // vault_version 只在 changed > 0 时 +1（用户反馈：无变化 sync 不应递增版本）
    let new_vault_version = if changed > 0 {
        old_outline.vault_version.wrapping_add(1)
    } else {
        old_outline.vault_version
    };
    let outline = Outline {
        version: 1,
        vault_version: new_vault_version,
        ciphers: cipher_entries,
        folders: folder_entries,
    };
    write_outline_file(&outline)?;

    Ok((outline, changed))
}

/// 从文件系统全量导入——clone_initial（B 机首次同步）时用。
///
/// 扫描 `~/.octopus/.vault/` 下所有 cipher / folder 文件，返回内存结构供上层
/// upsert 到 SQLite。
///
/// **注意**：meta.json 由调用方单独读（需要先派生 master_root_key 解密验证
/// security_stamp 后才能信任 meta）。
pub fn import_all_from_files() -> Result<(Vec<VaultCipher>, Vec<VaultFolder>)> {
    let root = vault_root();
    let ciphers_dir = root.join("ciphers");
    let folders_dir = root.join("folders");

    // R-IMPORT-NOFAULT-TOLERANT 修复（2026-07-25）：单文件损坏不再中止整个 import。
    //
    // 之前单文件 read/parse 失败直接 ? 中止 → clone_initial 连锁死锁：
    //   meta 已在 import 前写入 DB（engine.rs:462）→ import 失败 → clone Err
    //   → DB 半初始化（有 meta 无 cipher）→ E2 守卫阻止重试（:419）→ 用户死锁。
    //
    // 与 pull #10（engine.rs:813-825 read_cipher_file 容错）+ hotword 导入
    //（engine.rs:478-491 log 不阻断）对齐——三者都处理「外部文件导入」应统一容错。
    let mut ciphers = Vec::new();
    if ciphers_dir.exists() {
        for entry in walk_json_files(&ciphers_dir)? {
            match std::fs::read_to_string(&entry)
                .with_context(|| format!("读 cipher 文件失败：{}", entry.display()))
                .and_then(|content| {
                    serde_json::from_str::<CipherFile>(&content)
                        .with_context(|| format!("解析 cipher 文件失败：{}", entry.display()))
                }) {
                Ok(file) => ciphers.push(file.to_vault_cipher()),
                Err(e) => {
                    log::warn!("[sync] clone import：cipher 文件跳过（损坏）：{}", e);
                }
            }
        }
    }

    let mut folders = Vec::new();
    if folders_dir.exists() {
        for entry in walk_json_files(&folders_dir)? {
            match std::fs::read_to_string(&entry)
                .with_context(|| format!("读 folder 文件失败：{}", entry.display()))
                .and_then(|content| {
                    serde_json::from_str::<FolderFile>(&content)
                        .with_context(|| format!("解析 folder 文件失败：{}", entry.display()))
                }) {
                Ok(file) => folders.push(file.to_vault_folder()),
                Err(e) => {
                    log::warn!("[sync] clone import：folder 文件跳过（损坏）：{}", e);
                }
            }
        }
    }

    Ok((ciphers, folders))
}

/// 递归遍历目录下所有 .json 文件（按路径排序，结果稳定）。
fn walk_json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_json_files(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 临时改 vault_root 到 tempdir——测试结束后自动恢复（thread_local override）。
    struct VaultRootGuard {
        _tmp: TempDir,
    }

    impl VaultRootGuard {
        fn new() -> Self {
            let tmp = TempDir::new().expect("create tempdir");
            // sync_root 指向 tempdir/.sync（与生产路径结构一致）
            // vault_dir = sync_root/vault，文件操作自动走子目录
            let sync_path = tmp.path().join(".sync");
            set_test_vault_root(sync_path);
            Self { _tmp: tmp }
        }
    }

    impl Drop for VaultRootGuard {
        fn drop(&mut self) {
            clear_test_vault_root();
        }
    }

    fn sample_vault_meta() -> VaultMeta {
        VaultMeta {
            id: 1,
            kdf_type: 0,
            kdf_salt: vec![1u8; 32],
            kdf_iterations: 3,
            kdf_memory_kib: 65536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:dummy-uvk".into(),
            app_key_local_enc: "v1:dummy-local".into(),
            app_key_sync_enc: "v1:dummy-sync".into(),
            security_stamp: "stamp-1".into(),
            equivalent_domains: "[]".into(),
            public_key: None,
            protected_private_key: None,
            created_at: "2026-07-21T00:00:00".into(),
            updated_at: "2026-07-21T00:00:00".into(),
        }
    }

    fn sample_cipher(id: &str) -> VaultCipher {
        VaultCipher {
            id: id.to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:enc-name".into(),
            notes: None,
            data: "v1:enc-data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            deleted_at: None,
            sync_md5: None,
            created_at: "2026-07-21T10:00:00".into(),
            updated_at: "2026-07-21T10:00:00".into(),
        }
    }

    // shard_dir 测试已随函数搬到 octopus_sync::store

    #[test]
    fn cipher_file_path_uses_shard() {
        let _g = VaultRootGuard::new();
        let p = cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").expect("path");
        assert!(p.to_string_lossy().contains("ciphers/a1/"));
        assert!(p.to_string_lossy().ends_with(".json"));
    }

    #[test]
    fn meta_round_trip_through_file() {
        let _g = VaultRootGuard::new();
        let meta = sample_vault_meta();
        let meta_file = MetaFile::from_vault_meta(&meta);
        write_meta_file(&meta_file).expect("write");
        let loaded = read_meta_file().expect("read");
        assert_eq!(loaded.protected_user_vault_key, "v1:dummy-uvk");
        assert_eq!(loaded.app_key_sync_enc, "v1:dummy-sync");
        assert_eq!(loaded.security_stamp, "stamp-1");
        // base64 salt round trip（to_sync_fields 现在返 MetaSyncFields struct，#9 修复）
        let f = loaded.to_sync_fields().expect("to_sync_fields");
        assert_eq!(f.kdf_type, 0);
        assert_eq!(f.kdf_salt, vec![1u8; 32]);
    }

    /// #9 修复：kdf_salt base64 解码失败不再 unwrap_or_default 吞错——显式报错。
    #[test]
    fn to_sync_fields_errors_on_invalid_base64_salt() {
        let _g = VaultRootGuard::new();
        let meta = MetaFile {
            version: 1,
            kdf_type: 0,
            kdf_salt: "!!!not valid base64!!!".into(), // 非法 base64
            kdf_iterations: 3,
            kdf_memory_kib: 65536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:dummy".into(),
            app_key_sync_enc: "v1:dummy".into(),
            security_stamp: "stamp".into(),
            equivalent_domains: "[]".into(),
        };
        let result = meta.to_sync_fields();
        assert!(
            result.is_err(),
            "#9：非法 base64 salt 应报错，不应 unwrap_or_default 吞成空 Vec"
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("base64 解码失败"));
    }

    #[test]
    fn cipher_file_round_trip() {
        let _g = VaultRootGuard::new();
        let cipher = sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456");
        write_cipher_file(&cipher).expect("write");
        let loaded = read_cipher_file(&cipher.id).expect("read");
        assert_eq!(loaded.id, cipher.id);
        assert_eq!(loaded.encrypted.name, "v1:enc-name");
        // VaultCipher round trip
        let back = loaded.to_vault_cipher();
        assert_eq!(back.name, cipher.name);
        assert_eq!(back.data, cipher.data);
    }

    #[test]
    fn delete_missing_cipher_file_is_ok() {
        let _g = VaultRootGuard::new();
        // 文件不存在不应 Err
        delete_cipher_file("nonexistent-uuid").expect("delete missing should be Ok");
    }

    #[test]
    fn outline_read_returns_default_when_missing() {
        let _g = VaultRootGuard::new();
        let outline = read_outline_file().expect("read");
        assert_eq!(outline.version, 1);
        assert_eq!(outline.vault_version, 0);
        assert!(outline.ciphers.is_empty());
    }

    #[test]
    fn export_all_then_import_round_trips() {
        let _g = VaultRootGuard::new();
        let meta = sample_vault_meta();
        let ciphers = vec![
            sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456"),
            sample_cipher("b2c3d4e5-f6a7-4890-9002-bcdef234567"),
        ];
        let folders = vec![VaultFolder {
            id: "c3d4e5f6-a7b8-4901-9003-cdefg345678".into(),
            name: "v1:enc-folder".into(),
            sort_order: 0,
            sync_md5: None,
            created_at: "2026-07-21T00:00:00".into(),
            updated_at: "2026-07-21T00:00:00".into(),
        }];

        let outline = export_all_to_files(&meta, &ciphers, &folders).expect("export");
        assert_eq!(outline.ciphers.len(), 2);
        assert_eq!(outline.folders.len(), 1);

        // 导入回来
        let (loaded_ciphers, loaded_folders) = import_all_from_files().expect("import");
        assert_eq!(loaded_ciphers.len(), 2);
        assert_eq!(loaded_folders.len(), 1);
        // cipher id 不变
        let ids: Vec<&str> = loaded_ciphers.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"a1b2c3d4-e5f6-4789-8901-abcdef123456"));
        assert!(ids.contains(&"b2c3d4e5-f6a7-4890-9002-bcdef234567"));
    }

    /// R-IMPORT-NOFAULT-TOLERANT 守护（2026-07-25）：单文件损坏不中止整个 import。
    ///
    /// 之前单文件 read/parse 失败直接 ? 中止 → clone_initial 连锁死锁
    ///（meta 已写入 DB + cipher 未导入 + E2 阻止重试）。现与 pull #10 / hotword
    /// 容错对齐——损坏文件 log::warn 跳过，其他文件仍导入。
    #[test]
    fn import_all_from_files_skips_corrupt_file() {
        let _g = VaultRootGuard::new();
        // 写 2 个正常 cipher 文件
        write_cipher_file(&sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456"))
            .expect("write good 1");
        write_cipher_file(&sample_cipher("b2c3d4e5-f6a7-4890-9002-bcdef234567"))
            .expect("write good 2");

        // 手写 1 个损坏 JSON 文件（合法路径，非法内容）
        let corrupt_path = cipher_file_path("cccccccc-1111-4222-8333-cccccccccccc").expect("path");
        if let Some(parent) = corrupt_path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&corrupt_path, "{ this is not valid json }}}")
            .expect("write corrupt");

        // import 应成功（不中止），返回 2 个正常 cipher（损坏的被跳过）
        let (ciphers, _folders) = import_all_from_files().expect("import 不应因单文件损坏失败");
        assert_eq!(
            ciphers.len(),
            2,
            "R-IMPORT-NOFAULT-TOLERANT: 损坏文件应跳过，2 个正常的仍导入，实际 {}",
            ciphers.len()
        );
        // 确认是 2 个正常的（非损坏的）
        let ids: Vec<&str> = ciphers.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"a1b2c3d4-e5f6-4789-8901-abcdef123456"));
        assert!(ids.contains(&"b2c3d4e5-f6a7-4890-9002-bcdef234567"));
    }

    /// E-PATH-TRAVERSAL-OUTLINE-UUID 守护（2026-07-26）：恶意 uuid 触发 path traversal
    /// 被 cipher_file_path/folder_file_path 的 validate_uuid 拦截。
    ///
    /// 攻击者污染远程 outline 的 uuid（如 ../../meta）→ read/delete 路径读/删
    /// vault_root 外文件。validate_uuid 在路径构造入口拒绝 path traversal 字符。
    #[test]
    fn path_traversal_uuid_rejected() {
        // 合法 UUID → Ok
        assert!(cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").is_ok());
        assert!(folder_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").is_ok());
        // 简短 id（测试用）→ Ok（不含 path 字符）
        assert!(cipher_file_path("test-uuid").is_ok());

        // path traversal 尝试 → Err
        let malicious_uuids = [
            "../../meta",           // 跳出 ciphers/ 到 vault_root
            "..\\..\\meta",         // Windows 风格
            "../../../etc/passwd",  // 跳出 vault_root
            "/etc/passwd",          // 绝对路径
            "a/../../b",            // 混合
            "",                     // 空串
        ];
        for uuid in &malicious_uuids {
            assert!(
                cipher_file_path(uuid).is_err(),
                "cipher_file_path 应拒绝恶意 uuid：{}",
                uuid
            );
            assert!(
                folder_file_path(uuid).is_err(),
                "folder_file_path 应拒绝恶意 uuid：{}",
                uuid
            );
        }
    }

    // sha256_hex 测试已随函数搬到 octopus_sync::store

    // === incremental_export 测试（2026-07-21 md5 增量同步） ===

    use crate::sync::fingerprint::cipher_md5;

    /// 增量 export：相同数据二次 export 应 0 变更（不重写文件）。
    #[test]
    fn incremental_export_zero_changes_on_unchanged_data() {
        let _g = VaultRootGuard::new();
        let meta = sample_vault_meta();
        let ciphers = vec![sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456")];

        // 首次 export——全写
        let (_, changed1) = incremental_export(&meta, &ciphers, &[]).expect("first export");
        assert_eq!(changed1, 1, "首次应写 1 个 cipher 文件");

        // 二次 export——数据相同应 0 变更
        let (_, changed2) = incremental_export(&meta, &ciphers, &[]).expect("second export");
        assert_eq!(
            changed2, 0,
            "数据未变时增量 export 应 0 变更（md5 一致跳过）"
        );
    }

    /// 增量 export：改 1 条 cipher 只写 1 个文件。
    #[test]
    fn incremental_export_writes_only_changed_cipher() {
        let _g = VaultRootGuard::new();
        let meta = sample_vault_meta();
        let c1 = sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456");
        let c2 = sample_cipher("b2c3d4e5-f6a7-4890-9002-bcdef234567");

        // 首次写 2 个
        let (_, changed1) = incremental_export(&meta, &[c1.clone(), c2.clone()], &[]).expect("first");
        assert_eq!(changed1, 2);

        // 改 c1 的 name → c1 md5 变；c2 不动
        let mut c1_modified = c1.clone();
        c1_modified.name = "v1:different-name".into();
        c1_modified.sync_md5 = Some(cipher_md5(&c1_modified));
        let (_, changed2) =
            incremental_export(&meta, &[c1_modified, c2], &[]).expect("second");
        assert_eq!(
            changed2, 1,
            "只改 1 条 cipher 应只写 1 个文件，实际 {}", changed2
        );
    }

    /// 增量 export：SQLite 无但 outline 有 → 删文件。
    #[test]
    fn incremental_export_deletes_missing_cipher_file() {
        let _g = VaultRootGuard::new();
        let meta = sample_vault_meta();
        let c1 = sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456");
        let c2 = sample_cipher("b2c3d4e5-f6a7-4890-9012-bcdef234567");

        // 首次写 c1 + c2
        let (_, _) = incremental_export(&meta, &[c1.clone(), c2.clone()], &[]).expect("first");
        assert!(cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").unwrap().exists());
        assert!(cipher_file_path("b2c3d4e5-f6a7-4890-9012-bcdef234567").unwrap().exists());

        // 二次：SQLite 只剩 c2（c1 被删）→ c1 文件应被删
        // 注意：DB 非空（有 c2），所以删除保护不触发，正常删除 c1
        let (_, changed) = incremental_export(&meta, &[c2.clone()], &[]).expect("second");
        assert_eq!(changed, 1, "应删 1 个文件（c1）");
        assert!(
            !cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").unwrap().exists(),
            "SQLite 无的 cipher 文件应被删"
        );
        assert!(
            cipher_file_path("b2c3d4e5-f6a7-4890-9012-bcdef234567").unwrap().exists(),
            "SQLite 有的 cipher 文件应保留"
        );
    }

    /// 回归守护（2026-07-27 sync 覆盖 bug）：DB 完全空 + .sync outline 有数据时，
    /// 不删除 .sync 文件——防止清库后空 DB 覆盖 .sync 已有数据。
    #[test]
    fn incremental_export_protects_sync_data_when_db_empty() {
        let _g = VaultRootGuard::new();
        let meta = sample_vault_meta();
        let c1 = sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456");

        // 首次写 c1（.sync 有数据）
        let (_, _) = incremental_export(&meta, &[c1], &[]).expect("first");
        assert!(cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").unwrap().exists());

        // 二次：DB 完全空（清库场景）→ 不应删 c1 文件
        let (_, changed) = incremental_export(&meta, &[], &[]).expect("second");
        assert_eq!(changed, 0, "DB 空 + .sync 有数据时不应删任何文件");
        assert!(
            cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").unwrap().exists(),
            "DB 空时 .sync 的 cipher 文件应保留（防止覆盖）"
        );
    }

    /// M8 回归守护：outline.json 解析失败时降级为全量重建——
    /// stale 文件（SQLite 已删但 outline 损坏前残留的）被 remove_dir_all 清理。
    ///
    /// 之前 bug：unwrap_or_default 吞解析错成空 Outline → 删除循环不执行 →
    /// stale 文件永久残留 → 新设备 clone 复活已删密码。
    #[test]
    fn incremental_export_degraded_rebuild_on_corrupt_outline() {
        let _g = VaultRootGuard::new();
        let meta = sample_vault_meta();
        let c1 = sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456");

        // 首次写 c1（生成正常 outline）
        let (_, _) = incremental_export(&meta, &[c1], &[]).expect("first");
        assert!(cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").unwrap().exists());

        // 破坏 outline.json（写入非法 JSON）
        let outline_p = outline_path();
        std::fs::write(&outline_p, "this is not valid json {{{{").unwrap();

        // 二次：SQLite 无 cipher（c1 已删）→ outline 损坏 → 应降级全量重建
        // 全量重建的 remove_dir_all 会清理 c1 的 stale 文件
        // changed 可能是 0（SQLite 无 cipher/folder 写入），但关键断言是 stale 文件被清理
        let (_, _changed) = incremental_export(&meta, &[], &[]).expect("degraded rebuild");

        // M8 核心断言：c1 的 stale 文件应被清理（不残留 → clone 不复活）
        assert!(
            !cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").unwrap().exists(),
            "M8: outline 损坏时降级全量重建应清理 stale cipher 文件（之前永久残留 → clone 复活）"
        );

        // outline.json 应被重写为有效 JSON（全量重建后）
        let outline = read_outline_file().expect("outline should be valid after rebuild");
        assert!(
            outline.ciphers.is_empty(),
            "重建后 outline 应无 cipher（SQLite 为空）"
        );
    }

    /// 增量 export：outline.sha 用 SQLite sync_md5（md5 内容指纹）。
    #[test]
    fn incremental_export_uses_sync_md5_in_outline() {
        let _g = VaultRootGuard::new();
        let meta = sample_vault_meta();
        let mut c1 = sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456");
        c1.sync_md5 = Some(cipher_md5(&c1));

        let (outline, _) = incremental_export(&meta, &[c1.clone()], &[]).expect("export");
        let entry = outline.ciphers.get("a1b2c3d4-e5f6-4789-8901-abcdef123456").unwrap();
        assert_eq!(
            entry.md5, c1.sync_md5.unwrap(),
            "outline.md5 应等于 cipher.sync_md5（md5 内容指纹）"
        );
    }

    /// vault_version 只在 changed > 0 时 +1（用户反馈：无变化 sync 不应递增版本）。
    #[test]
    fn incremental_export_vault_version_only_increments_on_change() {
        let _g = VaultRootGuard::new();
        let meta = sample_vault_meta();
        let c1 = sample_cipher("a1b2c3d4-e5f6-4789-8901-abcdef123456");

        // 首次——有变化，version 应 +1（从 0 → 1）
        let (outline1, changed1) = incremental_export(&meta, &[c1.clone()], &[]).expect("first");
        assert_eq!(changed1, 1);
        assert_eq!(outline1.vault_version, 1, "首次有变化应 +1");

        // 二次——无变化，version 应保持 1（不递增）
        let (outline2, changed2) = incremental_export(&meta, &[c1], &[]).expect("second");
        assert_eq!(changed2, 0);
        assert_eq!(
            outline2.vault_version, 1,
            "无变化时 vault_version 不应递增（用户反馈：每次同步都 +1 是 bug）"
        );
    }
}
