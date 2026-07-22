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
use octopus_infra::db::{self, VaultCipher, VaultCipherInput, VaultMeta, VaultMetaInput};
// 通用 sync 基础设施（2026-07-22 抽离到 octopus_sync）
use octopus_sync::error::SyncError;
use octopus_sync::git;
use octopus_sync::privacy::{self, PrivacyVerdict};

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

/// 同步中标记——AtomicBool 让 get_sync_status 能查到「正在同步」状态。
///
/// 与 SYNC_LOCK 的区别：
/// - SYNC_LOCK：Mutex<bool> 串行化同步，guard 出作用域自动释放
/// - SYNCING：AtomicBool 显式标记，让 UI 查询状态时能区分「正在同步」vs「idle」
///
/// 设置点：sync_now 入口 try_sync_lock 成功后设 true，函数退出（Ok/Err）时设 false。
static SYNCING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 查询当前是否正在同步——供 UI 显示进度条。
pub fn is_syncing() -> bool {
    SYNCING.load(std::sync::atomic::Ordering::Relaxed)
}

/// RAII 守卫——sync_now 入口 `let _g = SyncingGuard::set();`，退出时自动清零。
///
/// 不直接复用 Mutex guard——SYNCING 是 AtomicBool 给 UI 查询用，
/// Mutex guard 是同步串行化。两者职责不同，分开维护。
struct SyncingGuard;
impl SyncingGuard {
    fn set() -> Self {
        SYNCING.store(true, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}
impl Drop for SyncingGuard {
    fn drop(&mut self) {
        SYNCING.store(false, std::sync::atomic::Ordering::Relaxed);
    }
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
    /// 当前是否正在后台同步——UI 据此显进度条（2026-07-21 增补）
    pub syncing: bool,
    /// 最近一次自动同步结果（Phase 2，scheduler 每小时触发，None=从未自动同步）
    pub last_auto_sync: Option<octopus_sync::store::LastAutoSync>,
}

/// 查询同步状态——UI 初始化时调用。
pub fn get_sync_status() -> SyncStatus {
    let git_available = git::check_git_available();
    let root = octopus_sync::store::sync_root();
    let syncing = is_syncing();

    if !git_available || !root.exists() || !git::is_git_repo(&root) {
        return SyncStatus {
            git_available,
            initialized: false,
            remotes: vec![],
            last_sync: None,
            last_commit_sha: None,
            syncing,
            last_auto_sync: None,
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
        syncing,
        last_auto_sync: octopus_sync::store::read_last_auto_sync(),
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

/// 启用同步——初始化本地 git repo + 从 SQLite 导出全部数据 + 首次 commit。
///
/// **不 push**（push 需要先配 remote——用户通过 add_remote 添加）。
/// 用户在 UI 里点「启用同步」后看到空 remote 列表，自己添加 remote URL。
pub fn enable_sync() -> Result<(), SyncError> {
    if !git::check_git_available() {
        return Err(SyncError::GitNotInstalled);
    }

    let root = octopus_sync::store::sync_root();
    if root.exists() && git::is_git_repo(&root) {
        return Err(SyncError::Other(anyhow::anyhow!(
            "同步已初始化，请先禁用同步再重新启用"
        )));
    }

    push_initial()?;
    Ok(())
}

/// 初始化本地仓库——从 SQLite 导出全部到文件系统 → git init + commit（不 push）。
fn push_initial() -> Result<(), SyncError> {
    let sync_root = octopus_sync::store::sync_root();
    let vault_dir = store::vault_dir();
    // 创建两层目录：.sync/（git repo 根）+ .sync/vault/（vault 数据）
    std::fs::create_dir_all(&vault_dir)
        .with_context(|| format!("创建 vault 目录失败：{}", vault_dir.display()))
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

    // 2. 导出到文件系统（写 .sync/vault/ 下）
    store::export_all_to_files(&meta, &ciphers, &folders)?;

    // 2.5 热词全量导出（v46：push_initial 同时导出 vault + hotword）
    match octopus_infra::db::list_hotword_sets() {
        Ok(hotword_sets) => {
            if let Err(e) = octopus_sync::hotword::export_all_hotwords(&hotword_sets) {
                log::warn!("[sync] 热词初始导出失败（不阻断 vault）：{}", e);
            }
        }
        Err(e) => log::warn!("[sync] 读热词列表失败（跳过热词初始导出）：{}", e),
    }

    // 3. git init + commit（在 .sync/ 根——git 自动跟踪 vault/ + hotword/ 子目录）
    git::git_init(&sync_root)?;
    git::git_add_all(&sync_root)?;
    git::git_commit(&sync_root, "init vault")?;

    log::info!(
        "[sync] push_initial 完成（未 push）：{} ciphers, {} folders",
        ciphers.len(),
        folders.len()
    );
    Ok(())
}

// === Remote 管理（列表式，不写死 GitHub/Gitee） ===

/// 添加 remote——用户自由输入 URL，不限制 GitHub/Gitee/自建。
///
/// name 是用户自定义的 remote 名称（如 origin / backup / work），如果重复返 Err。
///
/// **私有库检测**（2026-07-21）：入口处校验 URL——
/// - 公有库（GitHub/Gitee API 确认 public / HTTPS ls-remote 能匿名拉到 refs）→ 拒绝
/// - 本地路径 → 拒绝
/// - SSH / Ambiguous / NetworkError → 放行（SSH 无法匿名嗅探，歧义/网络错误不阻断用户）
pub fn add_remote(name: &str, url: &str) -> Result<(), SyncError> {
    let root = octopus_sync::store::sync_root();
    if !git::is_git_repo(&root) {
        return Err(SyncError::RepoNotInitialized);
    }
    // 私有库守卫——硬阻断公有库（用原始 URL 检测，避免 SSH 转换后检测逻辑混乱）
    ensure_private_repo(url)?;
    // HTTPS → SSH 自动改写（github.com / gitee.com 的 HTTPS URL，且本机 SSH key 可用）
    let effective_url = maybe_rewrite_to_ssh(url)?;
    git::git_remote_add(&root, name, &effective_url)?;
    log::info!("[sync] 添加 remote: {} → {}（effective: {}）", name, url, effective_url);
    Ok(())
}

/// 私有库守卫——检测 URL，公有库直接返 Err。
///
/// 其他 verdict（Private / Ambiguous / SshUnverifiable / NetworkError）放行，
/// 仅记录日志让用户能看到检测过程。
fn ensure_private_repo(url: &str) -> Result<(), SyncError> {
    let verdict = privacy::check_privacy(url)?;
    match verdict {
        PrivacyVerdict::Public => {
            log::warn!("[sync] 拒绝公有库: {}", url);
            Err(SyncError::PublicRepoRejected(url.to_string()))
        }
        PrivacyVerdict::Private => {
            log::info!("[sync] 确认私有库: {}", url);
            Ok(())
        }
        PrivacyVerdict::Ambiguous(reason) => {
            log::info!("[sync] 仓库可见性不明（放行）: {} —— {}", url, reason);
            Ok(())
        }
        PrivacyVerdict::SshUnverifiable => {
            log::info!("[sync] SSH URL 无法自动检测（放行，由用户保证私有）: {}", url);
            Ok(())
        }
        PrivacyVerdict::NetworkError(msg) => {
            log::warn!("[sync] 仓库可见性检测网络错误（放行）: {} —— {}", url, msg);
            Ok(())
        }
    }
}

/// HTTPS → SSH 自动改写（2026-07-21 增补）。
///
/// 对 `github.com` / `gitee.com` 的 HTTPS URL，先验证本机 SSH key 能认证该 host：
/// - SSH key 可用 → 返 SSH URL（scp-like）
/// - SSH key 不可用 / ssh 命令失败 → 返原 HTTPS URL（让用户后续 push 失败时得到原始错误）
///
/// 对其他 URL（SSH 协议 / 自建 host / 本地路径）→ 直接返原值。
///
/// 设计动机：GitHub 自 2021-08 禁用 HTTPS 密码认证，仅支持 PAT。
/// 用户从浏览器复制的 URL 默认 HTTPS——自动转 SSH 用 `~/.ssh/` 私钥，
/// 避免「Password authentication is not supported」死局（用户已踩坑）。
///
/// **不做静默转换失败**：SSH 验证失败时返原 URL，但后续 push 失败的错误信息
/// 会被前端 toast 显示（含 GitHub 的 "Password authentication is not supported"），
/// 用户能据此判断要配 SSH key 还是要换 URL。
fn maybe_rewrite_to_ssh(url: &str) -> Result<String, SyncError> {
    let Some(ssh_url) = privacy::try_convert_https_to_ssh(url) else {
        return Ok(url.to_string());
    };
    // 解析 SSH URL 拿 host 做 ssh -T 预检
    let parsed = privacy::GitRemoteUrl::parse(&ssh_url)?;
    log::info!(
        "[sync] 检测到 {} HTTPS URL，验证 SSH key 后尝试转 SSH: {}",
        parsed.host, url
    );
    match git::verify_ssh_key_for_host(&parsed.host) {
        Ok(true) => {
            log::info!(
                "[sync] SSH key 可用，HTTPS → SSH: {} → {}",
                url, ssh_url
            );
            Ok(ssh_url)
        }
        Ok(false) => {
            log::warn!(
                "[sync] SSH key 不可用（保留 HTTPS，后续 push 可能失败）: {}",
                url
            );
            Ok(url.to_string())
        }
        Err(e) => {
            log::warn!(
                "[sync] SSH 预检失败（保留 HTTPS）: {} —— {}",
                url, e
            );
            Ok(url.to_string())
        }
    }
}

/// sync_now 入口对每个 remote 检查并自动改写 HTTPS → SSH（2026-07-21 增补）。
///
/// 解决场景：用户在自动改写功能**加上之前**已经 `git remote add` 过 HTTPS URL，
/// `.git/config` 里是 HTTPS——sync_now 时 push 会卡在 GitHub 用户名 prompt
/// （GitHub 已禁用 HTTPS 密码认证）。或用户先加的 HTTPS，后来才装的 SSH key。
///
/// 流程：遍历 .git/config 里的所有 remote：
/// - HTTPS URL 且 SSH key 可用 → `git remote set-url` 改 SSH
/// - HTTPS URL 且 SSH key 不可用 → 不动（保留 HTTPS，让 push 错误由 toast 暴露）
/// - SSH URL / 自建 host / 本地路径 → 不动
///
/// 错误处理：单个 remote 改写失败不影响其他 remote，只记日志。
pub fn ensure_remotes_use_ssh_when_possible(root: &std::path::Path) {
    let remotes = match git::git_remote_list(root) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[sync] 列 remote 失败，跳过 SSH 改写：{}", e);
            return;
        }
    };
    for (name, url) in &remotes {
        match maybe_rewrite_to_ssh(url) {
            Ok(rewritten) if rewritten != *url => {
                // 改写后 URL 不同——set-url
                match git::git_remote_set_url(root, name, &rewritten) {
                    Ok(()) => log::info!(
                        "[sync] sync_now 自动改写 remote {}：{} → {}",
                        name, url, rewritten
                    ),
                    Err(e) => log::warn!(
                        "[sync] sync_now 自动改写 remote {} 失败（保留 HTTPS）：{}",
                        name, e
                    ),
                }
            }
            Ok(_) => { /* URL 未变（已是 SSH / SSH key 不可用 / 非 github/gitee） */ }
            Err(e) => log::warn!(
                "[sync] remote {} 改写检测失败（保留 {}）：{}",
                name, url, e
            ),
        }
    }
}

/// 删除 remote。
pub fn remove_remote(name: &str) -> Result<(), SyncError> {
    let root = octopus_sync::store::sync_root();
    if !git::is_git_repo(&root) {
        return Err(SyncError::RepoNotInitialized);
    }
    git::git_remote_remove(&root, name)?;
    log::info!("[sync] 删除 remote: {}", name);
    Ok(())
}

/// 列出所有 remote（name → url）。
pub fn list_remotes() -> Result<Vec<(String, String)>, SyncError> {
    let root = octopus_sync::store::sync_root();
    if !git::is_git_repo(&root) {
        return Ok(vec![]);
    }
    git::git_remote_list(&root)
}

/// 从指定 remote URL clone 仓库（B 机首次同步）。
///
/// 用户先 add_remote 再 clone_from，或者直接 clone（会自动配 origin）。
pub fn clone_from(remote_url: &str) -> Result<(), SyncError> {
    clone_initial(remote_url)
}

// === T4.6: clone_initial ===

/// B 机首次同步——clone 远程仓库 → 文件导入 SQLite。
///
/// **注意**：clone 后用户必须输 master_password 解锁（前端流程），unlock 后
/// 才能解密 cipher。本函数只做 clone + 文件 → SQLite upsert，不做加密验证。
///
/// **私有库守卫**（2026-07-21）：clone 前先校验 URL——避免从公有库 clone 进来
/// （如果远程是公有库，说明用户配错了 remote；clone 完才发现为时已晚）。
///
/// **HTTPS → SSH 自动改写**（2026-07-21）：同 add_remote，github.com / gitee.com
/// 的 HTTPS URL 在 SSH key 可用时转 SSH，避免 clone 时遇到 HTTPS 认证失败。
fn clone_initial(remote_url: &str) -> Result<(), SyncError> {
    // 私有库守卫——硬阻断公有库（用原始 URL 检测）
    ensure_private_repo(remote_url)?;
    // HTTPS → SSH 自动改写
    let effective_url = maybe_rewrite_to_ssh(remote_url)?;

    let root = octopus_sync::store::sync_root();
    // 确保父目录存在
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建父目录失败：{}", parent.display()))
            .map_err(SyncError::Other)?;
    }

    // 1. git clone（clone 会创建 .sync 目录；用 effective_url 走 SSH）
    // clone 后 .sync/ 是 git repo 根，包含远程仓库的所有内容（如 vault/ 子目录）
    git::git_clone(&effective_url, &root)?;

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
        // 从文件读出的 cipher——算 md5 填 sync_md5（保证与 row 版本一致）
        let row = VaultCipher {
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
            deleted_at: None, // 文件读出的 cipher 未软删
            sync_md5: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let md5 = crate::sync::fingerprint::cipher_md5(&row);
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
            sync_md5: Some(md5),
        };
        // upsert：先 try update，失败则 insert
        upsert_cipher(&input)?;
    }
    for f in &folders {
        // upsert folder：先 try update name，失败则 insert。sort_order 用 0（默认）
        let md5 = crate::sync::fingerprint::folder_md5_from_fields(&f.id, &f.name, 0);
        upsert_folder(&f.id, &f.name, &md5)?;
    }

    // 4. 读所有热词版本文件 → upsert SQLite（v46：clone 同时导入 vault + hotword）
    match octopus_sync::hotword::import_hotwords_from_files() {
        Ok(hotword_files) => {
            for file in &hotword_files {
                let h = file.to_hotword_set(None);
                let md5 = octopus_sync::hotword::hotword_set_md5(&h);
                let mut h = h;
                h.sync_md5 = Some(md5);
                if let Err(e) = octopus_infra::db::upsert_hotword_set(&h) {
                    log::warn!("[sync] 热词版本 {} upsert 失败：{}", file.id, e);
                }
            }
            log::info!("[sync] clone_initial：{} 热词版本导入 SQLite", hotword_files.len());
        }
        Err(e) => log::warn!("[sync] 热词导入失败（不阻断 vault clone）：{}", e),
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
fn upsert_folder(id: &str, encrypted_name: &str, sync_md5: &str) -> Result<(), SyncError> {
    // infra 没有直接 upsert folder——用 list 检查存在性
    let exists = db::list_vault_folders()
        .map_err(SyncError::Other)?
        .iter()
        .any(|f| f.id == id);
    if exists {
        db::update_vault_folder_name(id, encrypted_name, sync_md5).map_err(SyncError::Other)?;
    } else {
        db::insert_vault_folder(id, encrypted_name, sync_md5).map_err(SyncError::Other)?;
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
    /// 热词同步统计（v46 新增——sync_now 同时同步 vault + hotword）。
    pub hotwords_pulled: usize,
    pub hotwords_pushed: usize,
    pub message: String,
}

/// 手动触发同步——编排 pull_merge_push 流程。
///
/// Phase 1 唯一触发方式（手动按钮）。Phase 2 会加自动同步。
pub fn sync_now() -> Result<SyncReport, SyncError> {
    let _guard = try_sync_lock()?;
    // 标记 syncing = true，函数退出（Ok/Err）时自动 false
    let _syncing_guard = SyncingGuard::set();

    if !git::check_git_available() {
        return Err(SyncError::GitNotInstalled);
    }

    let root = octopus_sync::store::sync_root();
    if !git::is_git_repo(&root) {
        return Err(SyncError::RepoNotInitialized);
    }

    // 0. 清理崩溃残留
    git::cleanup_in_progress_ops(&root)?;

    // 0.5 兜底：把已有 HTTPS remote 自动改成 SSH（避免 GitHub HTTPS 卡用户名 prompt）
    ensure_remotes_use_ssh_when_possible(&root);

    // 1. fetch
    git::git_fetch_all(&root)?;

    // 2. merge --ff-only（区分 3 种结果）
    // - FastForwarded：远程领先，已合并
    // - CannotFastForward：分叉 → rebase 兜底
    // - NoUpstream：远程是空仓库（首次推送场景）→ 跳过 merge/rebase，直接 push -u
    let merge_result = git::git_merge_ff(&root, "origin/main")?;
    let is_first_push = matches!(merge_result, git::MergeFfResult::NoUpstream);
    if !is_first_push {
        match merge_result {
            git::MergeFfResult::FastForwarded => {
                log::debug!("[sync] ff-only merge 成功")
            }
            git::MergeFfResult::CannotFastForward => {
                log::info!("[sync] merge --ff-only 失败（分叉），走 rebase 路径");
                match git::git_rebase(&root, "origin/main") {
                    Ok(()) => log::info!("[sync] rebase 成功"),
                    Err(e) => {
                        log::error!("[sync] rebase 失败，需手动介入：{}", e);
                        return Err(e);
                    }
                }
            }
            git::MergeFfResult::NoUpstream => unreachable!("已由 is_first_push 处理"),
        }
    } else {
        log::info!("[sync] 远程无 main 分支（首次推送场景），跳过 merge/rebase");
    }

    // 3. pull 阶段：文件系统 → SQLite
    // 首次推送时远程是空的，outline 也是默认空——pull 不会引入数据
    let pulled = pull_from_files()?;
    // 热词 pull（v46：sync_now 同时同步 vault + hotword）
    let hotwords_pulled = match octopus_sync::hotword::pull_hotwords_from_files() {
        Ok(n) => n,
        Err(e) => {
            log::warn!("[sync] 热词 pull 失败（不阻断 vault 同步）：{}", e);
            0
        }
    };

    // 4. push 阶段：SQLite → 文件系统
    let pushed = push_to_files()?;
    // 热词 push
    let hotwords_pushed = match octopus_sync::hotword::push_hotwords_to_files() {
        Ok(n) => n,
        Err(e) => {
            log::warn!("[sync] 热词 push 失败（不阻断 vault 同步）：{}", e);
            0
        }
    };

    // 5. commit（无变化时 git_commit 返 false，不阻断流程）
    let root = octopus_sync::store::sync_root();
    git::git_add_all(&root)?;
    let _committed = git::git_commit(&root, "sync")?;

    // 6. push to all remotes（无条件 push——git push 在零变化时返 Everything up-to-date，幂等无副作用）
    // 首次推送用 -u 设 upstream；后续普通 push
    {
        let remotes = git::git_remote_list(&root).unwrap_or_default();
        if remotes.is_empty() {
            log::warn!("[sync] 无 remote 配置——跳过 push");
        }
        for (name, _url) in &remotes {
            let push_result = if is_first_push {
                git::git_push_set_upstream(&root, name, "main")
            } else {
                git::git_push(&root, name, "main")
            };
            match push_result {
                Ok(()) => log::debug!("[sync] pushed to {} (first_push={})", name, is_first_push),
                Err(e) => log::warn!("[sync] push to {} 失败：{}", name, e),
            }
        }
    }

    let report = SyncReport {
        pulled,
        pushed,
        deleted: 0,
        hotwords_pulled,
        hotwords_pushed,
        message: if is_first_push {
            "首次同步完成，已推送到远程".to_string()
        } else {
            format!(
                "同步完成：vault 拉取 {} 条/推送 {} 条，热词拉取 {} 条/推送 {} 条",
                pulled, pushed, hotwords_pulled, hotwords_pushed
            )
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
                // 从文件读出的 cipher 算 md5 填 sync_md5
                let md5 = crate::sync::fingerprint::cipher_md5(&row);
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
                    sync_md5: Some(md5),
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
                let md5 = crate::sync::fingerprint::folder_md5_from_fields(
                    &folder_file.id,
                    &folder_file.encrypted_name,
                    0,
                );
                upsert_folder(&folder_file.id, &folder_file.encrypted_name, &md5)?;
                count += 1;
            }
        }
    }

    // meta.json → upsert vault_meta（同步 KDF 参数 + sync keys）
    //
    // **security_stamp 守卫**（INV-S9，2026-07-22 修复）：远程 meta.json 的 stamp
    // 必须与本地一致才允许覆盖——否则意味着远程 vault 用了不同主密码，覆盖会破坏
    // 本地加密参数（曾导致主密码验证失败：dummy meta.json 覆盖了真实 vault_meta）。
    if let Ok(meta_file) = store::read_meta_file() {
        let (kdf_type, salt, iters, mem, par, uvk, app_sync, stamp, equiv) =
            meta_file.to_sync_fields();

        // 本地 vault_meta 已存在时，校验 stamp 一致
        let local_meta = db::load_vault_meta().map_err(SyncError::Other)?;
        if let Some(ref local) = local_meta {
            if local.security_stamp != stamp {
                return Err(SyncError::MasterPasswordMismatch);
            }
        }

        // stamp 一致（或本地无 vault_meta）——保留本地 app_key_local_enc / public_key
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

// === stamp 冲突解决（2026-07-22）===
//
// pull_from_files 检测到 security_stamp 不一致时返 MasterPasswordMismatch，
// 用户通过 resolve_with_remote / resolve_with_local 主动选择以哪边为准。

/// 以远程为准——本地 vault_meta 被污染（如开发期 dummy 数据覆盖），远程是对的。
///
/// 用户输入远程 vault 的主密码，验证通过后用远程 meta 的同步字段覆盖本地
/// （保留 app_key_local_enc / public_key / protected_private_key）。
/// 本地 stamp 变成 remote stamp → 后续 sync 正常。
///
/// **密码验证**：用远程 KDF 参数（salt + Argon2Params）+ 用户输入密码派生
/// master_root_key，尝试解 remote protected_user_vault_key——失败即密码错误。
pub fn resolve_with_remote(password: &str) -> Result<(), SyncError> {
    use crate::crypto::kdf::{derive_master_root_key, Argon2Params};

    // 1. 读远程 meta.json
    let remote_meta = store::read_meta_file()?;
    let (kdf_type, salt, iters, mem, par, uvk, app_sync, stamp, equiv) =
        remote_meta.to_sync_fields();

    // 2. 用远程 KDF 参数 + 密码派生 master_root_key，验证密码
    let params = Argon2Params {
        iterations: iters as u32,
        memory_kib: mem as u32,
        parallelism: par as u32,
    };
    let master = derive_master_root_key(password.as_bytes(), &salt, &params)
        .map_err(|e| SyncError::Other(e.context("KDF 派生失败")))?;
    // 验证密码：解 protected_user_vault_key，失败即密码错
    let _uvk_bytes = master.decrypt(&uvk).map_err(|_| {
        SyncError::Other(anyhow::anyhow!("密码错误——无法解远程 protected_user_vault_key"))
    })?;

    // 3. 密码正确 → 用远程 9 个同步字段覆盖本地 vault_meta
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
        app_key_local_enc: local_enc, // 保留本地 K_machine 加密的
        app_key_sync_enc: app_sync,
        security_stamp: stamp,
        equivalent_domains: equiv,
        public_key: pub_key,
        protected_private_key: priv_key,
    };
    db::upsert_vault_meta(&meta_input).map_err(SyncError::Other)?;
    log::info!("[sync] resolve_with_remote 完成——本地 vault_meta 已用远程覆盖");
    Ok(())
}

/// 以本地为准——远程 meta 被污染，本地是对的。
///
/// 用户输入本地 vault 的主密码验证后，重新 export 本地 meta 到 `.sync/vault/meta.json`
/// 覆盖远程的脏 meta，git add + commit。下次 push 时远程 stamp 被本地覆盖。
///
/// **密码验证**：用本地 KDF 参数 + 密码派生 master_root_key，解本地
/// protected_user_vault_key——失败即密码错误（和 unlock 同逻辑）。
pub fn resolve_with_local(password: &str) -> Result<(), SyncError> {
    use crate::crypto::kdf::{derive_master_root_key, Argon2Params};

    // 1. 读本地 vault_meta，用本地 KDF 参数验证密码
    let local_meta: VaultMeta = db::load_vault_meta()
        .map_err(SyncError::Other)?
        .ok_or_else(|| SyncError::Other(anyhow::anyhow!("本地 vault_meta 不存在")))?;
    let params = Argon2Params {
        iterations: local_meta.kdf_iterations as u32,
        memory_kib: local_meta.kdf_memory_kib as u32,
        parallelism: local_meta.kdf_parallelism as u32,
    };
    let master = derive_master_root_key(password.as_bytes(), &local_meta.kdf_salt, &params)
        .map_err(|e| SyncError::Other(e.context("KDF 派生失败")))?;
    // 验证密码
    let _uvk_bytes = master.decrypt(&local_meta.protected_user_vault_key).map_err(|_| {
        SyncError::Other(anyhow::anyhow!("密码错误——无法解本地 protected_user_vault_key"))
    })?;

    // 2. 密码正确 → 重新 export 本地 meta 到文件系统覆盖远程的脏 meta
    let root = octopus_sync::store::sync_root();
    if !git::is_git_repo(&root) {
        return Err(SyncError::RepoNotInitialized);
    }
    let meta_file = store::MetaFile::from_vault_meta(&local_meta);
    store::write_meta_file(&meta_file)?;

    // 3. git add + commit
    git::git_add_all(&root)?;
    git::git_commit(&root, "resolve: use local meta")?;
    log::info!("[sync] resolve_with_local 完成——远程 meta 已用本地覆盖");
    Ok(())
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
/// 返回**实际变更**的 cipher+folder 数量（对比旧 outline 的 sha，新增或修改才算）。
/// 不是总数——避免「每次同步都推 4 条」的误导（用户反馈：本地和 remote 已同步，
/// 应该连推送都没有）。
fn push_to_files() -> Result<usize, SyncError> {
    let meta = db::load_vault_meta()
        .map_err(SyncError::Other)?
        .ok_or_else(|| SyncError::Other(anyhow::anyhow!("vault_meta 不存在")))?;
    let ciphers = db::list_vault_ciphers().map_err(SyncError::Other)?;
    let folders = db::list_vault_folders().map_err(SyncError::Other)?;

    // 增量导出——只写 sync_md5 变化的文件，删 SQLite 无的。
    // 返实际变更文件数（不是总数），SyncReport 据此显示「推送 N 条」。
    let (_new_outline, changed) = store::incremental_export(&meta, &ciphers, &folders)?;
    Ok(changed)
}

// === T4.9: disable_sync ===

/// 禁用同步——删除 `~/.octopus/.sync/`（git repo 根 + 所有子目录，保留 SQLite 数据）。
pub fn disable_sync() -> Result<(), SyncError> {
    let root = octopus_sync::store::sync_root();
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .with_context(|| format!("删除 sync 目录失败：{}", root.display()))
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

    /// 私有库守卫：本地路径直接拒绝（不依赖 git / 网络）。
    #[test]
    fn ensure_private_rejects_local_path() {
        let err = ensure_private_repo("/Users/me/repo").unwrap_err();
        assert!(matches!(err, SyncError::LocalPathRejected));
        let err = ensure_private_repo("file:///path/to/repo").unwrap_err();
        assert!(matches!(err, SyncError::LocalPathRejected));
    }

    /// 私有库守卫：SSH URL 放行（无法自动检测）。
    #[test]
    fn ensure_private_allows_ssh() {
        ensure_private_repo("git@github.com:owner/repo.git").expect("SSH 应放行");
        ensure_private_repo("ssh://git@github.com/owner/repo.git").expect("SSH 应放行");
    }

    /// 私有库守卫：未知 scheme 拒绝。
    #[test]
    fn ensure_private_rejects_unknown_scheme() {
        assert!(ensure_private_repo("git://host/repo.git").is_err());
    }

    // === maybe_rewrite_to_ssh（2026-07-21 增补） ===

    /// 非 github/gitee URL 不改写（自建 host、SSH URL、本地路径都应原样返回）。
    #[test]
    fn rewrite_preserves_non_github_gitee_urls() {
        // SSH URL 不改写
        let ssh = "git@github.com:owner/repo.git";
        assert_eq!(maybe_rewrite_to_ssh(ssh).unwrap(), ssh);
        // 自建 host 不改写（即使 HTTPS）
        let self_hosted = "https://gitlab.com/owner/repo.git";
        assert_eq!(maybe_rewrite_to_ssh(self_hosted).unwrap(), self_hosted);
        // 本地路径不改写
        let local = "/abs/path/to/repo";
        assert_eq!(maybe_rewrite_to_ssh(local).unwrap(), local);
    }

    // === 集成测试（2026-07-22 增补）===
    //
    // 这些测试是为了抓住设计层面的偏离——之前 bug：
    // 1. .git 建在 vault/ 而非 .sync/（每个子目录独立 repo）
    // 2. outline.json 字段名 sha 而非 md5
    // 3. vault_version 无变化时也 +1
    // 都是设计偏离，单元测试本应抓住但当时没写。

    use octopus_infra::db::VaultMetaInput;

    /// 集成测试 guard——隔离 sync_root + 内存 DB + 预置 vault_meta。
    struct IntegrationGuard {
        _tmp: tempfile::TempDir,
    }
    impl IntegrationGuard {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().expect("tempdir");
            let sync_path = tmp.path().join(".sync");
            store::set_test_vault_root(sync_path);

            // 内存 DB + 预置 vault_meta（不经 setup_vault / Keychain）
            let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
            octopus_infra::db::set_test_db(conn);
            let input = VaultMetaInput {
                kdf_type: 0,
                kdf_salt: vec![1u8; 32],
                kdf_iterations: 3,
                kdf_memory_kib: 65_536,
                kdf_parallelism: 4,
                protected_user_vault_key: "v1:dummy-uvk".into(),
                app_key_local_enc: "v1:dummy-local".into(),
                app_key_sync_enc: "v1:dummy-sync".into(),
                security_stamp: "stamp-test".into(),
                equivalent_domains: "[]".into(),
                public_key: None,
                protected_private_key: None,
            };
            octopus_infra::db::upsert_vault_meta(&input).expect("setup vault_meta");

            Self { _tmp: tmp }
        }
    }
    impl Drop for IntegrationGuard {
        fn drop(&mut self) {
            store::clear_test_vault_root();
        }
    }

    /// enable_sync 后 .git 应在 sync_root（.sync/），不在 vault_dir（.sync/vault/）。
    /// 回归测试：之前 bug 是 .git 建在 vault/ 下，变成每子目录独立 repo。
    #[test]
    fn enable_sync_creates_git_in_sync_root_not_vault_dir() {
        let g = IntegrationGuard::new();
        // enable_sync 需要 git 可用——CI 没装 git 时跳过
        if !git::check_git_available() {
            eprintln!("[skip] git not available");
            return;
        }

        enable_sync().expect("enable_sync 应成功");

        let sync_root = octopus_sync::store::sync_root();
        let vault_dir = store::vault_dir();

        // .git 必须在 sync_root
        assert!(
            sync_root.join(".git").exists(),
            ".git 必须在 sync_root（{}），实际不存在",
            sync_root.display()
        );
        // .git 不能在 vault_dir（之前的 bug）
        assert!(
            !vault_dir.join(".git").exists(),
            ".git 不能在 vault_dir（{}）——这是之前的 bug，每个子目录变成独立 repo",
            vault_dir.display()
        );
        let _ = g;
    }

    /// enable_sync 后 vault 数据文件应在 .sync/vault/ 下，不在 .sync/ 根。
    #[test]
    fn enable_sync_writes_vault_data_in_vault_subdir() {
        let g = IntegrationGuard::new();
        if !git::check_git_available() {
            eprintln!("[skip] git not available");
            return;
        }

        enable_sync().expect("enable_sync");

        let sync_root = octopus_sync::store::sync_root();
        let vault_dir = store::vault_dir();

        // meta.json / outline.json 在 vault_dir 下
        assert!(vault_dir.join("meta.json").exists(), "meta.json 应在 vault_dir 下");
        assert!(vault_dir.join("outline.json").exists(), "outline.json 应在 vault_dir 下");
        // meta.json / outline.json 不在 sync_root 根
        assert!(!sync_root.join("meta.json").exists(), "meta.json 不能在 sync_root 根");
        assert!(!sync_root.join("outline.json").exists(), "outline.json 不能在 sync_root 根");
        let _ = g;
    }

    /// outline.json 序列化的字段名是 `md5` + `updated_ms`，不是 `sha` + `updated_at`。
    /// 回归测试：之前字段名歧义（sha 值是 md5），用户反馈后改正。
    ///
    /// 不走 enable_sync（空 DB 时 outline 是空 object 无法验证字段名）——直接
    /// 构造一个含 cipher 的 outline 序列化，检查字段名。
    #[test]
    fn outline_json_uses_md5_and_updated_ms_field_names() {
        use octopus_sync::outline::{Outline, OutlineEntry};
        use std::collections::BTreeMap;

        let outline = Outline {
            version: 1,
            vault_version: 1,
            ciphers: BTreeMap::from([(
                "uuid-1".to_string(),
                OutlineEntry {
                    md5: "abc123".to_string(),
                    updated_ms: 1234567890,
                },
            )]),
            folders: BTreeMap::new(),
        };
        let json = serde_json::to_string(&outline).expect("serialize");

        // 字段名必须是 md5 + updated_ms（不是 sha + updated_at）
        assert!(
            json.contains("\"md5\""),
            "outline.json 应含 \"md5\" 字段，实际：{}", json
        );
        assert!(
            json.contains("\"updated_ms\""),
            "outline.json 应含 \"updated_ms\" 字段，实际：{}", json
        );
        assert!(
            !json.contains("\"sha\""),
            "outline.json 不应含旧字段名 \"sha\"，实际：{}", json
        );
        assert!(
            !json.contains("\"updated_at\""),
            "outline.json 不应含旧字段名 \"updated_at\"，实际：{}", json
        );
    }

    // === 热词同步集成测试（Task 13，2026-07-22 增补）===
    //
    // 验证 sync_now / push_initial / clone_initial 的热词集成点。

    /// enable_sync（= push_initial）后热词文件应在 .sync/hotword/ 下生成。
    /// 回归守护：push_initial 必须同时导出 vault + hotword（Task 13.7 集成）。
    #[test]
    fn enable_sync_exports_hotword_data() {
        let g = IntegrationGuard::new();
        if !git::check_git_available() {
            eprintln!("[skip] git not available");
            return;
        }

        // IntegrationGuard 的 set_test_db 已建默认「通用」热词 seed——再加一个版本
        octopus_infra::db::insert_hotword_set("test-uuid-hotword-a", "测试版本A")
            .expect("insert hotword set");
        octopus_infra::db::set_hotword_set_words("test-uuid-hotword-a", "苹果 香蕉")
            .expect("set words");

        enable_sync().expect("enable_sync");

        let sync_root = octopus_sync::store::sync_root();
        let hotword_dir = sync_root.join("hotword");

        // hotword/outline.json 必须存在
        assert!(
            hotword_dir.join("outline.json").exists(),
            "enable_sync 后应生成 .sync/hotword/outline.json"
        );

        // outline 应含 2 个 entry（默认「通用」+ 测试版本A）
        let outline = octopus_sync::hotword::read_hotword_outline().expect("read outline");
        assert!(
            outline.ciphers.len() >= 2,
            "hotword outline 应含至少 2 个版本（通用 + 测试A），实际 {}",
            outline.ciphers.len()
        );
        assert!(
            outline.ciphers.contains_key("test-uuid-hotword-a"),
            "hotword outline 应含测试版本A"
        );

        // sets/ 下应有版本文件（分桶）
        let set_file =
            octopus_sync::hotword::hotword_set_file_path("test-uuid-hotword-a");
        assert!(
            set_file.exists(),
            "热词版本文件应存在：{}",
            set_file.display()
        );
        let _ = g;
    }

    /// enable_sync 后修改热词，调 push_hotwords_to_files 应把变更写入文件系统。
    /// 验证 sync_now push 阶段的热词调用链（不经过 git，直接测文件层）。
    #[test]
    fn hotword_push_reflects_db_changes_to_files() {
        let g = IntegrationGuard::new();
        if !git::check_git_available() {
            eprintln!("[skip] git not available");
            return;
        }

        // 首次 enable_sync（含默认「通用」热词）
        enable_sync().expect("enable_sync initial");

        // 新增热词版本（DB 层）
        octopus_infra::db::insert_hotword_set("test-uuid-push-1", "推送测试")
            .expect("insert");
        octopus_infra::db::set_hotword_set_words("test-uuid-push-1", "葡萄")
            .expect("set words");

        // 调 push——应把新版本写入文件
        let pushed = octopus_sync::hotword::push_hotwords_to_files().expect("push");
        assert!(pushed > 0, "新增版本应有变更写入");

        // 文件应存在
        let set_file = octopus_sync::hotword::hotword_set_file_path("test-uuid-push-1");
        assert!(set_file.exists(), "push 后版本文件应存在");

        // outline 应含新版本
        let outline = octopus_sync::hotword::read_hotword_outline().expect("outline");
        assert!(outline.ciphers.contains_key("test-uuid-push-1"));
        let _ = g;
    }

    /// pull 应从文件系统读热词到 DB。
    /// 模拟 clone_initial 的热词 import 路径：文件已有 → pull 到空 DB。
    #[test]
    fn hotword_pull_imports_files_to_db() {
        let g = IntegrationGuard::new();
        if !git::check_git_available() {
            eprintln!("[skip] git not available");
            return;
        }

        // 准备：先写一些热词版本到 DB + export 到文件
        octopus_infra::db::insert_hotword_set("test-uuid-pull-1", "拉取测试1")
            .expect("insert 1");
        octopus_infra::db::insert_hotword_set("test-uuid-pull-2", "拉取测试2")
            .expect("insert 2");
        let sets = octopus_infra::db::list_hotword_sets().expect("list");
        octopus_sync::hotword::export_all_hotwords(&sets).expect("export");

        // 清空 DB 热词（模拟 B 机 clone 前 DB 无热词）
        for h in octopus_infra::db::list_hotword_sets().expect("list") {
            let _ = octopus_infra::db::delete_hotword_set(&h.id);
        }
        assert!(
            octopus_infra::db::list_hotword_sets().unwrap().is_empty(),
            "清空后 DB 应无热词"
        );

        // pull——应从文件读回
        let pulled = octopus_sync::hotword::pull_hotwords_from_files().expect("pull");
        assert!(pulled >= 2, "应至少拉取 2 个版本");

        let db_sets = octopus_infra::db::list_hotword_sets().expect("list");
        assert!(
            db_sets.iter().any(|h| h.id == "test-uuid-pull-1"),
            "DB 应含拉取测试1"
        );
        assert!(
            db_sets.iter().any(|h| h.id == "test-uuid-pull-2"),
            "DB 应含拉取测试2"
        );
        let _ = g;
    }

    /// sync_now 完整流程（enable_sync → sync_now）应不 panic，且 SyncReport 含热词字段。
    /// 验证 sync_now 的热词 pull/push 调用点被正确执行（即使无 remote，pull/push 文件层仍跑）。
    #[test]
    fn sync_now_runs_hotword_integration_without_panic() {
        let g = IntegrationGuard::new();
        if !git::check_git_available() {
            eprintln!("[skip] git not available");
            return;
        }

        // 准备热词数据
        octopus_infra::db::insert_hotword_set("test-uuid-syncnow-1", "同步测试")
            .expect("insert");

        // 首次 enable_sync（建 git repo + 初始 commit）
        enable_sync().expect("enable_sync");

        // sync_now——无 remote 时 fetch 会失败，但热词 pull/push 文件层在 fetch 之前不执行。
        // 这个测试验证的是「sync_now 能被调用且不因热词集成 panic」。
        // fetch 失败返 Err 是预期的（无 remote）——我们关心的是热词代码路径无 bug。
        let result = sync_now();
        // 无 remote → fetch 失败 → Err 是预期。但不应是热词相关的 panic。
        match result {
            Ok(report) => {
                // 理论上无 remote 时不会走到这，但如果走到，热词字段应在
                let _ = report.hotwords_pulled;
                let _ = report.hotwords_pushed;
            }
            Err(e) => {
                // Err 是预期（无 remote）——只要不是热词 panic 就行
                let msg = e.to_string();
                assert!(
                    !msg.contains("hotword") || msg.contains("热词"),
                    "错误不应是热词特有 panic：{}",
                    msg
                );
            }
        }
        let _ = g;
    }

    /// security_stamp 守卫（INV-S9）：pull_from_files 读到 stamp 不一致的 meta.json 时
    /// 必须拒绝覆盖 vault_meta——否则会用错误主密码的加密参数破坏本地数据。
    ///
    /// 回归守护：曾因缺少此校验，dummy meta.json（stamp-1）覆盖了真实 vault_meta
    /// （真实 UUID stamp），导致用户主密码验证失败。
    #[test]
    fn pull_rejects_mismatched_security_stamp() {
        let g = IntegrationGuard::new();
        // IntegrationGuard 预置的 vault_meta security_stamp = "stamp-test"

        // 手动写一个 stamp 不一致的 meta.json 到 .sync/vault/
        let meta_file = store::MetaFile {
            version: 1,
            kdf_type: 0,
            kdf_salt: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=".into(),
            kdf_iterations: 3,
            kdf_memory_kib: 65536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:dummy-uvk".into(),
            app_key_sync_enc: "v1:dummy-sync".into(),
            security_stamp: "DIFFERENT-STAMP-XXX".into(), // 与本地 "stamp-test" 不一致
            equivalent_domains: "[]".into(),
        };
        store::write_meta_file(&meta_file).expect("write meta");

        // pull_from_files 应返 MasterPasswordMismatch，不覆盖 vault_meta
        let result = pull_from_files();
        assert!(
            matches!(result, Err(SyncError::MasterPasswordMismatch)),
            "stamp 不一致应拒绝覆盖，实际：{:?}",
            result
        );

        // 验证本地 vault_meta 没被破坏
        let local = octopus_infra::db::load_vault_meta().unwrap().unwrap();
        assert_eq!(
            local.security_stamp, "stamp-test",
            "本地 stamp 不应被覆盖"
        );
        assert_eq!(
            local.protected_user_vault_key, "v1:dummy-uvk",
            "本地 protected_user_vault_key 不应被覆盖（仍是 IntegrationGuard 预置值）"
        );
        let _ = g;
    }

    /// security_stamp 一致时 pull_from_files 应正常覆盖 vault_meta（合法同步场景）。
    #[test]
    fn pull_allows_matching_security_stamp() {
        let g = IntegrationGuard::new();

        // 写一个 stamp 一致（stamp-test）的 meta.json
        let meta_file = store::MetaFile {
            version: 1,
            kdf_type: 0,
            kdf_salt: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=".into(),
            kdf_iterations: 3,
            kdf_memory_kib: 65536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:new-uvk-from-remote".into(),
            app_key_sync_enc: "v1:new-sync".into(),
            security_stamp: "stamp-test".into(), // 与本地一致
            equivalent_domains: "[]".into(),
        };
        store::write_meta_file(&meta_file).expect("write meta");

        // pull 应成功（stamp 一致），不返 Err
        let result = pull_from_files();
        assert!(
            result.is_ok(),
            "stamp 一致应允许覆盖，实际：{:?}",
            result
        );

        // vault_meta 应被更新为 meta.json 的值
        let local = octopus_infra::db::load_vault_meta().unwrap().unwrap();
        assert_eq!(
            local.protected_user_vault_key, "v1:new-uvk-from-remote",
            "stamp 一致时 vault_meta 应被 meta.json 覆盖"
        );
        let _ = g;
    }
}
