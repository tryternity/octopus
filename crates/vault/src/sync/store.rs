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
pub fn cipher_file_path(uuid: &str) -> PathBuf {
    vault_root()
        .join("ciphers")
        .join(sync_store::shard_dir(uuid))
        .join(format!("{}.json", uuid))
}

/// folder 文件路径：`folders/<前2hex>/<uuid>.json`（folder 也分桶）。
pub fn folder_file_path(uuid: &str) -> PathBuf {
    vault_root()
        .join("folders")
        .join(sync_store::shard_dir(uuid))
        .join(format!("{}.json", uuid))
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
    pub fn to_sync_fields(&self) -> (i64, Vec<u8>, i64, i64, i64, String, String, String, String) {
        use base64::Engine;
        let salt = base64::engine::general_purpose::STANDARD
            .decode(&self.kdf_salt)
            .unwrap_or_default();
        (
            self.kdf_type,
            salt,
            self.kdf_iterations,
            self.kdf_memory_kib,
            self.kdf_parallelism,
            self.protected_user_vault_key.clone(),
            self.app_key_sync_enc.clone(),
            self.security_stamp.clone(),
            self.equivalent_domains.clone(),
        )
    }
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

/// 读 meta.json。
pub fn read_meta_file() -> Result<MetaFile> {
    let path = meta_path();
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读 meta.json 失败：{}", path.display()))?;
    let meta: MetaFile =
        serde_json::from_str(&content).context("meta.json JSON 解析失败")?;
    Ok(meta)
}

/// 写 meta.json（pretty print，便于 git diff 可读）。
pub fn write_meta_file(meta: &MetaFile) -> Result<()> {
    let path = meta_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败：{}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(meta).context("序列化 meta.json 失败")?;
    std::fs::write(&path, format!("{}\n", json))
        .with_context(|| format!("写 meta.json 失败：{}", path.display()))?;
    Ok(())
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

/// 写 outline.json。
pub fn write_outline_file(outline: &Outline) -> Result<()> {
    let path = outline_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败：{}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(outline).context("序列化 outline.json 失败")?;
    std::fs::write(&path, format!("{}\n", json))
        .with_context(|| format!("写 outline.json 失败：{}", path.display()))?;
    Ok(())
}

/// 读单个 cipher 文件。
pub fn read_cipher_file(uuid: &str) -> Result<CipherFile> {
    let path = cipher_file_path(uuid);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读 cipher 文件失败：{}", path.display()))?;
    let cipher: CipherFile =
        serde_json::from_str(&content).context("cipher 文件 JSON 解析失败")?;
    Ok(cipher)
}

/// 写单个 cipher 文件（含分桶目录创建）。
pub fn write_cipher_file(cipher: &VaultCipher) -> Result<()> {
    let path = cipher_file_path(&cipher.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建桶目录失败：{}", parent.display()))?;
    }
    let file = CipherFile::from_vault_cipher(cipher);
    let json = serde_json::to_string_pretty(&file).context("序列化 cipher 文件失败")?;
    std::fs::write(&path, format!("{}\n", json))
        .with_context(|| format!("写 cipher 文件失败：{}", path.display()))?;
    Ok(())
}

/// 删除单个 cipher 文件（同步删除场景）。
pub fn delete_cipher_file(uuid: &str) -> Result<()> {
    let path = cipher_file_path(uuid);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("删 cipher 文件失败：{}", path.display()))),
    }
}

/// 读单个 folder 文件。
pub fn read_folder_file(uuid: &str) -> Result<FolderFile> {
    let path = folder_file_path(uuid);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("读 folder 文件失败：{}", path.display()))?;
    let folder: FolderFile =
        serde_json::from_str(&content).context("folder 文件 JSON 解析失败")?;
    Ok(folder)
}

/// 写单个 folder 文件。
pub fn write_folder_file(folder: &VaultFolder) -> Result<()> {
    let path = folder_file_path(&folder.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建桶目录失败：{}", parent.display()))?;
    }
    let file = FolderFile::from_vault_folder(folder);
    let json = serde_json::to_string_pretty(&file).context("序列化 folder 文件失败")?;
    std::fs::write(&path, format!("{}\n", json))
        .with_context(|| format!("写 folder 文件失败：{}", path.display()))?;
    Ok(())
}

/// 删除单个 folder 文件。
pub fn delete_folder_file(uuid: &str) -> Result<()> {
    let path = folder_file_path(uuid);
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
    let old_outline = read_outline_file().unwrap_or_default();
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
    // 删 SQLite 无但 outline 有的 cipher 文件
    for old_uuid in old_outline.ciphers.keys() {
        if !cipher_id_set.contains(old_uuid.as_str()) {
            let _ = delete_cipher_file(old_uuid); // 文件可能已不存在，忽略错误
            changed += 1;
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
    for old_uuid in old_outline.folders.keys() {
        if !folder_id_set.contains(old_uuid.as_str()) {
            let _ = delete_folder_file(old_uuid);
            changed += 1;
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

    let mut ciphers = Vec::new();
    if ciphers_dir.exists() {
        for entry in walk_json_files(&ciphers_dir)? {
            let content = std::fs::read_to_string(&entry)
                .with_context(|| format!("读文件失败：{}", entry.display()))?;
            let file: CipherFile = serde_json::from_str(&content)
                .with_context(|| format!("解析 cipher 文件失败：{}", entry.display()))?;
            ciphers.push(file.to_vault_cipher());
        }
    }

    let mut folders = Vec::new();
    if folders_dir.exists() {
        for entry in walk_json_files(&folders_dir)? {
            let content = std::fs::read_to_string(&entry)
                .with_context(|| format!("读文件失败：{}", entry.display()))?;
            let file: FolderFile = serde_json::from_str(&content)
                .with_context(|| format!("解析 folder 文件失败：{}", entry.display()))?;
            folders.push(file.to_vault_folder());
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
        let p = cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456");
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
        // base64 salt round trip
        let (kdf_type, salt, _, _, _, _, _, _, _) = loaded.to_sync_fields();
        assert_eq!(kdf_type, 0);
        assert_eq!(salt, vec![1u8; 32]);
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

        // 首次写 c1
        let (_, _) = incremental_export(&meta, &[c1], &[]).expect("first");
        assert!(cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").exists());

        // 二次：SQLite 无 cipher → 文件应被删
        let (_, changed) = incremental_export(&meta, &[], &[]).expect("second");
        assert_eq!(changed, 1, "应删 1 个文件");
        assert!(
            !cipher_file_path("a1b2c3d4-e5f6-4789-8901-abcdef123456").exists(),
            "SQLite 无的 cipher 文件应被删"
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
