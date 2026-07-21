//! 同步引擎——编排 fetch → merge → 文件系统 ↔ SQLite 双向同步 → commit → push。
//!
//! 流程概览（详见 spec §4）：
//!
//! ```text
//! sync_now():
//!   1. try_lock SyncState（失败返「同步进行中」）
//!   2. 清理崩溃残留（merge/rebase in-progress）
//!   3. git fetch --all
//!   4. git merge --ff-only origin/main
//!      - 成功 → 继续（远程有更新已 ff 到本地）
//!      - 不能 ff → rebase 兜底（§4.4）
//!   5. **pull 阶段**：文件系统 → SQLite
//!      - 读 outline.json，对比本地 outline
//!      - 按 sha 差异读 cipher/folder 文件 → upsert SQLite
//!      - 删除 outline 中不存在的 cipher（软删）
//!   6. **push 阶段**：SQLite → 文件系统
//!      - 读 SQLite 最新 ciphers/folders/meta
//!      - 写文件系统
//!      - 更新 outline（vault_version++）
//!   7. git add -A && git commit -m "sync"
//!   8. git push origin main（+ gitee if configured）
//! ```

use std::sync::OnceLock;

use anyhow::{Context, Result};
use octopus_infra::db::{self, VaultCipherInput, VaultMetaInput};

use crate::sync::error::SyncError;
use crate::sync::git;
use crate::sync::store;

// === T4.1: SyncState 进程内锁 ===

/// 同步状态——防止并发同步（用户连点同步按钮 / Tauri 命令并发）。
///
/// 不做跨进程锁——单实例 app 已有 tauri_plugin_single_instance 保证只有一个
/// octopus 进程。
static SYNC_LOCK: OnceLock<std::sync::Mutex<bool>> = OnceLock::new();

fn sync_lock() -> &'static std::sync::Mutex<bool> {
    SYNC_LOCK.get_or_init(|| std::sync::Mutex::new(false))
}

/// 尝试获取同步锁——返 Ok(guard) 成功，Err 表示同步进行中。
pub fn try_sync_lock() -> Result<std::sync::MutexGuard<'static, bool>, SyncError> {
    sync_lock()
        .try_lock()
        .map_err(|_| SyncError::Other(anyhow::anyhow!("同步正在进行中，请稍后再试")))
}

// === T4.2: SyncStatus ===

/// 同步状态——UI 显示用。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncStatus {
    /// git 是否可用
    pub git_available: bool,
    /// `~/.octopus/.vault/` 是否已初始化（.git 存在）
    pub initialized: bool,
    /// 配置的 remotes（name → url）
    pub remotes: Vec<(String, String)>,
    /// 最近一次 commit 的 ISO 时间戳（如果有）
    pub last_sync: Option<String>,
    /// 最近一次 commit 的 SHA（如果有）
    pub last_commit_sha: Option<String>,
}

/// 查询同步状态——UI 初始化时调用。
pub fn get_sync_status() -> SyncStatus {
    let git_available = git::check_git_available();
    let root = store::vault_root();

    if !git_available || !root.exists() || !git::is_git_repo(&root) {
        return SyncStatus {
            git_available,
            initialized: false,
            remotes: vec![],
            last_sync: None,
            last_commit_sha: None,
        };
    }

    let remotes = git::git_remote_list(&root).unwrap_or_default();
    let (last_commit_sha, last_sync) =
        match git::git_last_commit_info(&root).ok().flatten() {
            Some((sha, ts)) => (Some(sha), Some(ts)),
            None => (None, None),
        };

    SyncStatus {
        git_available,
        initialized: true,
        remotes,
        last_commit_sha,
        last_sync,
    }
}

// === T4.3: test_connection ===

/// 测试远程连接——`git ls-remote --heads <url>`。
///
/// 成功 Ok(())，失败返 SyncError（SSH / 网络 / URL 错误分类）。
pub fn test_connection(remote_url: &str) -> Result<(), SyncError> {
    git::git_ls_remote(remote_url)?;
    Ok(())
}

// === T4.4-T4.5: enable_sync / push_initial ===

/// 启用同步——检测远程状态决定走 push_initial（A 机首次）还是 clone_initial（B 机首次）。
///
/// 参数：
/// - `remote_url`：主 remote URL（如 `git@github.com:user/vault.git`）
/// - `gitee_url`：可选 Gitee mirror URL
pub fn enable_sync(remote_url: &str, gitee_url: Option<&str>) -> Result<(), SyncError> {
    if !git::check_git_available() {
        return Err(SyncError::GitNotInstalled);
    }

    let root = store::vault_root();
    if root.exists() && git::is_git_repo(&root) {
        return Err(SyncError::Other(anyhow::anyhow!(
            "同步已初始化，请先禁用同步再重新启用"
        )));
    }

    // 检测远程是否有数据
    let remote_has_data = match git::git_ls_remote(remote_url)? {
        true => {
            // ls-remote 成功——进一步检查是否有 refs（空 repo 返空 stdout）
            // git_ls_remote 返 bool，实际需要看 stdout 是否非空
            // 重新跑一次看输出
            let output = std::process::Command::new("git")
                .args(["ls-remote", "--heads", remote_url])
                .output()
                .map_err(|e| SyncError::Other(anyhow::Error::from(e)))?;
            if output.status.success() {
                !String::from_utf8_lossy(&output.stdout).trim().is_empty()
            } else {
                false
            }
        }
        false => false,
    };

    if remote_has_data {
        // B 机首次：clone
        clone_initial(remote_url)?;
    } else {
        // A 机首次：push
        push_initial(remote_url)?;
    }

    // 配置 Gitee mirror（如果提供）
    if let Some(gitee) = gitee_url {
        git::git_remote_add(&root, "gitee", gitee)?;
    }

    Ok(())
}

/// A 机首次推送——从 SQLite 导出全部到文件系统 → git init + commit + push。
fn push_initial(remote_url: &str) -> Result<(), SyncError> {
    let root = store::vault_root();
    std::fs::create_dir_all(&root)
        .with_context(|| format!("创建 vault 目录失败：{}", root.display()))
        .map_err(SyncError::Other)?;

    // 1. 从 SQLite 读全部数据
    let meta = db::load_vault_meta()
        .map_err(SyncError::Other)?
        .ok_or_else(|| {
            SyncError::Other(anyhow::anyhow!(
                "vault 未初始化——请先设置主密码"
            ))
        })?;
    let ciphers = db::list_vault_ciphers().map_err(SyncError::Other)?;
    let folders = db::list_vault_folders().map_err(SyncError::Other)?;

    // 2. 导出到文件系统
    store::export_all_to_files(&meta, &ciphers, &folders)?;

    // 3. git init + commit + push
    git::git_init(&root)?;
    git::git_remote_add(&root, "origin", remote_url)?;
    git::git_add_all(&root)?;
    git::git_commit(&root, "init vault")?;
    git::git_push_set_upstream(&root, "origin", "main")?;

    log::info!("[sync] push_initial 完成：{} ciphers, {} folders", ciphers.len(), folders.len());
    Ok(())
}

// === T4.6: clone_initial ===

/// B 机首次同步——clone 远程仓库 → 文件导入 SQLite。
///
/// **注意**：clone 后用户必须输 master_password 解锁（前端流程），unlock 后
/// 才能解密 cipher。本函数只做 clone + 文件 → SQLite upsert，不做加密验证。
fn clone_initial(remote_url: &str) -> Result<(), SyncError> {
    let root = store::vault_root();
    // 确保父目录存在
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建父目录失败：{}", parent.display()))
            .map_err(SyncError::Other)?;
    }

    // 1. git clone（clone 会创建 .vault 目录）
    git::git_clone(remote_url, &root)?;

    // 2. 读 meta.json → upsert vault_meta
    let meta_file = store::read_meta_file()?;
    let (kdf_type, salt, iters, mem, par, uvk, app_sync, stamp, equiv) =
        meta_file.to_sync_fields();
    // clone_initial 时 vault_meta 可能还没初始化（B 机首次）——用 upsert 写入
    // app_key_local_enc / public_key 留空，用户解锁后 refresh_app_key_local_enc 会填
    let meta_input = VaultMetaInput {
        kdf_type,
        kdf_salt: salt,
        kdf_iterations: iters,
        kdf_memory_kib: mem,
        kdf_parallelism: par,
        protected_user_vault_key: uvk,
        app_key_local_enc: String::new(), // 留空——解锁后由 refresh_app_key_local_enc 填
        app_key_sync_enc: app_sync,
        security_stamp: stamp,
        equivalent_domains: equiv,
        public_key: None,
        protected_private_key: None,
    };
    db::upsert_vault_meta(&meta_input).map_err(SyncError::Other)?;

    // 3. 读所有 cipher/folder 文件 → upsert SQLite
    let (ciphers, folders) = store::import_all_from_files()?;
    for c in &ciphers {
        let input = VaultCipherInput {
            id: c.id.clone(),
            folder_id: c.folder_id.clone(),
            favorite: c.favorite,
            atype: c.atype,
            name: c.name.clone(),
            notes: c.notes.clone(),
            data: c.data.clone(),
            fields: c.fields.clone(),
            password_history: c.password_history.clone(),
            reprompt: c.reprompt,
        };
        // upsert：先 try update，失败则 insert
        upsert_cipher(&input)?;
    }
    for f in &folders {
        // upsert folder：先 try update name，失败则 insert
        upsert_folder(&f.id, &f.name)?;
    }

    log::info!(
        "[sync] clone_initial 完成：{} ciphers, {} folders 导入 SQLite",
        ciphers.len(),
        folders.len()
    );
    Ok(())
}

/// Upsert cipher——存在则 UPDATE，不存在则 INSERT。
fn upsert_cipher(input: &VaultCipherInput) -> Result<(), SyncError> {
    match db::load_vault_cipher(&input.id).map_err(SyncError::Other)? {
        Some(_) => {
            db::update_vault_cipher(&input.id, input).map_err(SyncError::Other)?;
        }
        None => {
            db::insert_vault_cipher(input).map_err(SyncError::Other)?;
        }
    }
    Ok(())
}

/// Upsert folder——存在则 rename，不存在则 insert。
fn upsert_folder(id: &str, encrypted_name: &str) -> Result<(), SyncError> {
    // infra 没有直接 upsert folder——用 list 检查存在性
    let exists = db::list_vault_folders()
        .map_err(SyncError::Other)?
        .iter()
        .any(|f| f.id == id);
    if exists {
        db::update_vault_folder_name(id, encrypted_name).map_err(SyncError::Other)?;
    } else {
        db::insert_vault_folder(id, encrypted_name).map_err(SyncError::Other)?;
    }
    Ok(())
}

// === T4.7: sync_now ===

/// 同步报告——sync_now 返回值，UI 显示「拉了 X 条，推了 Y 条」。
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct SyncReport {
    pub pulled: usize,
    pub pushed: usize,
    pub deleted: usize,
    pub message: String,
}

/// 手动触发同步——编排 pull_merge_push 流程。
///
/// Phase 1 唯一触发方式（手动按钮）。Phase 2 会加自动同步。
pub fn sync_now() -> Result<SyncReport, SyncError> {
    let _guard = try_sync_lock()?;

    if !git::check_git_available() {
        return Err(SyncError::GitNotInstalled);
    }

    let root = store::vault_root();
    if !git::is_git_repo(&root) {
        return Err(SyncError::RepoNotInitialized);
    }

    // 0. 清理崩溃残留
    git::cleanup_in_progress_ops(&root)?;

    // 1. fetch
    git::git_fetch_all(&root)?;

    // 2. merge --ff-only
    let merge_result = git::git_merge_ff(&root, "origin/main")?;
    if !merge_result {
        // 不能 ff → rebase 兜底
        log::info!("[sync] merge --ff-only 失败，走 rebase 路径");
        match git::git_rebase(&root, "origin/main") {
            Ok(()) => log::info!("[sync] rebase 成功"),
            Err(e) => {
                log::error!("[sync] rebase 失败，需手动介入：{}", e);
                return Err(e);
            }
        }
    }

    // 3. pull 阶段：文件系统 → SQLite
    let pulled = pull_from_files()?;

    // 4. push 阶段：SQLite → 文件系统
    let pushed = push_to_files()?;

    // 5. commit
    let root = store::vault_root();
    git::git_add_all(&root)?;
    let committed = git::git_commit(&root, "sync")?;

    // 6. push to origin
    if committed {
        git::git_push(&root, "origin", "main")?;
        // 如果配了 gitee，也 push
        let remotes = git::git_remote_list(&root).unwrap_or_default();
        if remotes.iter().any(|(name, _)| name == "gitee") {
            match git::git_push(&root, "gitee", "main") {
                Ok(()) => log::debug!("[sync] pushed to gitee"),
                Err(e) => log::warn!("[sync] gitee push 失败（origin 已成功）：{}", e),
            }
        }
    }

    let report = SyncReport {
        pulled,
        pushed,
        deleted: 0,
        message: if committed {
            format!("同步完成：拉取 {} 条，推送 {} 条", pulled, pushed)
        } else {
            "已是最新，无需同步".to_string()
        }
    };
    log::info!("[sync] sync_now 完成：{}", report.message);
    Ok(report)
}

/// Pull 阶段：读 outline.json 对比本地，按 sha 差异读文件 upsert SQLite。
///
/// 返回 upsert 的 cipher+folder 数量。
fn pull_from_files() -> Result<usize, SyncError> {
    let remote_outline = store::read_outline_file()?;
    // 本地 outline（从 SQLite 推算——读所有 cipher 算 sha）
    // 简化：直接对比 remote outline vs SQLite 中已有的 cipher
    let db_ciphers = db::list_vault_ciphers().map_err(SyncError::Other)?;
    let db_folders = db::list_vault_folders().map_err(SyncError::Other)?;

    let db_cipher_ids: std::collections::HashSet<&str> =
        db_ciphers.iter().map(|c| c.id.as_str()).collect();
    let db_folder_ids: std::collections::HashSet<&str> =
        db_folders.iter().map(|f| f.id.as_str()).collect();

    let mut count = 0;

    // cipher：outline 有但 DB 无，或 sha 不匹配 → 读文件 upsert
    for (uuid, _entry) in &remote_outline.ciphers {
        let needs_update = !db_cipher_ids.contains(uuid.as_str())
            || cipher_sha_mismatch(uuid, &db_ciphers);
        if needs_update {
            if let Ok(cipher_file) = store::read_cipher_file(uuid) {
                let row = cipher_file.to_vault_cipher();
                let input = VaultCipherInput {
                    id: row.id.clone(),
                    folder_id: row.folder_id.clone(),
                    favorite: row.favorite,
                    atype: row.atype,
                    name: row.name.clone(),
                    notes: row.notes.clone(),
                    data: row.data.clone(),
                    fields: row.fields.clone(),
                    password_history: row.password_history.clone(),
                    reprompt: row.reprompt,
                };
                upsert_cipher(&input)?;
                count += 1;
            }
        }
    }

    // folder：同
    for (uuid, _entry) in &remote_outline.folders {
        if !db_folder_ids.contains(uuid.as_str()) {
            if let Ok(folder_file) = store::read_folder_file(uuid) {
                upsert_folder(&folder_file.id, &folder_file.encrypted_name)?;
                count += 1;
            }
        }
    }

    // meta.json → upsert vault_meta（同步 KDF 参数 + sync keys）
    if let Ok(meta_file) = store::read_meta_file() {
        let (kdf_type, salt, iters, mem, par, uvk, app_sync, stamp, equiv) =
            meta_file.to_sync_fields();
        // 保留本地 app_key_local_enc / public_key——只更新同步字段
        let local_meta = db::load_vault_meta().map_err(SyncError::Other)?;
        let (local_enc, pub_key, priv_key) = match local_meta {
            Some(ref m) => (
                m.app_key_local_enc.clone(),
                m.public_key.clone(),
                m.protected_private_key.clone(),
            ),
            None => (String::new(), None, None),
        };
        let meta_input = VaultMetaInput {
            kdf_type,
            kdf_salt: salt,
            kdf_iterations: iters,
            kdf_memory_kib: mem,
            kdf_parallelism: par,
            protected_user_vault_key: uvk,
            app_key_local_enc: local_enc,
            app_key_sync_enc: app_sync,
            security_stamp: stamp,
            equivalent_domains: equiv,
            public_key: pub_key,
            protected_private_key: priv_key,
        };
        db::upsert_vault_meta(&meta_input).map_err(SyncError::Other)?;
    }

    Ok(count)
}

/// 检测 cipher 文件 sha 是否与 DB 中不匹配（需要更新）。
fn cipher_sha_mismatch(uuid: &str, db_ciphers: &[db::VaultCipher]) -> bool {
    // 简化：文件存在 + DB 有 → 比较 updated_at（文件更新则 sha 大概率变了）
    // 完整 sha 对比需要读文件算 sha——为了简化用 updated_at 比较
    let db_cipher = match db_ciphers.iter().find(|c| c.id == uuid) {
        Some(c) => c,
        None => return true, // DB 无 → 需要 pull
    };
    // 读文件看 updated_at 是否更新
    match store::read_cipher_file(uuid) {
        Ok(file) => file.plaintext_meta.updated_at > db_cipher.updated_at,
        Err(_) => false, // 文件读失败——不 pull（避免误删）
    }
}

/// Push 阶段：SQLite 最新数据 → 文件系统 + 更新 outline。
///
/// 返回写入的 cipher+folder 数量。
fn push_to_files() -> Result<usize, SyncError> {
    let meta = db::load_vault_meta()
        .map_err(SyncError::Other)?
        .ok_or_else(|| SyncError::Other(anyhow::anyhow!("vault_meta 不存在")))?;
    let ciphers = db::list_vault_ciphers().map_err(SyncError::Other)?;
    let folders = db::list_vault_folders().map_err(SyncError::Other)?;

    // 直接全量写（简化——cipher 数量通常 < 1000，全量写毫秒级）
    // outline 在 export_all_to_files 内部已生成
    let _outline = store::export_all_to_files(&meta, &ciphers, &folders)?;

    Ok(ciphers.len() + folders.len())
}

// === T4.9: disable_sync ===

/// 禁用同步——删除 `~/.octopus/.vault/`（保留 SQLite 数据）。
pub fn disable_sync() -> Result<(), SyncError> {
    let root = store::vault_root();
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .with_context(|| format!("删除 vault 目录失败：{}", root.display()))
            .map_err(SyncError::Other)?;
    }
    log::info!("[sync] 同步已禁用，本地 SQLite 数据保留");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_lock_serializes() {
        // 获取锁
        let _g = try_sync_lock().expect("first lock should succeed");
        // 第二次获取应失败
        let result = try_sync_lock();
        assert!(result.is_err(), "并发获取同步锁应失败");
    }

    #[test]
    fn get_sync_status_without_git_or_repo() {
        // 不依赖 git / repo 环境的状态查询不应 panic
        let _status = get_sync_status();
    }

    #[test]
    fn sync_report_default() {
        let r = SyncReport::default();
        assert_eq!(r.pulled, 0);
        assert_eq!(r.pushed, 0);
        assert_eq!(r.deleted, 0);
    }
}
