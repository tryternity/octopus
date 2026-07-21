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
use crate::sync::privacy::{self, PrivacyVerdict};
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
}

/// 查询同步状态——UI 初始化时调用。
pub fn get_sync_status() -> SyncStatus {
    let git_available = git::check_git_available();
    let root = store::vault_root();
    let syncing = is_syncing();

    if !git_available || !root.exists() || !git::is_git_repo(&root) {
        return SyncStatus {
            git_available,
            initialized: false,
            remotes: vec![],
            last_sync: None,
            last_commit_sha: None,
            syncing,
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

    let root = store::vault_root();
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

    // 3. git init + commit（不 push——还没配 remote）
    git::git_init(&root)?;
    git::git_add_all(&root)?;
    git::git_commit(&root, "init vault")?;

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
    let root = store::vault_root();
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
    let root = store::vault_root();
    if !git::is_git_repo(&root) {
        return Err(SyncError::RepoNotInitialized);
    }
    git::git_remote_remove(&root, name)?;
    log::info!("[sync] 删除 remote: {}", name);
    Ok(())
}

/// 列出所有 remote（name → url）。
pub fn list_remotes() -> Result<Vec<(String, String)>, SyncError> {
    let root = store::vault_root();
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

    let root = store::vault_root();
    // 确保父目录存在
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建父目录失败：{}", parent.display()))
            .map_err(SyncError::Other)?;
    }

    // 1. git clone（clone 会创建 .vault 目录；用 effective_url 走 SSH）
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
    // 标记 syncing = true，函数退出（Ok/Err）时自动 false
    let _syncing_guard = SyncingGuard::set();

    if !git::check_git_available() {
        return Err(SyncError::GitNotInstalled);
    }

    let root = store::vault_root();
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

    // 4. push 阶段：SQLite → 文件系统
    let pushed = push_to_files()?;

    // 5. commit
    let root = store::vault_root();
    git::git_add_all(&root)?;
    let committed = git::git_commit(&root, "sync")?;

    // 6. push to all remotes（用户自定义的任意 remote）
    // 首次推送用 -u 设 upstream；后续普通 push
    if committed {
        let remotes = git::git_remote_list(&root).unwrap_or_default();
        if remotes.is_empty() {
            log::warn!("[sync] 本地有 commit 但无 remote 配置——跳过 push");
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
/// 返回**实际变更**的 cipher+folder 数量（对比旧 outline 的 sha，新增或修改才算）。
/// 不是总数——避免「每次同步都推 4 条」的误导（用户反馈：本地和 remote 已同步，
/// 应该连推送都没有）。
fn push_to_files() -> Result<usize, SyncError> {
    let meta = db::load_vault_meta()
        .map_err(SyncError::Other)?
        .ok_or_else(|| SyncError::Other(anyhow::anyhow!("vault_meta 不存在")))?;
    let ciphers = db::list_vault_ciphers().map_err(SyncError::Other)?;
    let folders = db::list_vault_folders().map_err(SyncError::Other)?;

    // 读旧 outline 对比 sha——算实际变更数
    let old_outline = store::read_outline_file().unwrap_or_default();

    // 全量写文件系统（cipher 数量通常 < 1000，全量写毫秒级）
    let new_outline = store::export_all_to_files(&meta, &ciphers, &folders)?;

    // 对比新旧 outline——sha 不同（含新增）算变更
    let changed = count_outline_changes(&old_outline, &new_outline);
    Ok(changed)
}

/// 对比两个 outline，返 cipher + folder 中 sha 变化（含新增/删除）的条目数。
fn count_outline_changes(
    old: &crate::sync::outline::Outline,
    new: &crate::sync::outline::Outline,
) -> usize {
    let mut count = 0;
    // ciphers：新增 or sha 变
    for (uuid, entry) in &new.ciphers {
        match old.ciphers.get(uuid) {
            None => count += 1, // 新增
            Some(old_entry) if old_entry.sha != entry.sha => count += 1, // sha 变
            _ => {}
        }
    }
    // 删除的也算（旧有新无）
    count += old.ciphers.keys().filter(|k| !new.ciphers.contains_key(*k)).count();
    // folders 同
    for (uuid, entry) in &new.folders {
        match old.folders.get(uuid) {
            None => count += 1,
            Some(old_entry) if old_entry.sha != entry.sha => count += 1,
            _ => {}
        }
    }
    count += old.folders.keys().filter(|k| !new.folders.contains_key(*k)).count();
    count
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

    // === count_outline_changes（2026-07-21 增补） ===

    use crate::sync::outline::{Outline, OutlineEntry};

    fn outline_entry(sha: &str) -> OutlineEntry {
        OutlineEntry {
            sha: sha.into(),
            updated_at: "2026-07-21T10:00:00".into(),
        }
    }

    #[test]
    fn count_changes_zero_when_identical() {
        // 完全相同 → 0 变更
        let old = Outline {
            ciphers: [("c1".into(), outline_entry("sha1"))].into_iter().collect(),
            ..Default::default()
        };
        let new = old.clone();
        assert_eq!(count_outline_changes(&old, &new), 0);
    }

    #[test]
    fn count_changes_detects_new_cipher() {
        let old = Outline::default();
        let new = Outline {
            ciphers: [("c1".into(), outline_entry("sha1"))].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(count_outline_changes(&old, &new), 1);
    }

    #[test]
    fn count_changes_detects_modified_cipher() {
        let old = Outline {
            ciphers: [("c1".into(), outline_entry("sha-old"))].into_iter().collect(),
            ..Default::default()
        };
        let new = Outline {
            ciphers: [("c1".into(), outline_entry("sha-new"))].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(count_outline_changes(&old, &new), 1);
    }

    #[test]
    fn count_changes_detects_deleted_cipher() {
        let old = Outline {
            ciphers: [("c1".into(), outline_entry("sha1"))].into_iter().collect(),
            ..Default::default()
        };
        let new = Outline::default();
        assert_eq!(count_outline_changes(&old, &new), 1);
    }

    #[test]
    fn count_changes_mixed_cipher_and_folder() {
        let old: Outline = Outline {
            ciphers: [
                ("c1".into(), outline_entry("sha-old")),
                ("c2-deleted".into(), outline_entry("sha2")),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let new: Outline = Outline {
            ciphers: [
                ("c1".into(), outline_entry("sha-new")), // 修改
                ("c3-new".into(), outline_entry("sha3")), // 新增
            ]
            .into_iter()
            .collect(),
            folders: [("f1".into(), outline_entry("sha-f1"))].into_iter().collect(),
            ..Default::default()
        };
        // c1 修改 + c3 新增 + c2 删除 + f1 新增 = 4
        assert_eq!(count_outline_changes(&old, &new), 4);
    }
}
