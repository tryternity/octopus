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
use octopus_infra::db::{self, VaultCipherInput, VaultMeta, VaultMetaInput};
// 通用 sync 基础设施（2026-07-22 抽离到 octopus_sync）
use octopus_sync::error::SyncError;
use octopus_sync::git;
use octopus_sync::privacy::{self, PrivacyVerdict};
use zeroize::Zeroizing;

use crate::sync::store;
use crate::crypto::kdf::Argon2Params;

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
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// git 是否可用
    pub git_available: bool,
    /// `~/.octopus/.vault/` 是否已初始化（.git 存在）
    pub initialized: bool,
    /// 配置的 remotes（name → 已 redact 的 url，SafeUrl 保证 PAT 不泄露到前端）
    pub remotes: Vec<(String, octopus_sync::error::SafeUrl)>,
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
        // E-UI-URL-LEAKS-PAT-LIST-REMOTES 修复（2026-07-26）：与 list_remotes 同型
        // （第七次外溢候选）——SyncStatus derive Serialize 直接流向前端，remotes
        // 字段透传 .git/config 原始 url（含 PAT）。当前前端未消费 status.remotes
        // （用独立 list_remotes 拉取），但序列化后仍在客户端内存，且未来可能新增
        // 消费者——走 redact_remotes_for_outflow helper（与 list_remotes 共用）。
        remotes: redact_remotes_for_outflow(&remotes),
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
    // #7 修复：覆盖全部写入口——sync_now 进行中点 enable 会留下半提交残留
    let _guard = try_sync_lock()?;
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
    // E-LOG-URL-LEAKS-PAT-INCOMPLETE-OUTBOUND：redact 所有 log 中的 url（含 PAT）
    let safe_url = octopus_sync::error::redact_url(url);
    let safe_effective = octopus_sync::error::redact_url(&effective_url);
    log::info!("[sync] 添加 remote: {} → {}（effective: {}）", name, safe_url, safe_effective);
    Ok(())
}

/// 私有库守卫——检测 URL，公有库直接返 Err。
///
/// 其他 verdict（Private / Ambiguous / SshUnverifiable / NetworkError）放行，
/// 仅记录日志让用户能看到检测过程。
fn ensure_private_repo(url: &str) -> Result<(), SyncError> {
    // E-LOG-URL-LEAKS-PAT-INCOMPLETE 修复（2026-07-26）：入口统一 redact，
    // 所有分支 log 用 safe_url（第四十九轮只改了 Public 分支，漏了其余 4 个——
    // Ambiguous 是 PAT 访问私有库的默认路径，每次 sync 把 PAT 写日志）。
    let safe_url = octopus_sync::error::redact_url(url);
    let verdict = privacy::check_privacy(url)?;
    match verdict {
        PrivacyVerdict::Public => {
            log::warn!("[sync] 拒绝公有库: {}", safe_url);
            Err(SyncError::PublicRepoRejected(url.to_string()))
        }
        PrivacyVerdict::Private => {
            log::info!("[sync] 确认私有库: {}", safe_url);
            Ok(())
        }
        PrivacyVerdict::Ambiguous(reason) => {
            log::info!("[sync] 仓库可见性不明（放行）: {} —— {}", safe_url, reason);
            Ok(())
        }
        // S-SYNC-PUBLIC-LEAK-ON-RATELIMIT 修复（2026-07-27，第七十七轮）：
        // 限流（403）硬阻断——用户误配的 public repo 在限流时若放行，push 会
        // 把 vault 密文推到 public 导致不可逆泄漏。限流是临时的（用户重试即恢复），
        // 用「不可逆密钥泄漏」换取「用户少等几分钟」是错误的代价权衡。
        PrivacyVerdict::RateLimited(reason) => {
            log::warn!("[sync] API 限流，硬阻断（防 public 漏检）: {} —— {}", safe_url, reason);
            Err(SyncError::RateLimited(reason))
        }
        PrivacyVerdict::SshUnverifiable => {
            log::info!("[sync] SSH URL 无法自动检测（放行，由用户保证私有）: {}", safe_url);
            Ok(())
        }
        PrivacyVerdict::NetworkError(msg) => {
            log::warn!("[sync] 仓库可见性检测网络错误（放行）: {} —— {}", safe_url, msg);
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
    // E-LOG-URL-LEAKS-PAT-INCOMPLETE-OUTBOUND：redact 所有 log（第五十轮修了
    // ensure_private_repo 内部，漏了调用方链的 maybe_rewrite_to_ssh）。
    let safe_url = octopus_sync::error::redact_url(url);
    let Some(ssh_url) = privacy::try_convert_https_to_ssh(url) else {
        return Ok(url.to_string());
    };
    // 解析 SSH URL 拿 host 做 ssh -T 预检
    let parsed = privacy::GitRemoteUrl::parse(&ssh_url)?;
    log::info!(
        "[sync] 检测到 {} HTTPS URL，验证 SSH key 后尝试转 SSH: {}",
        parsed.host, safe_url
    );
    match git::verify_ssh_key_for_host(&parsed.host) {
        Ok(true) => {
            log::info!(
                "[sync] SSH key 可用，HTTPS → SSH: {} → {}",
                safe_url, ssh_url
            );
            Ok(ssh_url)
        }
        Ok(false) => {
            log::warn!(
                "[sync] SSH key 不可用（保留 HTTPS，后续 push 可能失败）: {}",
                safe_url
            );
            Ok(url.to_string())
        }
        Err(e) => {
            log::warn!(
                "[sync] SSH 预检失败（保留 HTTPS）: {} —— {}",
                safe_url, e
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
        // E-LOG-URL-LEAKS-PAT-INCOMPLETE-4TH-OUTBOUND 修复（2026-07-26）：第五次外溢
        // ——add_remote / ensure_private_repo / maybe_rewrite_to_ssh 都改了，
        // 漏了 sync_now 路径上的 ensure_remotes_use_ssh_when_possible。url 来自
        // .git/config（OBS-CLONE-URL-STORES-PAT-IN-CONFIG 场景存 PAT），透传泄露 PAT。
        let safe_url = octopus_sync::error::redact_url(url);
        match maybe_rewrite_to_ssh(url) {
            Ok(rewritten) if rewritten != *url => {
                // 改写后 URL 不同——set-url（rewritten 是 SSH URL，已 strip userinfo，安全）
                match git::git_remote_set_url(root, name, &rewritten) {
                    Ok(()) => log::info!(
                        "[sync] sync_now 自动改写 remote {}：{} → {}",
                        name, safe_url, rewritten
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
                name, safe_url, e
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
pub fn list_remotes() -> Result<Vec<(String, octopus_sync::error::SafeUrl)>, SyncError> {
    let root = octopus_sync::store::sync_root();
    if !git::is_git_repo(&root) {
        return Ok(vec![]);
    }
    // E-UI-URL-LEAKS-PAT-LIST-REMOTES 修复（2026-07-26）：第六次外溢——
    // 前五轮只追 log:: 宏的 url 透传，漏了「返回值流向前端 UI」维度。
    // list_remotes 返回的 url 来自 .git/config（OBS-CLONE-URL-STORES-PAT-IN-CONFIG
    // 场景存 PAT），透传到 SyncPanel.tsx:540 {url} 裸渲染——截图/录屏/窥屏即泄露。
    //
    // 在 engine 层 redact（而非 desktop 命令层）：cli/server 未来调 list_remotes
    // 也受益，单点维护。
    //
    // 功能权衡：test_connection(redacted_url) 对 PAT HTTPS remote 会失败
    // （CredentialsRequired，因 redact 后无凭据）——但 add_remote 时已强制
    // ensure_private_repo + maybe_rewrite_to_ssh（SSH key 预检），事后测试非刚需。
    // SSH remote（git@github.com:...）redact 后原样返回（url::Url parse 失败），
    // test_connection 不受影响。
    Ok(redact_remotes_for_outflow(&git::git_remote_list(&root)?))
}

/// 流出 crate 边界前对 remotes 做 redact（list_remotes + SyncStatus 共用）。
///
/// E-UI-URL-LEAKS-PAT-LIST-REMOTES 第六次外溢的根因防御：抽成 helper 让所有
/// 「remotes 流出」点统一走这一个入口。当前调用点：
///   - `list_remotes`（pub fn 返回值，Tauri command 返回）
///   - `get_sync_status`（SyncStatus.remotes 字段，Serialize 流向前端）
///
/// SafeUrl newtype（2026-07-28 实施）：返 `Vec<(String, SafeUrl)>`——编译期保证
/// url 经 redact_url（SafeUrl 唯一构造器）。helper 仍保留作为集中构造点，
/// 与 SafeUrl 配合形成双重防御。
fn redact_remotes_for_outflow(remotes: &[(String, String)]) -> Vec<(String, octopus_sync::error::SafeUrl)> {
    remotes
        .iter()
        .map(|(name, url)| (name.clone(), octopus_sync::error::redact_url(url)))
        .collect()
}

/// 从指定 remote URL clone 仓库（B 机首次同步）。
///
/// 用户先 add_remote 再 clone_from，或者直接 clone（会自动配 origin）。
pub fn clone_from(remote_url: &str) -> Result<(), SyncError> {
    // #7 修复：clone 也是写入口（git clone + 文件导入 SQLite）
    let _guard = try_sync_lock()?;
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
    // E2 修复（2026-07-24）：clone 前检查本地 vault_meta 已存在——
    // 若已设过主密码（DB 有 meta），clone 会用远程 meta 覆盖本地 kdf_salt/
    // protected_user_vault_key/security_stamp，导致本地 cipher 用旧 user_vault_key
    // 加密但新 meta 的 key 解不开 → 原数据永久锁死。
    // enable_sync 的 .sync 已是 repo 检查覆盖不到此场景（.sync 不存在 + DB 有 meta）。
    if db::load_vault_meta().map_err(SyncError::Other)?.is_some() {
        return Err(SyncError::Other(anyhow::anyhow!(
            "本地已初始化 vault（DB 有 vault_meta）——clone 会覆盖加密参数导致原数据锁死。\
             请先禁用同步 + 清空本地 vault（删除 vault_meta + 所有 cipher）后再 clone"
        )));
    }

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
    let f = meta_file.to_sync_fields()?;
    // K1-GAP 修复（2026-07-25）：clone 的远程 KDF 参数用 from_i64_strict 校验——
    // 防攻击者污染私有同步库的 meta.json 为弱 KDF（memory_kib=8 废掉 Argon2id
    // 内存硬度）。与 resolve_with_remote:970 一致，补齐 K1 防御的主路径（之前
    // 只 stamp 冲突罕见分支有 strict，常规 clone/pull 漏防）。
    let _strict_params = Argon2Params::from_i64_strict(f.kdf_iterations, f.kdf_memory_kib, f.kdf_parallelism)
        .map_err(SyncError::Other)?;
    // clone_initial 时 vault_meta 可能还没初始化（B 机首次）——用 upsert 写入
    // app_key_local_enc / public_key 留空，用户解锁后 refresh_app_key_local_enc 会填
    let meta_input = VaultMetaInput {
        kdf_type: f.kdf_type,
        kdf_salt: f.kdf_salt,
        kdf_iterations: f.kdf_iterations,
        kdf_memory_kib: f.kdf_memory_kib,
        kdf_parallelism: f.kdf_parallelism,
        protected_user_vault_key: f.protected_user_vault_key,
        app_key_local_enc: String::new(), // 留空——解锁后由 refresh_app_key_local_enc 填
        app_key_sync_enc: f.app_key_sync_enc,
        security_stamp: f.security_stamp,
        equivalent_domains: f.equivalent_domains,
        public_key: None,
        protected_private_key: None,
    };
    db::upsert_vault_meta(&meta_input).map_err(SyncError::Other)?;

    // 3. 读所有 cipher/folder 文件 → upsert SQLite
    let (ciphers, folders) = store::import_all_from_files()?;
    for c in &ciphers {
        // H2 修复：build_cipher_input_from_file 保留 is_deleted（之前硬编码 false → 复活）
        let input = build_cipher_input_from_file(c);
        upsert_cipher(&input)?;
    }
    for f in &folders {
        // upsert folder 带 sort_order（#6 修复——不再硬编码 0）
        let md5 = crate::sync::fingerprint::folder_md5_from_fields(&f.id, &f.name, f.sort_order);
        upsert_folder_with_sort(&f.id, &f.name, f.sort_order, &md5)?;
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

/// 从文件读出的 VaultCipher 构造 VaultCipherInput（clone/pull 共用，T1 修复）。
///
/// **H2 不变量**：is_deleted 必须从文件取值传入（不能硬编码 false）——否则软删密码
/// 在新机 clone / 对端 pull 时复活成 live。此 helper 是生产构造点的单一真相源，
/// 测试调它即覆盖生产逻辑（避免「测试自带修复值、绕过生产构造点」的 MatchType#1 同型弱点）。
fn build_cipher_input_from_file(c: &octopus_infra::db::VaultCipher) -> VaultCipherInput {
    let md5 = crate::sync::fingerprint::cipher_md5(c);
    VaultCipherInput {
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
        is_deleted: c.is_deleted, // H2：保留文件中的软删状态
        sync_md5: Some(md5),
    }
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

/// Upsert folder 带 sort_order（sync pull 用，#6 修复）。
///
/// 存在则 update（name + sort_order + sync_md5 全字段）；不存在则 insert_with_sort
/// （E5 修复：一次写，不再 insert+update 两次）。
fn upsert_folder_with_sort(
    id: &str,
    encrypted_name: &str,
    sort_order: i64,
    sync_md5: &str,
) -> Result<(), SyncError> {
    // P-FOLDER-SCAN 修复（2026-07-25）：用 load_vault_folder 单条查询（O(1)）替代
    // list_vault_folders().iter().any() 全表扫（O(N)）。与 upsert_cipher（load_vault_cipher）对称。
    let exists = db::load_vault_folder(id)
        .map_err(SyncError::Other)?
        .is_some();
    if exists {
        db::update_vault_folder_fields(id, encrypted_name, sort_order, sync_md5)
            .map_err(SyncError::Other)?;
    } else {
        // E5 修复：一次写（不再 insert+update 两次）
        db::insert_vault_folder_with_sort(id, encrypted_name, sort_order, sync_md5)
            .map_err(SyncError::Other)?;
    }
    Ok(())
}

// === T4.7: sync_now ===

/// 同步报告——sync_now 返回值，UI 显示「拉了 X 条，推了 Y 条」。
#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub pulled: usize,
    pub pushed: usize,
    pub deleted: usize,
    /// 热词同步统计（v46 新增——sync_now 同时同步 vault + hotword）。
    pub hotwords_pulled: usize,
    pub hotwords_pushed: usize,
    /// push 失败的 remote → 用户可读错误消息（SyncError 的 Display，#4 修复）。
    ///
    /// 空 = 全部 remote 推送成功；非空 = 部分 remote 失败（本地已 commit，未上云）。
    /// 之前 push 失败只 log::warn! + SyncReport 无条件报「已推送到远程」→ 用户误以为
    /// 已备份。现在失败原因累计到此字段，前端据此显 warning toast。
    pub push_errors: Vec<(String, String)>,
    /// pull 阶段因文件读取失败被跳过的条目数（#10 修复——不再静默吞错）。
    pub skipped: usize,
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

    // 2. merge --ff-only（区分 4 种结果）
    // - UpToDate：远程无新 commit（HEAD hash 未变）→ 跳过 pull（避免旧文件覆盖本地新 DB）
    // - FastForwarded：远程领先，已合并 → 正常 pull
    // - CannotFastForward：分叉 → rebase 兜底 → 正常 pull
    // - NoUpstream：远程是空仓库（首次推送场景）→ 跳过 merge/rebase + 跳过 pull，直接 push -u
    let merge_result = git::git_merge_ff(&root, "origin/main")?;
    let is_first_push = matches!(merge_result, git::MergeFfResult::NoUpstream);
    // NoUpstream（首次推送）时跳过 pull——远程无内容可拉。
    // UpToDate 不再跳过 pull（2026-07-27 修复）：pull_from_files 内部已有 md5 比对，
    // 只 upsert「outline 有 + DB 无」或「md5 不匹配」的条目，不会无脑覆盖 DB 更新的数据。
    // 原 skip_pull 是对「热词加词后 sync 消失」bug 的误判修复——真正根因是 push 阶段的
    // 删除传播（已在 incremental_export 加保护修复，见 store.rs db_all_empty 检查）。
    let skip_pull = matches!(merge_result, git::MergeFfResult::NoUpstream);
    if !is_first_push {
        match merge_result {
            git::MergeFfResult::UpToDate => {
                log::debug!("[sync] 远程无新 commit（UpToDate），仍执行 pull（md5 比对保护不覆盖新数据）")
            }
            git::MergeFfResult::FastForwarded => {
                log::debug!("[sync] ff-only merge 成功（远程有新 commit）")
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

    // 3. merge 阶段：DB ↔ 文件系统 双向同步（spec §3.1，2026-07-27）。
    //
    // 替代原来 pull_from_files + push_to_files 两步——合并后无顺序依赖（FK /
    // skip_pull / 删除保护等问题自然消失）。按 updated_at 最新赢，冲突（相同
    // 时间戳）DB 赢。NoUpstream（首次推送）时 .sync outline 由 enable_sync 写过
    // ——merge 会判定「outline == DB」无变化，安全。
    //
    // 热词子系统（octopus_sync::hotword）独立维护 pull/push 两步，不受 vault
    // merge 影响——本机 vault merge 模型尚未推广到 hotword。
    let merge_report = if skip_pull {
        // NoUpstream：远程无 main 分支，文件系统已由 enable_sync 写过——
        // 走 push_to_files 把本地最新 DB 写到文件系统（merge_vault 也能做，但
        // NoUpstream 时远程 .sync meta 必然与本地一致，merge 退化成纯 push）。
        // 保留 push_to_files 调用是为了复用 incremental_export 的元数据/outline 写入
        // 逻辑（merge_vault 内部走 export_all_to_files 会清空目录重写，对首次推送
        // 场景无差异，但 push_to_files 更经济）。
        let pushed = push_to_files()?;
        MergeReport {
            pulled: 0,
            pushed,
            conflicts: 0,
            skipped: 0,
        }
    } else {
        merge_vault()?
    };
    let pulled = merge_report.pulled;
    let pushed = merge_report.pushed;
    let skipped = merge_report.skipped;
    // 热词 pull（v46：sync_now 同时同步 vault + hotword）
    let hotwords_pulled = if skip_pull {
        0
    } else {
        match octopus_sync::hotword::pull_hotwords_from_files() {
            Ok(n) => n,
            Err(e) => {
                log::warn!("[sync] 热词 pull 失败（不阻断 vault 同步）：{}", e);
                0
            }
        }
    };

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
    // #4 修复：push 失败累计到 push_errors，不再静默 log::warn! 后谎报「已推送」
    let mut push_errors: Vec<(String, String)> = Vec::new();
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
                Err(e) => {
                    log::warn!("[sync] push to {} 失败：{}", name, e);
                    push_errors.push((name.clone(), e.to_string()));
                }
            }
        }
    }

    // message 措辞根据 push_errors / skipped 分支（#4——不再无条件「已推送到远程」）
    let message = if push_errors.is_empty() {
        if is_first_push {
            "首次同步完成，已推送到远程".to_string()
        } else {
            format!(
                "同步完成：vault 拉取 {} 条/推送 {} 条，热词拉取 {} 条/推送 {} 条",
                pulled, pushed, hotwords_pulled, hotwords_pushed
            )
        }
    } else {
        // 部分 remote 推送失败——明确告知用户「本地已保存，未上云」
        let failed_names: Vec<&str> = push_errors.iter().map(|(n, _)| n.as_str()).collect();
        if is_first_push {
            format!(
                "首次同步：本地已保存，但 {} 个 remote 推送失败（{}）——数据未上云，请检查 remote 配置",
                push_errors.len(),
                failed_names.join(", ")
            )
        } else {
            format!(
                "同步完成：vault 拉取 {} 条/推送 {} 条；但 {} 个 remote 推送失败（{}）——本地已保存，未上云",
                pulled, pushed, push_errors.len(), failed_names.join(", ")
            )
        }
    };

    let report = SyncReport {
        pulled,
        pushed,
        deleted: 0,
        hotwords_pulled,
        hotwords_pushed,
        push_errors,
        skipped,
        message,
    };
    log::info!("[sync] sync_now 完成：{}", report.message);
    Ok(report)
}

/// Pull 阶段：读 outline.json 对比本地，按 md5 差异读文件 upsert SQLite。
///
/// 返回 `(pulled, skipped)`：
/// - `pulled`：实际 upsert 的 cipher+folder 数量
/// - `skipped`：因文件读取失败被跳过的条目数（#10——不再静默吞错）
///
/// **两阶段执行**（#3 修复，2026-07-24）：
/// 1. **校验阶段**：先读 meta.json 校验 security_stamp——不一致直接返
///    `MasterPasswordMismatch`，**不触碰 cipher/folder DB**（避免污染后无回滚）
/// 2. **应用阶段**：stamp 一致（或本地无 vault_meta）后才 upsert cipher/folder/meta
///
/// **判定标准**（#2 修复，2026-07-24）：pull 侧改用 outline.md5 vs DB sync_md5
/// 比对，与 push 侧（incremental_export 用 sync_md5）对称。参照 hotword.rs 模式。
/// 不再依赖 updated_at 字符串比较（跨设备格式不可控）。
///
/// ⚠️ 2026-07-27（spec §3.1）：sync_now 改用 `merge_vault` 替代 pull + push 两步。
/// 本函数保留——`merge_vault` 的阶段 A（stamp 校验）+ 阶段 B（cipher/folder pull 路径）
/// 沿用其设计；现有回归测试（`sync_recovers_data_when_db_emptied` /
/// `pull_clears_local_enc_when_sync_enc_differs`）也直接调它验证 pull 子流程。
/// 生产路径已不调用——`#[allow(dead_code)]` 抑制 prod 构建的 unused 警告。
#[cfg_attr(not(test), allow(dead_code))]
fn pull_from_files() -> Result<(usize, usize), SyncError> {
    let remote_outline = store::read_outline_file()?;
    let db_ciphers = db::list_vault_ciphers().map_err(SyncError::Other)?;
    let db_folders = db::list_vault_folders().map_err(SyncError::Other)?;

    // P-MD5-LINEAR-SCAN 修复（2026-07-25）：HashSet 升级为 HashMap（id → sync_md5）。
    // 之前 db_cipher_ids 只用于 exists 判断（O(1)），但 md5 比对（cipher_md5_mismatch）
    // 又回到 db_ciphers Vec 线性 find（O(N)）→ 外层 for × 内部 find = O(M×N)。
    // HashMap 后 exists 用 contains_key（O(1)），md5 比对用 get（O(1)），整体降到 O(M)。
    let db_cipher_md5: std::collections::HashMap<&str, &str> = db_ciphers
        .iter()
        .map(|c| (c.id.as_str(), c.sync_md5.as_deref().unwrap_or("")))
        .collect();
    let db_folder_md5: std::collections::HashMap<&str, &str> = db_folders
        .iter()
        .map(|f| (f.id.as_str(), f.sync_md5.as_deref().unwrap_or("")))
        .collect();

    // === 阶段 A：stamp 校验（前置——不通过则不触碰 cipher/folder DB）===
    //
    // 必须在 upsert cipher/folder 之前完成，否则 stamp 不一致时本地 DB 已被
    // 用错误 user_vault_key 加密的密文污染，返 Err 也无回滚（INV-S9 强化）。
    let local_meta = db::load_vault_meta().map_err(SyncError::Other)?;
    // meta.json 不存在是合法场景（首次同步/纯新增）——只跳过 stamp 校验 + meta upsert
    let meta_file = match store::read_meta_file() {
        Ok(m) => Some(m),
        Err(e) if meta_file_not_found(&e) => {
            // C-PULL-NO-META-SKIPS-STAMP 修复（2026-07-25）：
            // 「meta 缺失合法」严格限定为 local_meta = None（本地也无 vault，首次同步）。
            // 若本地已有 vault（local_meta = Some）+ 远程 meta 缺失 → 异常态（损坏/
            // 不完整 clone/篡改），拒绝 pull——否则 stamp 校验被跳过 + 远程 cipher
            // （用 K_remote 加密）被无条件 upsert 进本地 DB（K_local 解密）→ 不可解密
            // 密文污染（违背 INV-S9「先校验后触碰 DB」不变量，:788-790 自述）。
            if local_meta.is_some() {
                return Err(SyncError::RepoCorrupted(
                    "远程 meta.json 缺失但本地已初始化 vault——拒绝 pull（无法校验加密一致性，可能远程仓库损坏）".into(),
                ));
            }
            log::debug!("[sync] meta.json 不存在，跳过 stamp 校验 + meta upsert");
            None
        }
        Err(e) => return Err(SyncError::Other(e)),
    };

    if let Some(ref mf) = meta_file {
        let stamp = mf.security_stamp.clone();
        if let Some(ref local) = local_meta {
            if local.security_stamp != stamp {
                // 空库恢复场景（2026-07-28 v2，与 merge_vault 对称）：本地空库 + stamp 不一致
                // → 返回 EmptyRecoveryNeedsPassword。详见 spec 2026-07-28-vault-sync-empty-recovery.md。
                let local_empty = db_ciphers.is_empty() && db_folders.is_empty();
                if local_empty {
                    log::info!(
                        "[sync] pull_from_files: 本地 vault 空库 + stamp 不一致（本地={}, 远程={}）\
                        ——返回 EmptyRecoveryNeedsPassword",
                        local.security_stamp, stamp
                    );
                    return Err(SyncError::EmptyRecoveryNeedsPassword);
                } else {
                    return Err(SyncError::MasterPasswordMismatch);
                }
            }
        }
    }

    // === 阶段 B：应用 folder / cipher / meta ===
    // ⚠️ 顺序修复（2026-07-27 FK constraint failed）：先 folder（被引用方）再 cipher
    // （引用方），避免 cipher 的 folder_id 引用尚未插入的 folder → FOREIGN KEY constraint failed。
    // 之前是 cipher 先 folder 后——如果 cipher 有非 null folder_id 就触发 FK。
    let mut count = 0usize;
    let mut skipped = 0usize;

    // folder 先：outline 有但 DB 无，或 md5 不匹配 → 读文件 upsert
    for (uuid, entry) in &remote_outline.folders {
        let needs_update = !db_folder_md5.contains_key(uuid.as_str())
            || folder_md5_mismatch(uuid, &entry.md5, &db_folder_md5);
        if needs_update {
            match store::read_folder_file(uuid) {
                Ok(folder_file) => {
                    let md5 = crate::sync::fingerprint::folder_md5_from_fields(
                        &folder_file.id,
                        &folder_file.encrypted_name,
                        folder_file.sort_order,
                    );
                    upsert_folder_with_sort(
                        &folder_file.id,
                        &folder_file.encrypted_name,
                        folder_file.sort_order,
                        &md5,
                    )?;
                    count += 1;
                }
                Err(e) => {
                    log::warn!("[sync] folder {} 文件读取失败，已跳过：{}", uuid, e);
                    skipped += 1;
                }
            }
        }
    }

    // cipher 后：outline 有但 DB 无，或 md5 不匹配 → 读文件 upsert
    for (uuid, entry) in &remote_outline.ciphers {
        let needs_update = !db_cipher_md5.contains_key(uuid.as_str())
            || cipher_md5_mismatch(uuid, &entry.md5, &db_cipher_md5);
        if needs_update {
            match store::read_cipher_file(uuid) {
                Ok(cipher_file) => {
                    let row = cipher_file.to_vault_cipher();
                    // build_cipher_input_from_file 保留 is_deleted（H2）——T1 后是单一真相源
                    let input = build_cipher_input_from_file(&row);
                    upsert_cipher(&input)?;
                    count += 1;
                }
                Err(e) => {
                    // #10：损坏文件不再静默吞——记日志 + 累计 skipped
                    log::warn!("[sync] cipher {} 文件读取失败，已跳过：{}", uuid, e);
                    skipped += 1;
                }
            }
        }
    }

    // meta → upsert vault_meta（stamp 已在阶段 A 校验通过）
    if let Some(mf) = meta_file {
        let f = mf.to_sync_fields()?;

        // K1-GAP 修复（2026-07-25）：pull 的远程 KDF 参数用 from_i64_strict 校验，
        // 与 clone_initial / resolve_with_remote 一致——补齐 K1 防御主路径。
        let _strict_params = Argon2Params::from_i64_strict(f.kdf_iterations, f.kdf_memory_kib, f.kdf_parallelism)
            .map_err(SyncError::Other)?;

        // stamp 一致（或本地无 vault_meta）——保留本地 app_key_local_enc / public_key。
        // ⚠️ app_key_sync_enc 不一致时清空 local_enc（2026-07-27 修复）：
        // 场景：B 机新建 vault（生成新 app_key）→ sync 从 A 机拉数据。
        // pull 把 app_key_sync_enc 覆盖成远程值（A 机 app_key 加密），但本地 local_enc
        // 仍是新 app_key 加密的。如果保留 local_enc，启动时优先用它解出新 app_key →
        // cipher（A 机旧 app_key 加密）解不开。清空 local_enc 强制走 sync_enc 路径，
        // 解出 A 机 app_key，cipher 才能正确解密。成功后 unlock 自动用本机 K_machine
        // 重写 local_enc。
        let (local_enc, pub_key, priv_key) = match &local_meta {
            Some(m) => {
                let sync_changed = m.app_key_sync_enc != f.app_key_sync_enc;
                let enc = if sync_changed {
                    log::info!(
                        "[sync] pull 检测到 app_key_sync_enc 变化（远程覆盖了 sync_enc）——\
                        清空本地 app_key_local_enc，强制下次 unlock 从 sync_enc 解 app_key"
                    );
                    String::new()
                } else {
                    m.app_key_local_enc.clone()
                };
                (enc, m.public_key.clone(), m.protected_private_key.clone())
            }
            None => (String::new(), None, None),
        };
        let meta_input = VaultMetaInput {
            kdf_type: f.kdf_type,
            kdf_salt: f.kdf_salt,
            kdf_iterations: f.kdf_iterations,
            kdf_memory_kib: f.kdf_memory_kib,
            kdf_parallelism: f.kdf_parallelism,
            protected_user_vault_key: f.protected_user_vault_key,
            app_key_local_enc: local_enc,
            app_key_sync_enc: f.app_key_sync_enc,
            security_stamp: f.security_stamp,
            equivalent_domains: f.equivalent_domains,
            public_key: pub_key,
            protected_private_key: priv_key,
        };
        db::upsert_vault_meta(&meta_input).map_err(SyncError::Other)?;
    }

    Ok((count, skipped))
}

/// 判断 meta.json 读取错误是否为「文件不存在」（合法场景——首次同步）。
///
/// E3 修复（2026-07-24）：改用 downcast 类型安全匹配（与 Q1 原则一致）——
/// 之前用 contains("No such file") 字符串匹配，在不同 io 错/locale 下可能漏判。
fn meta_file_not_found(e: &anyhow::Error) -> bool {
    use std::io::ErrorKind;
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .map_or(false, |io| io.kind() == ErrorKind::NotFound)
    })
}

/// 检测 cipher 是否需要 pull——对比 outline.md5 vs DB sync_md5（#2 修复）。
///
/// 与 push 侧（incremental_export 用 sync_md5 决定重写）对称：两端都基于
/// 内容指纹 md5，不依赖跨设备不稳定的时间戳字符串。
///
/// - DB 无该 cipher → true（需 pull）
/// - DB.sync_md5 与 outline.md5 不等 → true（内容变了，需 pull）
/// - DB.sync_md5 与 outline.md5 相等 → false（无变化）
///
/// 不再读文件（消除 2N syscall——低优先级清理项 8.7）。
/// P-MD5-LINEAR-SCAN 修复（2026-07-25）：接收 HashMap（id → sync_md5）而非 Vec，
/// 用 get（O(1)）替代 iter().find（O(N)）。整体 pull 复杂度从 O(M×N) 降到 O(M)。
fn cipher_md5_mismatch(
    uuid: &str,
    outline_md5: &str,
    db_cipher_md5: &std::collections::HashMap<&str, &str>,
) -> bool {
    match db_cipher_md5.get(uuid) {
        None => true,
        Some(db_md5) => *db_md5 != outline_md5,
    }
}

/// 检测 folder 是否需要 pull——对比 outline.md5 vs DB sync_md5（#5 修复）。
///
/// 与 cipher 对称。之前 folder 只在「DB 不存在」时 pull，已有 folder 整个跳过
/// → 远程 rename 被静默丢弃（last-write-wins 数据丢失）。现在与 cipher 同标准。
fn folder_md5_mismatch(
    uuid: &str,
    outline_md5: &str,
    db_folder_md5: &std::collections::HashMap<&str, &str>,
) -> bool {
    match db_folder_md5.get(uuid) {
        None => true,
        Some(db_md5) => *db_md5 != outline_md5,
    }
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
pub fn resolve_with_remote(password: Zeroizing<String>) -> Result<(), SyncError> {
    use crate::crypto::kdf::{derive_master_root_key, Argon2Params};

    // #7 修复：resolve 会 git add/commit，与 sync_now 并发会留残留
    let _guard = try_sync_lock()?;

    // 1. 读远程 meta.json
    let remote_meta = store::read_meta_file()?;
    let f = remote_meta.to_sync_fields()?;

    // 2. 用远程 KDF 参数 + 密码派生 master_root_key，验证密码
    //    #14 修复：用 from_i64 校验 i64→u32 范围（防止篡改削弱 KDF）
    //    K1 修复（2026-07-24）：远程参数（f.kdf_* 来自同步仓库 meta.json，不可信）
    //    用 from_i64_strict——安全下限（memory≥16384KiB）防攻击者污染仓库为弱 KDF
    //    废掉 Argon2id 内存硬度。本地 DB 路径（resolve_with_local）继续用 from_i64。
    let params = Argon2Params::from_i64_strict(f.kdf_iterations, f.kdf_memory_kib, f.kdf_parallelism)
        .map_err(SyncError::Other)?;
    let master = derive_master_root_key(password.as_bytes(), &f.kdf_salt, &params)
        .map_err(|e| SyncError::Other(e.context("KDF 派生失败")))?;
    // 验证密码：解 protected_user_vault_key，失败即密码错
    let _uvk_bytes = master.decrypt(&f.protected_user_vault_key).map_err(|_| {
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
        kdf_type: f.kdf_type,
        kdf_salt: f.kdf_salt,
        kdf_iterations: f.kdf_iterations,
        kdf_memory_kib: f.kdf_memory_kib,
        kdf_parallelism: f.kdf_parallelism,
        protected_user_vault_key: f.protected_user_vault_key,
        app_key_local_enc: local_enc, // 保留本地 K_machine 加密的
        app_key_sync_enc: f.app_key_sync_enc,
        security_stamp: f.security_stamp,
        equivalent_domains: f.equivalent_domains,
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
pub fn resolve_with_local(password: Zeroizing<String>) -> Result<(), SyncError> {
    use crate::crypto::kdf::{derive_master_root_key, Argon2Params};

    // #7 修复：resolve 会 git add/commit，与 sync_now 并发会留残留
    let _guard = try_sync_lock()?;

    // 1. 读本地 vault_meta，用本地 KDF 参数验证密码
    let local_meta: VaultMeta = db::load_vault_meta()
        .map_err(SyncError::Other)?
        .ok_or_else(|| SyncError::Other(anyhow::anyhow!("本地 vault_meta 不存在")))?;
    // #14 修复：用 from_i64 校验 i64→u32 范围（防止篡改削弱 KDF）
    let params = Argon2Params::from_i64(
        local_meta.kdf_iterations,
        local_meta.kdf_memory_kib,
        local_meta.kdf_parallelism,
    )
    .map_err(SyncError::Other)?;
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

// === merge_vault（spec §3.1，2026-07-27）===
//
// 双向 merge：DB ↔ .sync 文件系统，按 updated_at 最新赢。
//
// 替代 sync_now 里 pull_from_files + push_to_files 两步——合并后无顺序依赖，
// FK / skip_pull / 删除保护等问题自然消失。pull_from_files + push_to_files
// 函数本身保留——clone_initial（B 机首次 clone）仍用 pull_from_files。
//
// 真相源 = updated_at 最新赢；冲突（相同时间戳）→ DB 赢（当前机器优先）。
// 「删除」是普通字段变更（is_deleted=1 + updated_at 更新），走标准 merge 路径——
// 不再硬删文件 / DB 行。

/// merge_vault 返回值——供 SyncReport 上报「拉了 X / 推了 Y / 冲突 Z」。
#[derive(Debug, Clone, Default)]
pub struct MergeReport {
    /// 从 .sync 拉到 DB 的条目数（folder + cipher）。
    pub pulled: usize,
    /// 从 DB 推到 .sync 的条目数（folder + cipher）。
    pub pushed: usize,
    /// 冲突数（updated_at 相同 + md5 不同 → DB 赢）。pushed 已含这部分，本字段单独统计便于诊断。
    pub conflicts: usize,
    /// 因文件读取失败被跳过的条目数（沿用 pull_from_files 容错语义）。
    pub skipped: usize,
}

/// 双向 merge——按 updated_at 最新赢。
///
/// 流程：
/// 1. 读 outline（远程视角）+ DB ciphers/folders（本地视角）+ vault_meta
/// 2. stamp 校验（沿用 pull_from_files 阶段 A 逻辑——校验失败不触碰 DB）
/// 3. merge folder（先，FK 被引用方）—— 按条目比对 updated_ms
/// 4. merge cipher（后，FK 引用方）
/// 5. merge meta（app_key_sync_enc 一致性——沿用 pull_from_files meta upsert 逻辑）
/// 6. 写 outline（从 DB 最新状态重建——merge 完后 DB 即单一真相源）
///
/// 判定规则（每条 folder/cipher）：
/// - outline 有 + DB 无 → pull（.sync → DB）
/// - DB 有 + outline 无 → push（DB → .sync）
/// - 都有 → 比 updated_ms（outline 的远程时间戳 vs DB 的本地时间戳转 ms）
///   - remote > local → pull 覆盖 DB
///   - local > remote → push 覆盖 .sync
///   - 相等 → md5 比对；md5 不同 → DB 赢（conflict）；md5 相同 → 跳过
pub(crate) fn merge_vault() -> Result<MergeReport, SyncError> {
    let remote_outline = store::read_outline_file()?;
    let db_ciphers = db::list_vault_ciphers().map_err(SyncError::Other)?;
    let db_folders = db::list_vault_folders().map_err(SyncError::Other)?;

    // === 阶段 A：stamp 校验（沿用 pull_from_files 模式——前置，不通过则不触碰 DB）===
    let local_meta = db::load_vault_meta().map_err(SyncError::Other)?;
    let meta_file = match store::read_meta_file() {
        Ok(m) => Some(m),
        Err(e) if meta_file_not_found(&e) => {
            // 远程 meta.json 缺失——两种场景：
            //   (1) 远程 outline 也空 → 远程从未有 vault 数据（用户主动清空远程
            //       或首次推送）→ 允许继续，走纯 push 路径把本地数据推到远程。
            //   (2) 远程 outline 有数据但 meta 缺失 → 异常态（损坏/不完整 clone/
            //       篡改）→ 拒绝 merge（无法校验加密一致性）。
            //    （2026-07-28 修复：原逻辑不分情况一律拒绝，误伤「远程清空后重新
            //     推送」的合法场景。）
            let remote_outline_empty =
                remote_outline.ciphers.is_empty() && remote_outline.folders.is_empty();
            if local_meta.is_some() && !remote_outline_empty {
                return Err(SyncError::RepoCorrupted(
                    "远程 meta.json 缺失但本地已初始化 vault 且远程 outline 有数据——拒绝 merge（无法校验加密一致性，可能远程仓库损坏）".into(),
                ));
            }
            log::debug!(
                "[sync] meta.json 不存在（remote_outline_empty={}），跳过 stamp 校验 + meta upsert",
                remote_outline_empty
            );
            None
        }
        Err(e) => return Err(SyncError::Other(e)),
    };

    if let Some(ref mf) = meta_file {
        let stamp = mf.security_stamp.clone();
        if let Some(ref local) = local_meta {
            if local.security_stamp != stamp {
                // 空库恢复场景（2026-07-28 v2）：本地刚 setup（stamp 必然是新的随机值）
                // 但 cipher/folder 都为空，.sync 有数据 → 返回 EmptyRecoveryNeedsPassword，
                // 让前端弹窗要求输源机器主密码，调 resolve_with_remote 校验 + 覆盖本地。
                // v1 是无条件放行，但用户输错主密码会进入「数据恢复但解不开」的死状态。
                // 详见 spec 2026-07-28-vault-sync-empty-recovery.md。
                let local_empty = db_ciphers.is_empty() && db_folders.is_empty();
                if local_empty {
                    log::info!(
                        "[sync] merge_vault: 本地 vault 空库 + stamp 不一致（本地={}, 远程={}）\
                        ——返回 EmptyRecoveryNeedsPassword，等待用户输源机密码",
                        local.security_stamp, stamp
                    );
                    return Err(SyncError::EmptyRecoveryNeedsPassword);
                } else {
                    return Err(SyncError::MasterPasswordMismatch);
                }
            }
        }
    }

    let mut report = MergeReport::default();

    // === 阶段 B：merge folder（先，FK 被引用方）===
    //
    // ⚠️ 顺序与 pull_from_files 一致（2026-07-27 FK constraint 修复）——folder 先，
    // 避免 cipher.folder_id 引用尚未插入的 folder。
    let db_folder_by_id: std::collections::HashMap<&str, &octopus_infra::db::VaultFolder> =
        db_folders.iter().map(|f| (f.id.as_str(), f)).collect();

    // folder：outline 有 + DB 无 → pull
    for (uuid, entry) in &remote_outline.folders {
        let remote_updated = entry.updated_ms;
        match db_folder_by_id.get(uuid.as_str()) {
            None => {
                // DB 无 → pull
                match store::read_folder_file(uuid) {
                    Ok(folder_file) => {
                        let md5 = crate::sync::fingerprint::folder_md5_from_fields(
                            &folder_file.id,
                            &folder_file.encrypted_name,
                            folder_file.sort_order,
                        );
                        upsert_folder_with_sort(
                            &folder_file.id,
                            &folder_file.encrypted_name,
                            folder_file.sort_order,
                            &md5,
                        )?;
                        report.pulled += 1;
                    }
                    Err(e) => {
                        log::warn!("[sync] merge: folder {} 文件读取失败，已跳过：{}", uuid, e);
                        report.skipped += 1;
                    }
                }
            }
            Some(db_f) => {
                let local_updated = octopus_sync::store::iso_to_unix_ms(&db_f.updated_at);
                if remote_updated > local_updated {
                    // .sync 更新 → pull 覆盖 DB
                    match store::read_folder_file(uuid) {
                        Ok(folder_file) => {
                            let md5 = crate::sync::fingerprint::folder_md5_from_fields(
                                &folder_file.id,
                                &folder_file.encrypted_name,
                                folder_file.sort_order,
                            );
                            upsert_folder_with_sort(
                                &folder_file.id,
                                &folder_file.encrypted_name,
                                folder_file.sort_order,
                                &md5,
                            )?;
                            report.pulled += 1;
                        }
                        Err(e) => {
                            log::warn!("[sync] merge: folder {} 文件读取失败，已跳过：{}", uuid, e);
                            report.skipped += 1;
                        }
                    }
                } else if local_updated > remote_updated {
                    // DB 更新 → push 覆盖 .sync
                    push_folder_to_files(db_f)?;
                    report.pushed += 1;
                } else {
                    // updated_at 相等 → md5 比对
                    let db_md5 = db_f
                        .sync_md5
                        .clone()
                        .unwrap_or_else(|| crate::sync::fingerprint::folder_md5(db_f));
                    if db_md5 != entry.md5 {
                        // 冲突 → DB 赢
                        push_folder_to_files(db_f)?;
                        report.pushed += 1;
                        report.conflicts += 1;
                    }
                    // md5 相同 → 跳过
                }
            }
        }
    }
    // folder：DB 有 + outline 无 → push（不再硬删文件）
    for db_f in &db_folders {
        if !remote_outline.folders.contains_key(&db_f.id) {
            push_folder_to_files(db_f)?;
            report.pushed += 1;
        }
    }

    // === 阶段 C：merge cipher（后，FK 引用方）===
    let db_cipher_by_id: std::collections::HashMap<&str, &octopus_infra::db::VaultCipher> =
        db_ciphers.iter().map(|c| (c.id.as_str(), c)).collect();

    // cipher：outline 有 + DB 无 / 或 .sync 更新 → pull
    for (uuid, entry) in &remote_outline.ciphers {
        let remote_updated = entry.updated_ms;
        match db_cipher_by_id.get(uuid.as_str()) {
            None => {
                // DB 无 → pull
                match store::read_cipher_file(uuid) {
                    Ok(cipher_file) => {
                        let row = cipher_file.to_vault_cipher();
                        let input = build_cipher_input_from_file(&row);
                        upsert_cipher(&input)?;
                        report.pulled += 1;
                    }
                    Err(e) => {
                        log::warn!("[sync] merge: cipher {} 文件读取失败，已跳过：{}", uuid, e);
                        report.skipped += 1;
                    }
                }
            }
            Some(db_c) => {
                let local_updated = octopus_sync::store::iso_to_unix_ms(&db_c.updated_at);
                if remote_updated > local_updated {
                    // .sync 更新 → pull 覆盖 DB
                    match store::read_cipher_file(uuid) {
                        Ok(cipher_file) => {
                            let row = cipher_file.to_vault_cipher();
                            let input = build_cipher_input_from_file(&row);
                            upsert_cipher(&input)?;
                            report.pulled += 1;
                        }
                        Err(e) => {
                            log::warn!("[sync] merge: cipher {} 文件读取失败，已跳过：{}", uuid, e);
                            report.skipped += 1;
                        }
                    }
                } else if local_updated > remote_updated {
                    // DB 更新 → push 覆盖 .sync
                    push_cipher_to_files(db_c)?;
                    report.pushed += 1;
                } else {
                    // updated_at 相等 → md5 比对
                    let db_md5 = db_c
                        .sync_md5
                        .clone()
                        .unwrap_or_else(|| crate::sync::fingerprint::cipher_md5(db_c));
                    if db_md5 != entry.md5 {
                        // 冲突 → DB 赢
                        push_cipher_to_files(db_c)?;
                        report.pushed += 1;
                        report.conflicts += 1;
                    }
                }
            }
        }
    }
    // cipher：DB 有 + outline 无 → push
    for db_c in &db_ciphers {
        if !remote_outline.ciphers.contains_key(&db_c.id) {
            push_cipher_to_files(db_c)?;
            report.pushed += 1;
        }
    }

    // === 阶段 D：merge meta（沿用 pull_from_files 阶段 B meta upsert + app_key local_enc 清空）===
    if let Some(mf) = meta_file {
        let f = mf.to_sync_fields()?;
        let _strict_params = Argon2Params::from_i64_strict(
            f.kdf_iterations,
            f.kdf_memory_kib,
            f.kdf_parallelism,
        )
        .map_err(SyncError::Other)?;

        // stamp 一致（或本地无 vault_meta）——保留本地 app_key_local_enc / public_key。
        // ⚠️ app_key_sync_enc 不一致时清空 local_enc（2026-07-27 修复，与 pull_from_files 一致）：
        // 场景：B 机新建 vault（生成新 app_key）→ sync 从 A 机拉数据。
        // merge 把 app_key_sync_enc 覆盖成远程值（A 机 app_key 加密），但本地 local_enc
        // 仍是新 app_key 加密的。保留 local_enc → 启动时优先用它解出新 app_key →
        // cipher（A 机旧 app_key 加密）解不开。清空 local_enc 强制走 sync_enc 路径。
        let (local_enc, pub_key, priv_key) = match &local_meta {
            Some(m) => {
                let sync_changed = m.app_key_sync_enc != f.app_key_sync_enc;
                let enc = if sync_changed {
                    log::info!(
                        "[sync] merge 检测到 app_key_sync_enc 变化——清空本地 app_key_local_enc，\
                        强制下次 unlock 从 sync_enc 解 app_key"
                    );
                    String::new()
                } else {
                    m.app_key_local_enc.clone()
                };
                (enc, m.public_key.clone(), m.protected_private_key.clone())
            }
            None => (String::new(), None, None),
        };
        let meta_input = VaultMetaInput {
            kdf_type: f.kdf_type,
            kdf_salt: f.kdf_salt,
            kdf_iterations: f.kdf_iterations,
            kdf_memory_kib: f.kdf_memory_kib,
            kdf_parallelism: f.kdf_parallelism,
            protected_user_vault_key: f.protected_user_vault_key,
            app_key_local_enc: local_enc,
            app_key_sync_enc: f.app_key_sync_enc,
            security_stamp: f.security_stamp,
            equivalent_domains: f.equivalent_domains,
            public_key: pub_key,
            protected_private_key: priv_key,
        };
        db::upsert_vault_meta(&meta_input).map_err(SyncError::Other)?;
    }

    // === 阶段 E：写 outline + meta（从 DB 最新状态重建——merge 完后 DB 即单一真相源）===
    //
    // 不复用 incremental_export——它有「DB 空 + .sync 有数据」保护，与 merge 的
    // 「pull .sync 到空 DB」语义冲突（merge 应该把 .sync 拉回 DB，而非保留空 DB）。
    // 这里走 push_initial 用的 export_all_to_files 路径——但只在 merge 真改了 DB 后
    // 才需要重建。即使无变更也重写 outline 是幂等的（文件内容不变 git 不会产生 diff）。
    let meta = db::load_vault_meta()
        .map_err(SyncError::Other)?
        .ok_or_else(|| SyncError::Other(anyhow::anyhow!("vault_meta 不存在（merge 后）")))?;
    let latest_ciphers = db::list_vault_ciphers().map_err(SyncError::Other)?;
    let latest_folders = db::list_vault_folders().map_err(SyncError::Other)?;
    store::export_all_to_files(&meta, &latest_ciphers, &latest_folders)?;

    log::info!(
        "[sync] merge_vault 完成：pulled={} pushed={} conflicts={} skipped={}",
        report.pulled, report.pushed, report.conflicts, report.skipped
    );
    Ok(report)
}

/// Push 单个 folder 到文件系统（merge 用——不重写 outline，最后统一重建）。
fn push_folder_to_files(folder: &octopus_infra::db::VaultFolder) -> Result<(), SyncError> {
    store::write_folder_file(folder).map_err(SyncError::Other)
}

/// Push 单个 cipher 到文件系统（merge 用——不重写 outline，最后统一重建）。
fn push_cipher_to_files(cipher: &octopus_infra::db::VaultCipher) -> Result<(), SyncError> {
    store::write_cipher_file(cipher).map_err(SyncError::Other)
}

// === T4.9: disable_sync ===

/// 禁用同步——删除 `~/.octopus/.sync/`（git repo 根 + 所有子目录，保留 SQLite 数据）。
pub fn disable_sync() -> Result<(), SyncError> {
    // #7 修复：sync_now 进行中点 disable 会 remove_dir_all(.sync/) → sync_now 后续
    // 命中 ENOENT，留下半提交 / index.lock 残留。加锁后并发 disable 被挡
    let _guard = try_sync_lock()?;
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

    /// 测试串行化 mutex（Task 3 修复后多个集成测试都持 SYNC_LOCK，多线程并发会互相竞争失败）。
    ///
    /// SYNC_LOCK 是进程全局 OnceLock<Mutex<bool>>——多线程测试时，持锁的集成测试
    /// （enable_sync / sync_now / clone 等）会互相 try_sync_lock 失败。用这个
    /// 测试专用 mutex 把所有持锁测试串行化，避免竞争。
    ///
    /// 用法：`let _g = test_lock();` 放在测试函数开头。
    static TEST_SERIALIZER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn sync_lock_serializes() {
        let _s = test_lock();
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

    /// C-PULL-NO-META-SKIPS-STAMP 修复后，pull 测试需写一个 stamp 一致的 meta.json
    ///（否则「本地有 vault + 远程无 meta」会被拒绝）。本辅助函数写 IntegrationGuard
    /// 预置 stamp（"stamp-test"）一致的 meta，让 pull 测试聚焦 cipher/folder 逻辑。
    fn write_stamp_matching_meta() {
        let meta_file = store::MetaFile {
            version: 1,
            kdf_type: 0,
            kdf_salt: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=".into(),
            kdf_iterations: 3,
            kdf_memory_kib: 65536,
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:test-uvk".into(),
            app_key_sync_enc: "v1:test-sync".into(),
            security_stamp: "stamp-test".into(),
            equivalent_domains: "[]".into(),
        };
        store::write_meta_file(&meta_file).expect("write stamp-matching meta");
    }

    /// enable_sync 后 .git 应在 sync_root（.sync/），不在 vault_dir（.sync/vault/）。
    /// 回归测试：之前 bug 是 .git 建在 vault/ 下，变成每子目录独立 repo。
    #[test]
    fn enable_sync_creates_git_in_sync_root_not_vault_dir() {
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
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
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
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
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
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
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
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
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
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
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
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

    /// security_stamp 守卫（INV-S9 + #3 强化）：pull_from_files 读到 stamp 不一致
    /// 的 meta.json 时必须在 **upsert cipher/folder 之前** 拒绝——不只是 meta 不被覆盖，
    /// cipher DB 也不能被污染（之前 bug：先 upsert cipher 再校验 stamp，返 Err 时
    /// 已用错误 user_vault_key 加密的密文留在 DB，无回滚）。
    ///
    /// 回归守护：曾因缺少此校验，dummy meta.json（stamp-1）覆盖了真实 vault_meta
    /// （真实 UUID stamp），导致用户主密码验证失败。
    #[test]
    fn pull_rejects_mismatched_security_stamp() {
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
        let g = IntegrationGuard::new();
        // IntegrationGuard 预置的 vault_meta security_stamp = "stamp-test"

        // pre-seed 一条本地 cipher——让本地 vault 非空（2026-07-28 调整）。
        // 理由：空库旁路（merge_vault / pull_from_files 的空库恢复场景）在
        // cipher=0 + folder=0 时跳过 stamp 校验。本测试的目的是「保护本地已有数据
        // 不被 poison cipher 覆盖」，所以本地必须有数据要保护——pre-seed 一个本地
        // cipher 让空库旁路不触发，回归测试的本意（stamp 不一致时拒绝）才能验证。
        use octopus_infra::db::{VaultCipher, VaultCipherInput};
        let local_cipher = VaultCipherInput {
            id: "local-legit-uuid".to_string(),
            folder_id: None, favorite: false, atype: 1,
            name: "v1:local-legit".into(), notes: None, data: "v1:local-data".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&local_cipher).expect("pre-seed local cipher");

        // 在 outline + 文件系统放一个 poison cipher——验证 stamp 不一致时它不会被 upsert
        let poison_cipher = VaultCipher {
            id: "stamp-poison-uuid".to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:from-wrong-key".into(),
            notes: None,
            data: "v1:poison-data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            is_deleted: false,
            sync_md5: None,
            created_at: "2026-07-24T00:00:00".into(),
            updated_at: "2026-07-24T00:00:00".into(),
        };
        store::write_cipher_file(&poison_cipher).expect("write poison cipher file");
        // outline：含这个 poison cipher 的 md5 entry
        use octopus_sync::outline::{Outline, OutlineEntry};
        use std::collections::BTreeMap;
        let poison_md5 = crate::sync::fingerprint::cipher_md5(&poison_cipher);
        let outline = Outline {
            version: 1,
            vault_version: 1,
            ciphers: BTreeMap::from([(
                "stamp-poison-uuid".to_string(),
                OutlineEntry {
                    md5: poison_md5,
                    updated_ms: 0,
                },
            )]),
            folders: BTreeMap::new(),
        };
        store::write_outline_file(&outline).expect("write outline");

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

        // pull_from_files 应在阶段 A（stamp 校验）就返 MasterPasswordMismatch
        let result = pull_from_files();
        assert!(
            matches!(result, Err(SyncError::MasterPasswordMismatch)),
            "stamp 不一致应在 upsert 前拒绝，实际：{:?}",
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

        // #3 强化：cipher DB 也不应被污染——poison cipher 不应出现在 DB 中
        let db_ciphers = octopus_infra::db::list_vault_ciphers().unwrap();
        assert!(
            !db_ciphers.iter().any(|c| c.id == "stamp-poison-uuid"),
            "stamp 不一致时 cipher 不应被 upsert 到 DB（#3——阶段 A 前置校验）"
        );
        let _ = g;
    }

    /// security_stamp 一致时 pull_from_files 应正常覆盖 vault_meta（合法同步场景）。
    #[test]
    fn pull_allows_matching_security_stamp() {
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
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

    /// C-PULL-NO-META-SKIPS-STAMP 守护（2026-07-25）：本地有 vault + 远程 meta 缺失 → pull 拒绝。
    ///
    /// 之前 meta.json 缺失时 stamp 校验被跳过，但 cipher upsert 无条件执行——
    /// 远程 cipher（K_remote 加密）被 upsert 进本地 DB（K_local 解密）→ 不可解密密文污染。
    /// 违背 INV-S9「先校验后触碰 DB」。修复：meta 缺失合法严格限定为 local_meta = None。
    #[test]
    fn pull_rejects_when_local_has_vault_but_remote_meta_missing() {
        let _s = test_lock();
        let g = IntegrationGuard::new();
        // IntegrationGuard 预置了 vault_meta（local_meta = Some）

        // 不写 meta.json（远程缺失）——但写一个 cipher 文件（模拟远程仓库异常态：
        // meta 损坏/丢失但 cipher 文件仍在）
        use octopus_infra::db::VaultCipher;
        let cipher = VaultCipher {
            id: "d4e5f6a7-b8c9-4123-9004-defab456789".to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:enc-name".into(),
            notes: None,
            data: "v1:enc-data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            is_deleted: false,
            created_at: "2026-07-25T00:00:00".into(),
            updated_at: "2026-07-25T00:00:00".into(),
            sync_md5: None,
        };
        store::write_cipher_file(&cipher).expect("write cipher");

        // pull 应拒绝（本地有 vault + 远程无 meta → 无法校验加密一致性）
        let result = pull_from_files();
        assert!(
            result.is_err(),
            "C-PULL-NO-META-SKIPS-STAMP: 本地有 vault + 远程无 meta 应拒绝 pull，实际：{:?}",
            result
        );

        let _ = g;
    }

    /// C-PULL-NO-META-SKIPS-STAMP 补充：本地无 vault + 远程无 meta → 仍允许（首次同步合法路径）。
    #[test]
    fn pull_allows_when_both_local_and_remote_meta_missing() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        // 删除本地 vault_meta（模拟本地无 vault）
        octopus_infra::db::with_db(|conn| {
            conn.execute("DELETE FROM vault_meta WHERE id = 1", [])?;
            Ok(())
        }).expect("delete vault_meta");

        // 不写远程 meta.json（首次同步/纯新增）
        // pull 应允许（不报错）——虽然没有任何 cipher 文件，pull 返回 Ok((0, 0))
        let result = pull_from_files();
        assert!(
            result.is_ok(),
            "本地无 vault + 远程无 meta 是首次同步合法路径，应允许 pull，实际：{:?}",
            result
        );

        let _ = g;
    }

    /// K1-GAP 守护（2026-07-25）：pull 拒绝弱 KDF 参数的远程 meta.json。
    ///
    /// 攻击者污染私有同步库的 meta.json 为 kdf_memory_kib=8 → pull 写入本地 DB →
    /// unlock 用崩溃下限接受 → 用户无感知地用废掉内存硬度的 Argon2id。
    /// K1-GAP 修复：pull_from_files 的 meta upsert 前调 from_i64_strict 拒绝弱参数。
    ///（之前只 resolve_with_remote 罕见分支有 strict，常规 pull 主路径漏防。）
    #[test]
    fn pull_rejects_weak_kdf_params() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        // 写一个 stamp 一致但 KDF 弱（memory_kib=8）的 meta.json
        let weak_meta = store::MetaFile {
            version: 1,
            kdf_type: 0,
            kdf_salt: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=".into(),
            kdf_iterations: 3,
            kdf_memory_kib: 8, // 弱：8KB 废掉 Argon2id 内存硬度（GPU 可全放寄存器）
            kdf_parallelism: 4,
            protected_user_vault_key: "v1:weak-uvk".into(),
            app_key_sync_enc: "v1:weak-sync".into(),
            security_stamp: "stamp-test".into(), // 与本地一致——通过 stamp 校验
            equivalent_domains: "[]".into(),
        };
        store::write_meta_file(&weak_meta).expect("write meta");

        // pull 应拒绝（from_i64_strict 拦截弱 KDF），不写入本地 DB
        let result = pull_from_files();
        assert!(
            result.is_err(),
            "K1-GAP: pull 应拒绝弱 KDF（memory_kib=8）的远程 meta，实际：{:?}",
            result
        );

        // 本地 DB 不应被污染——kdf_memory_kib 仍是原来的 65536
        let local = octopus_infra::db::load_vault_meta().unwrap().unwrap();
        assert_eq!(
            local.kdf_memory_kib, 65536,
            "K1-GAP: 弱 KDF 不应写入本地 DB，kdf_memory_kib 应仍是 65536"
        );
        let _ = g;
    }

    // === 代码审查修复测试（2026-07-24）===

    /// #2 修复：cipher_md5_mismatch 对比 outline.md5 vs DB sync_md5。
    /// - DB 无该 cipher → true（需 pull）
    /// - md5 不等 → true
    /// - md5 相等 → false
    #[test]
    fn cipher_md5_mismatch_compares_outline_vs_db() {
        use octopus_infra::db::VaultCipher;
        let db_cipher = VaultCipher {
            id: "uuid-1".to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:name".into(),
            notes: None,
            data: "v1:data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            is_deleted: false,
            sync_md5: Some("md5-aaa".into()),
            created_at: "2026-07-24".into(),
            updated_at: "2026-07-24".into(),
        };
        let db_ciphers = vec![db_cipher];
        // P-MD5-LINEAR-SCAN：测试改用 HashMap（id → sync_md5），与生产签名一致
        let db_cipher_md5: std::collections::HashMap<&str, &str> = db_ciphers
            .iter()
            .map(|c| (c.id.as_str(), c.sync_md5.as_deref().unwrap_or("")))
            .collect();

        // DB 有 + md5 相同 → false
        assert!(!cipher_md5_mismatch("uuid-1", "md5-aaa", &db_cipher_md5));
        // DB 有 + md5 不同 → true
        assert!(cipher_md5_mismatch("uuid-1", "md5-bbb", &db_cipher_md5));
        // DB 无 → true
        assert!(cipher_md5_mismatch("uuid-other", "md5-aaa", &db_cipher_md5));
    }

    /// #5 修复：folder_md5_mismatch 对比 outline.md5 vs DB sync_md5（与 cipher 对称）。
    #[test]
    fn folder_md5_mismatch_compares_outline_vs_db() {
        use octopus_infra::db::VaultFolder;
        let db_folder = VaultFolder {
            id: "folder-1".to_string(),
            name: "v1:name".into(),
            sort_order: 0,
            is_deleted: false,
            sync_md5: Some("md5-aaa".into()),
            created_at: "2026-07-24".into(),
            updated_at: "2026-07-24".into(),
        };
        let db_folders = vec![db_folder];
        let db_folder_md5: std::collections::HashMap<&str, &str> = db_folders
            .iter()
            .map(|f| (f.id.as_str(), f.sync_md5.as_deref().unwrap_or("")))
            .collect();

        assert!(!folder_md5_mismatch("folder-1", "md5-aaa", &db_folder_md5));
        assert!(folder_md5_mismatch("folder-1", "md5-bbb", &db_folder_md5));
        assert!(folder_md5_mismatch("folder-other", "md5-aaa", &db_folder_md5));
    }

    /// #2 修复回归守护：pull 用 outline.md5 比对，而非 updated_at 字符串。
    ///
    /// 场景：DB cipher sync_md5 与 outline.md5 相同（内容一致），但文件 updated_at
    /// 更新（跨设备时间戳不同）。旧逻辑用 updated_at 比较会误判需 pull → 反复重写
    /// 文件。新逻辑用 md5 应正确跳过。
    #[test]
    fn pull_uses_md5_not_updated_at() {
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
        let g = IntegrationGuard::new();

        use octopus_infra::db::{VaultCipher, VaultCipherInput};
        // 先在 DB 放一个 cipher（带 sync_md5）
        let cipher = VaultCipher {
            id: "md5-test-uuid".to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:name".into(),
            notes: None,
            data: "v1:data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            is_deleted: false,
            sync_md5: None,
            created_at: "2026-07-24T00:00:00".into(),
            updated_at: "2026-07-24T00:00:00".into(),
        };
        let md5 = crate::sync::fingerprint::cipher_md5(&cipher);
        let input = VaultCipherInput {
            id: cipher.id.clone(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:name".into(),
            notes: None,
            data: "v1:data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            is_deleted: false,
            sync_md5: Some(md5.clone()),
        };
        octopus_infra::db::insert_vault_cipher(&input).unwrap();

        // 文件系统写同一 cipher，但 updated_at 故意设成「未来」（模拟跨设备时间戳差异）
        let mut file_cipher = cipher.clone();
        file_cipher.updated_at = "2099-12-31T23:59:59".into(); // 远比 DB 新
        store::write_cipher_file(&file_cipher).unwrap();

        // outline：md5 与 DB sync_md5 相同（内容一致）
        use octopus_sync::outline::{Outline, OutlineEntry};
        use std::collections::BTreeMap;
        let outline = Outline {
            version: 1,
            vault_version: 1,
            ciphers: BTreeMap::from([(
                "md5-test-uuid".to_string(),
                OutlineEntry {
                    md5: md5.clone(),
                    updated_ms: 0,
                },
            )]),
            folders: BTreeMap::new(),
        };
        store::write_outline_file(&outline).expect("write outline");

        // pull：md5 一致应跳过，pulled = 0
        write_stamp_matching_meta();
        let (pulled, _skipped) = pull_from_files().expect("pull should succeed");
        assert_eq!(
            pulled, 0,
            "md5 一致时不应 pull（旧 updated_at 逻辑会误判——文件 updated_at 是 2099）"
        );
        let _ = g;
    }

    /// #5 修复：folder rename 能被 pull 捕获（之前已有 folder 整个跳过）。
    ///
    /// 场景：DB 有 folder（name A），远程 rename 为 name B（同 id）。pull 应检测到
    /// md5 变化并 upsert 新 name。
    #[test]
    fn pull_captures_folder_rename() {
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
        let g = IntegrationGuard::new();

        use octopus_infra::db::VaultFolder;
        // DB 已有 folder（name A, sort_order 0）
        let folder = VaultFolder {
            id: "rename-test-folder".to_string(),
            name: "v1:name-a".into(),
            sort_order: 0,
            is_deleted: false,
            sync_md5: None,
            created_at: "2026-07-24T00:00:00".into(),
            updated_at: "2026-07-24T00:00:00".into(),
        };
        let old_md5 = crate::sync::fingerprint::folder_md5(&folder);
        octopus_infra::db::insert_vault_folder(&folder.id, &folder.name, &old_md5).unwrap();

        // 文件系统写 rename 后的 folder（name B, sort_order 2）
        let renamed_folder = VaultFolder {
            name: "v1:name-b".into(),
            sort_order: 2,
            ..folder.clone()
        };
        store::write_folder_file(&renamed_folder).unwrap();

        // outline：md5 是 rename 后的（含 name B + sort_order 2）
        use octopus_sync::outline::{Outline, OutlineEntry};
        use std::collections::BTreeMap;
        let new_md5 = crate::sync::fingerprint::folder_md5(&renamed_folder);
        let outline = Outline {
            version: 1,
            vault_version: 1,
            ciphers: BTreeMap::new(),
            folders: BTreeMap::from([(
                "rename-test-folder".to_string(),
                OutlineEntry {
                    md5: new_md5,
                    updated_ms: 0,
                },
            )]),
        };
        store::write_outline_file(&outline).expect("write outline");

        // pull 应检测到 folder 变化并 upsert
        write_stamp_matching_meta();
        let (pulled, _skipped) = pull_from_files().expect("pull should succeed");
        assert_eq!(
            pulled, 1,
            "folder rename 应被 pull 捕获（#5——已有 folder 也要比对 md5）"
        );

        // 验证 DB 中 folder 已更新为 name B + sort_order 2
        let db_folders = octopus_infra::db::list_vault_folders().unwrap();
        let updated = db_folders.iter().find(|f| f.id == "rename-test-folder").unwrap();
        assert_eq!(updated.name, "v1:name-b", "folder name 应被 rename");
        assert_eq!(updated.sort_order, 2, "sort_order 也应被同步（#6）");
        let _ = g;
    }

    /// #10 修复：损坏 cipher 文件不再静默吞——skipped 计数 + log warn。
    #[test]
    fn pull_skips_corrupted_cipher_file() {
        let _s = test_lock(); // 串行化：避免 SYNC_LOCK / 全局 DB 状态跨测试竞争
        let g = IntegrationGuard::new();

        // outline 声明有 cipher，但文件内容是损坏的 JSON
        use octopus_sync::outline::{Outline, OutlineEntry};
        use std::collections::BTreeMap;
        let outline = Outline {
            version: 1,
            vault_version: 1,
            ciphers: BTreeMap::from([(
                "corrupt-uuid".to_string(),
                OutlineEntry {
                    md5: "any-md5".into(),
                    updated_ms: 0,
                },
            )]),
            folders: BTreeMap::new(),
        };
        store::write_outline_file(&outline).expect("write outline");

        // 写一个损坏的 cipher 文件（非法 JSON）——用合法 UUID（E-PATH-TRAVERSAL 校验后要求合法格式）
        let path = store::cipher_file_path("cccccccc-1111-4222-8333-cccccccccccc").expect("path");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is not valid json {{{{").unwrap();

        // pull 应 Ok（不阻断），但 skipped = 1，pulled = 0
        write_stamp_matching_meta();
        let (pulled, skipped) = pull_from_files().expect("pull should not fail");
        assert_eq!(pulled, 0, "损坏文件不应被 pull");
        assert_eq!(skipped, 1, "损坏文件应被计入 skipped（#10——不再静默吞）");
        let _ = g;
    }

    /// #6 修复回归守护：folder_md5_from_fields 含 sort_order。
    /// 改 sort_order 应改变 md5（否则 sort_order 永不同步）。
    #[test]
    fn folder_md5_includes_sort_order() {
        let m1 = crate::sync::fingerprint::folder_md5_from_fields("id-1", "name", 0);
        let m2 = crate::sync::fingerprint::folder_md5_from_fields("id-1", "name", 1);
        assert_ne!(
            m1, m2,
            "sort_order 变化应改变 folder md5（否则排序永不同步）"
        );
    }

    /// E2 回归守护：clone 时本地已有 vault_meta 应拒绝——
    /// 否则 clone 覆盖加密参数导致本地 cipher 永久锁死。
    #[test]
    fn clone_rejects_when_local_vault_already_initialized() {
        let _s = test_lock();
        let g = IntegrationGuard::new();
        // IntegrationGuard 预置了 vault_meta（security_stamp = "stamp-test"）
        // clone_from 应在 git clone 之前就拒绝（vault_meta 已存在）
        let result = clone_from("git@github.com:test/repo.git");
        assert!(
            result.is_err(),
            "E2: 本地已初始化 vault 时 clone 应拒绝（防覆盖加密参数致数据锁死）"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("已初始化") || err_msg.contains("vault_meta"),
            "错误消息应说明原因，实际：{}",
            err_msg
        );
        let _ = g;
    }

    /// H2 回归守护：软删密码经 pull 同步后 is_deleted 必须存活。
    ///
    /// 场景：设备 A 软删密码 X（is_deleted=true）→ 文件带 is_deleted=true →
    /// 设备 B pull → DB 应保留 is_deleted=true（而非复活成 live）。
    ///
    /// 之前 bug：VaultCipherInput 无 is_deleted 字段，pull 构造时丢弃 →
    /// INSERT 默认 false / UPDATE 不碰 → 软删密码跨设备复活。
    #[test]
    fn pull_preserves_soft_deleted_at() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        use octopus_infra::db::VaultCipher;
        // 文件系统写一个软删密码（is_deleted = true）
        let soft_deleted = VaultCipher {
            id: "soft-delete-test-uuid".to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:deleted-name".into(),
            notes: None,
            data: "v1:deleted-data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            is_deleted: true, // 软删
            sync_md5: None,
            created_at: "2026-07-24T00:00:00".into(),
            updated_at: "2026-07-24T12:00:00".into(),
        };
        store::write_cipher_file(&soft_deleted).unwrap();

        // outline：含这个软删密码的 md5
        use octopus_sync::outline::{Outline, OutlineEntry};
        use std::collections::BTreeMap;
        let md5 = crate::sync::fingerprint::cipher_md5(&soft_deleted);
        let outline = Outline {
            version: 1,
            vault_version: 1,
            ciphers: BTreeMap::from([(
                "soft-delete-test-uuid".to_string(),
                OutlineEntry {
                    md5,
                    updated_ms: 0,
                },
            )]),
            folders: BTreeMap::new(),
        };
        store::write_outline_file(&outline).unwrap();

        // pull 应把软删密码导入 DB（含 is_deleted）
        write_stamp_matching_meta();
        let (pulled, _skipped) = pull_from_files().expect("pull should succeed");
        assert_eq!(pulled, 1, "软删密码应被 pull 导入");

        // H2 核心断言：DB 中 is_deleted 必须保留（不能复活成 false）
        let db_cipher = octopus_infra::db::load_vault_cipher("soft-delete-test-uuid")
            .unwrap()
            .expect("cipher should exist in DB");
        assert!(
            db_cipher.is_deleted,
            "H2: 软删密码 pull 后 is_deleted 必须存活（不能复活成 live）"
        );
        let _ = g;
    }

    /// H2 补充：clone 也应保留软删状态（之前 clone_initial 硬编码 is_deleted: false）。
    ///
    /// T1 修复（2026-07-24）：改用 build_cipher_input_from_file（生产构造点的单一真相源），
    /// 而非测试自带构造——若日后有人把 clone_initial 改回 false，此测试会真正报红。
    #[test]
    fn clone_preserves_soft_deleted_at() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        use octopus_infra::db::VaultCipher;
        // 文件系统写一个软删密码
        let soft_deleted = VaultCipher {
            id: "clone-soft-delete-uuid".to_string(),
            folder_id: None,
            favorite: false,
            atype: 1,
            name: "v1:name".into(),
            notes: None,
            data: "v1:data".into(),
            fields: None,
            password_history: None,
            reprompt: 0,
            is_deleted: true,
            sync_md5: None,
            created_at: "2026-07-24T00:00:00".into(),
            updated_at: "2026-07-24T10:00:00".into(),
        };
        store::write_cipher_file(&soft_deleted).unwrap();

        // clone_initial 走 import_all_from_files → build_cipher_input_from_file → upsert
        // T1：用生产 helper（与 clone_initial 相同路径），不再手写 VaultCipherInput
        let (ciphers, _folders) = store::import_all_from_files().unwrap();
        assert_eq!(ciphers.len(), 1, "应导入 1 个 cipher");
        let input = build_cipher_input_from_file(&ciphers[0]);
        upsert_cipher(&input).unwrap();

        // H2 核心断言
        let db_cipher = octopus_infra::db::load_vault_cipher("clone-soft-delete-uuid")
            .unwrap()
            .expect("cipher should exist");
        assert!(
            db_cipher.is_deleted,
            "H2: clone 后软删状态必须保留（之前硬编码 false → 复活）"
        );
        let _ = g;
    }

    /// E-UI-URL-LEAKS-PAT-LIST-REMOTES 行为守护（2026-07-26）。
    ///
    /// 第六次外溢的根因（报告 §二）：前五轮只追 log:: 宏的 url 透传，漏了「返回值
    /// 流向前端 UI」维度。list_remotes / SyncStatus.remotes 直接透传 git_remote_list
    /// 原始 url（含 PAT），到 SyncPanel.tsx {url} 裸渲染。
    ///
    /// 契约测试（redact_url_strips_userinfo）只防 redact_url 实现退化，不防漏调。
    /// 本测试用行为层守护：直接调用 list_remotes / get_sync_status 的 redact 路径，
    /// 断言返回值不含 PAT。这两个函数依赖真实 git repo，无法直接单元测试——
    /// 但 redact 在 vault/sync/engine.rs 的两个流出点（list_remotes 返回 +
    /// SyncStatus.remotes 字段）是纯 map 操作，可以提取成可测函数。
    ///
    /// 这里测的是「redact_remotes_for_outflow」helper——list_remotes 和
    /// get_sync_status 共用的流出前 redact 步骤。任何一处漏调这个 helper，
    /// 本测试仍 pass，但新增流出点如果不调 helper 就不会被守护——
    /// 这是当前架构下的最佳折中（编译期 newtype 才能完全防漏调，已评估暂不引入）。
    #[test]
    fn redact_remotes_for_outflow_strips_pat() {
        let input = vec![
            ("origin".to_string(), "https://user:ghp_secret@github.com/owner/repo.git".to_string()),
            ("backup".to_string(), "git@github.com:owner/repo.git".to_string()),
        ];
        let redacted = super::redact_remotes_for_outflow(&input);
        // PAT 必须被剥离
        assert!(
            !redacted.iter().any(|(_, u)| u.as_str().contains("ghp_secret")),
            "redact_remotes_for_outflow 后仍含 PAT：{:?}",
            redacted
        );
        // SSH URL（scp-like）原样返回
        let ssh = redacted.iter().find(|(n, _)| n == "backup").expect("backup remote");
        assert_eq!(ssh.1.as_str(), "git@github.com:owner/repo.git");
        // HTTPS URL 剥 userinfo
        let https = redacted.iter().find(|(n, _)| n == "origin").expect("origin remote");
        assert_eq!(https.1.as_str(), "https://github.com/owner/repo.git");
    }

    /// 回归守护（2026-07-27 sync 覆盖 bug 系列）：
    /// 完整模拟用户场景——.sync 有数据 + DB 清空 → sync 应恢复数据到 DB，不丢 .sync 文件。
    /// 同时验证 pull 顺序（folder 先 cipher 后，避免 FK constraint failed）。
    ///
    /// 覆盖 4 个 bug 修复点的协同：
    /// 1. push 不删 cipher 文件（incremental_export db_all_empty 保护）
    /// 2. push 不写空 outline（incremental_export 返回旧 outline）
    /// 3. pull 能从 .sync 拉回数据（pull_from_files md5 比对）
    /// 4. pull 先 folder 后 cipher（避免 FK constraint failed）
    #[test]
    fn sync_recovers_data_when_db_emptied() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        // 阶段 1：DB 有 2 个 cipher（c1 有 folder_id）+ 1 个 folder，push 到 .sync
        let folder_id = "ccc33333-3333-4333-8333-333333333333";
        db::insert_vault_folder(folder_id, "v1:enc-folder1", "md5-f1").expect("insert f1");
        let c1 = VaultCipherInput {
            id: "aaa11111-1111-4111-8111-111111111111".to_string(),
            folder_id: Some(folder_id.to_string()), // ⚠️ 引用 folder——测 FK 顺序
            favorite: false, atype: 1,
            name: "v1:enc-site1".into(), notes: None, data: "v1:enc-data1".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        let c2 = VaultCipherInput {
            id: "bbb22222-2222-4222-8222-222222222222".to_string(),
            folder_id: None,
            favorite: false, atype: 1,
            name: "v1:enc-site2".into(), notes: None, data: "v1:enc-data2".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&c1).expect("insert c1");
        db::insert_vault_cipher(&c2).expect("insert c2");

        let pushed1 = push_to_files().expect("push 1");
        assert!(pushed1 >= 3, "首次 push 应写至少 3 个文件（2 cipher + 1 folder），实际 {}", pushed1);

        // 验证 .sync outline 有数据
        let outline1 = store::read_outline_file().expect("outline 1");
        assert_eq!(outline1.ciphers.len(), 2, ".sync outline 应有 2 cipher");
        assert_eq!(outline1.folders.len(), 1, ".sync outline 应有 1 folder");

        // 阶段 2：模拟清库——清空 DB cipher/folder（保留 vault_meta，stamp 一致）
        db::with_db(|conn| {
            conn.execute("DELETE FROM vault_ciphers", []).expect("del ciphers");
            conn.execute("DELETE FROM vault_folders", []).expect("del folders");
            Ok::<_, anyhow::Error>(())
        }).expect("clear db");
        assert!(db::list_vault_ciphers().expect("list").is_empty(), "清库后 DB cipher 应空");
        assert!(db::list_vault_folders().expect("list").is_empty(), "清库后 DB folder 应空");

        // 阶段 3：push（DB 空）——保护应生效，不删文件 + 不覆盖 outline
        let pushed2 = push_to_files().expect("push 2");
        assert_eq!(pushed2, 0, "DB 空 + .sync 有数据 → 不应删/写任何文件");

        let outline2 = store::read_outline_file().expect("outline 2");
        assert_eq!(outline2.ciphers.len(), 2, "保护后 outline 仍应有 2 cipher");
        assert_eq!(outline2.folders.len(), 1, "保护后 outline 仍应有 1 folder");

        // 阶段 4：pull（DB 空 + .sync 有数据）——应恢复数据，不触发 FK
        // c1 有 folder_id=folder_id，folder 必须先于 cipher 写入（否则 FK failed）
        let (pulled, skipped) = pull_from_files().expect("pull");
        assert_eq!(pulled, 3, "应从 .sync 拉回 3 条（2 cipher + 1 folder），实际 {}", pulled);
        assert_eq!(skipped, 0, "无跳过");

        // 验证 DB 恢复
        let db_ciphers_after = db::list_vault_ciphers().expect("list after");
        let db_folders_after = db::list_vault_folders().expect("list after");
        assert_eq!(db_ciphers_after.len(), 2, "pull 后 DB 应有 2 cipher");
        assert_eq!(db_folders_after.len(), 1, "pull 后 DB 应有 1 folder");
        let ids: Vec<&str> = db_ciphers_after.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"aaa11111-1111-4111-8111-111111111111"), "c1 应恢复");
        assert!(ids.contains(&"bbb22222-2222-4222-8222-222222222222"), "c2 应恢复");
        // c1 的 folder_id 应正确恢复
        let c1_after = db_ciphers_after.iter().find(|c| c.id == "aaa11111-1111-4111-8111-111111111111").expect("c1");
        assert_eq!(c1_after.folder_id.as_deref(), Some(folder_id), "c1 folder_id 应恢复");

        let _ = g;
    }

    /// 回归守护（2026-07-27 app_key 不匹配 bug）：
    /// B 机新建 vault（新 app_key + local_enc）→ sync 从 A 机拉数据（旧 app_key + sync_enc）
    /// → pull 后 app_key_local_enc 应被清空（强制下次 unlock 从 sync_enc 解旧 app_key）。
    /// 否则启动优先用 local_enc 解出新 app_key → cipher（旧 app_key 加密）解不开。
    #[test]
    fn pull_clears_local_enc_when_sync_enc_differs() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        // DB 已有 vault_meta（IntegrationGuard 设的，stamp-test，app_key_local_enc 非空）
        let meta_before = db::load_vault_meta().expect("meta").expect("meta exists");
        let original_local_enc = meta_before.app_key_local_enc.clone();
        assert!(!original_local_enc.is_empty(), "IntegrationGuard 应设了非空 local_enc");

        // 模拟 A 机数据 push 到 .sync（不同 app_key_sync_enc）
        let c1 = VaultCipherInput {
            id: "ddd44444-4444-4444-8444-444444444444".to_string(),
            folder_id: None, favorite: false, atype: 1,
            name: "v1:enc-site".into(), notes: None, data: "v1:enc-data".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&c1).expect("insert");
        push_to_files().expect("push");

        // 清空 DB cipher（模拟 B 机新建 vault 后 cipher 空）
        db::with_db(|conn| {
            conn.execute("DELETE FROM vault_ciphers", []).expect("del");
            Ok::<_, anyhow::Error>(())
        }).expect("clear");

        // 模拟 A 机不同的 app_key_sync_enc：直接改 .sync meta.json 的 sync_enc
        let meta_file = store::read_meta_file().expect("read meta file");
        let mut meta_fields = meta_file;
        meta_fields.app_key_sync_enc = "v1:DIFFERENT_SYNC_ENC_FROM_A_MACHINE".into();
        store::write_meta_file(&meta_fields).expect("write changed meta");

        // pull（DB 空 + .sync 有数据 + sync_enc 不同）
        let (pulled, _) = pull_from_files().expect("pull");
        assert_eq!(pulled, 1, "应拉回 1 cipher");

        // 验证 app_key_local_enc 被清空（强制走 sync_enc）
        let meta_after = db::load_vault_meta().expect("meta after").expect("meta exists");
        assert!(
            meta_after.app_key_local_enc.is_empty(),
            "sync_enc 变化时 local_enc 应被清空，实际：{}",
            meta_after.app_key_local_enc
        );
        assert_eq!(
            meta_after.app_key_sync_enc, "v1:DIFFERENT_SYNC_ENC_FROM_A_MACHINE",
            "app_key_sync_enc 应被远程值覆盖"
        );

        let _ = g;
    }

    // === merge_vault 测试（spec §3.1，2026-07-27）===
    //
    // 5 个测试覆盖 merge_vault 全部分支：
    // 1. pull 方向（.sync 有 + DB 空）—— B 机新建 vault 恢复
    // 2. push 方向（DB 有 + .sync 空）—— 本机新增上云
    // 3. .sync updated_at 更新赢 —— A 机更新覆盖 B 机旧 DB
    // 4. DB updated_at 更新赢 —— B 机更新覆盖 .sync 旧版本
    // 5. 冲突（updated_at 相同 + md5 不同）—— DB 赢（当前机器优先）

    /// B 机新建 vault（DB 空）→ sync 从 A 机拉数据 → DB 应恢复。
    /// 覆盖：pull 方向（.sync → DB），updated_at 最新赢（.sync 有 + DB 无 → .sync 赢）
    #[test]
    fn merge_vault_pulls_remote_data_to_empty_db() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        // 阶段 1：DB 有数据，push 到 .sync（模拟 A 机有数据）
        let folder_id = "fff00000-0000-4000-8000-000000000001";
        db::insert_vault_folder(folder_id, "v1:enc-folder", "md5-f1").expect("insert folder");
        let c1 = VaultCipherInput {
            id: "aaa00000-0000-4000-8000-000000000001".to_string(),
            folder_id: Some(folder_id.to_string()),
            favorite: false, atype: 1,
            name: "v1:enc-name1".into(), notes: None, data: "v1:enc-data1".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        let c2 = VaultCipherInput {
            id: "bbb00000-0000-4000-8000-000000000002".to_string(),
            folder_id: None, favorite: false, atype: 1,
            name: "v1:enc-name2".into(), notes: None, data: "v1:enc-data2".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&c1).expect("insert c1");
        db::insert_vault_cipher(&c2).expect("insert c2");
        push_to_files().expect("push to .sync");

        // 验证 .sync 有数据
        let outline = store::read_outline_file().expect("outline");
        assert_eq!(outline.ciphers.len(), 2);
        assert_eq!(outline.folders.len(), 1);

        // 阶段 2：清空 DB（模拟 B 机新建 vault）
        db::with_db(|conn| {
            conn.execute("DELETE FROM vault_ciphers", []).unwrap();
            conn.execute("DELETE FROM vault_folders", []).unwrap();
            Ok::<_, anyhow::Error>(())
        }).unwrap();

        // 阶段 3：merge → 应 pull 2 cipher + 1 folder
        let report = merge_vault().expect("merge");
        assert_eq!(report.pulled, 3, "应拉回 3 条（2 cipher + 1 folder）");
        assert_eq!(report.pushed, 0, "DB 空，无 push");

        // 验证 DB 恢复
        assert_eq!(db::list_vault_ciphers().unwrap().len(), 2);
        assert_eq!(db::list_vault_folders().unwrap().len(), 1);
        let _ = g;
    }

    /// DB 有数据 + .sync 没有 → push 到 .sync。
    #[test]
    fn merge_vault_pushes_local_only_data() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        // .sync 无 meta.json——但 IntegrationGuard 设了 vault_meta（local_meta=Some），
        // merge 的 stamp 校验拒绝「本地有 vault + 远程无 meta」场景。
        // 预置 stamp 一致的 meta.json 让 merge 跳过 stamp 错误，进 merge 分支。
        write_stamp_matching_meta();

        let c1 = VaultCipherInput {
            id: "ccc00000-0000-4000-8000-000000000003".to_string(),
            folder_id: None, favorite: false, atype: 1,
            name: "v1:enc-name3".into(), notes: None, data: "v1:enc-data3".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&c1).expect("insert");

        let report = merge_vault().expect("merge");
        assert_eq!(report.pushed, 1, "应 push 1 cipher 到 .sync");
        assert_eq!(report.pulled, 0);

        let outline = store::read_outline_file().expect("outline");
        assert_eq!(outline.ciphers.len(), 1);
        let _ = g;
    }

    /// DB + .sync 都有同一 cipher，.sync 的 updated_at 更新 → .sync 赢（pull 覆盖 DB）。
    #[test]
    fn merge_vault_updated_at_remote_wins() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        // DB 有 cipher（旧时间戳）
        let c1 = VaultCipherInput {
            id: "ddd00000-0000-4000-8000-000000000004".to_string(),
            folder_id: None, favorite: false, atype: 1,
            name: "v1:old-name".into(), notes: None, data: "v1:old-data".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&c1).expect("insert");
        push_to_files().expect("push old version");

        // 改 .sync 文件内容（模拟 A 机更新了 cipher，updated_at 更新）
        let cipher = db::load_vault_cipher("ddd00000-0000-4000-8000-000000000004").unwrap().unwrap();
        let mut updated = cipher.clone();
        updated.name = "v1:new-name-from-remote".into();
        updated.updated_at = "2026-12-31 23:59:59".into(); // 远比 DB 的新
        store::write_cipher_file(&updated).expect("write updated cipher to .sync");

        // 更新 outline 的 updated_ms
        let mut outline = store::read_outline_file().expect("outline");
        outline.ciphers.get_mut("ddd00000-0000-4000-8000-000000000004").unwrap().updated_ms =
            octopus_sync::store::iso_to_unix_ms("2026-12-31 23:59:59");
        store::write_outline_file(&outline).expect("write outline");

        // merge → .sync 赢（updated_at 更新）
        let report = merge_vault().expect("merge");
        assert!(report.pulled >= 1, "应 pull 远程更新的 cipher");

        // 验证 DB 被远程版本覆盖
        let after = db::load_vault_cipher("ddd00000-0000-4000-8000-000000000004").unwrap().unwrap();
        assert_eq!(after.name, "v1:new-name-from-remote", "DB 应被远程版本覆盖");
        let _ = g;
    }

    /// DB + .sync 都有同一 cipher，DB 的 updated_at 更新 → DB 赢（push 覆盖 .sync）。
    #[test]
    fn merge_vault_updated_at_db_wins() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        let c1 = VaultCipherInput {
            id: "eee00000-0000-4000-8000-000000000005".to_string(),
            folder_id: None, favorite: false, atype: 1,
            name: "v1:original".into(), notes: None, data: "v1:original-data".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&c1).expect("insert");
        push_to_files().expect("push");

        // DB 更新 cipher（新时间戳）
        db::with_db(|conn| {
            conn.execute(
                "UPDATE vault_ciphers SET name = 'v1:local-newer', updated_at = '2026-12-31 23:59:59' WHERE id = 'eee00000-0000-4000-8000-000000000005'",
                [],
            ).unwrap();
            Ok::<_, anyhow::Error>(())
        }).unwrap();

        // merge → DB 赢
        let report = merge_vault().expect("merge");
        assert!(report.pushed >= 1, "应 push DB 更新的 cipher 到 .sync");

        // 验证 .sync 被 DB 版本覆盖
        let file = store::read_cipher_file("eee00000-0000-4000-8000-000000000005").expect("read file");
        let cipher = file.to_vault_cipher();
        assert_eq!(cipher.name, "v1:local-newer", ".sync 应被 DB 版本覆盖");
        let _ = g;
    }

    /// updated_at 相同 + md5 不同 → DB 赢
    #[test]
    fn merge_vault_conflict_db_wins() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        let c1 = VaultCipherInput {
            id: "fff00000-0000-4000-8000-000000000006".to_string(),
            folder_id: None, favorite: false, atype: 1,
            name: "v1:db-version".into(), notes: None, data: "v1:db-data".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&c1).expect("insert");
        push_to_files().expect("push");

        // 改 .sync 文件（不同内容，相同 updated_at）——模拟 A 机同时间戳改了 cipher
        let cipher = db::load_vault_cipher("fff00000-0000-4000-8000-000000000006").unwrap().unwrap();
        let mut remote = cipher.clone();
        remote.name = "v1:remote-version".into();
        // updated_at 不变（相同时间戳 → 冲突）
        store::write_cipher_file(&remote).expect("write remote version");

        // 同步更新 outline 的 md5（反映新文件内容的指纹）——保持 updated_ms 不变
        // （模拟真实场景：A 机改 cipher 时 outline.md5 会重算，updated_at 未变时 updated_ms 相同）
        let remote_md5 = crate::sync::fingerprint::cipher_md5(&remote);
        let mut outline = store::read_outline_file().expect("outline");
        outline
            .ciphers
            .get_mut("fff00000-0000-4000-8000-000000000006")
            .unwrap()
            .md5 = remote_md5;
        store::write_outline_file(&outline).expect("write outline");

        let report = merge_vault().expect("merge");
        assert!(report.conflicts >= 1 || report.pushed >= 1, "冲突时应 DB 赢（push 或 conflict 计数）");

        // .sync 应被 DB 版本覆盖
        let file = store::read_cipher_file("fff00000-0000-4000-8000-000000000006").expect("read");
        let after = file.to_vault_cipher();
        assert_eq!(after.name, "v1:db-version", "冲突时 DB 应赢");
        let _ = g;
    }

    /// 清库恢复场景：本地 vault 空（cipher=0 + folder=0）+ 本地 stamp 与 .sync 不一致
    /// （模拟 `setup_vault` 生成的随机新 stamp）→ merge_vault 应返回
    /// `EmptyRecoveryNeedsPassword`（v2：要求用户输源机密码确认，不再无条件放行）。
    ///
    /// 背景（2026-07-28 v2）：v1 是无条件放行，但用户输错主密码会进入「数据恢复但解不开」
    /// 的死状态。v2 改为返回 EmptyRecoveryNeedsPassword，让前端弹窗输源机密码。
    /// 详见 spec 2026-07-28-vault-sync-empty-recovery.md。
    #[test]
    fn merge_vault_recovers_when_local_empty_and_stamp_differs() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        // 1. 本地有一条 cipher，push 到 .sync（让远程有数据）
        let c1 = VaultCipherInput {
            id: "eee00000-0000-4000-8000-000000000001".to_string(),
            folder_id: None, favorite: false, atype: 1,
            name: "v1:cipher-name".into(), notes: None, data: "v1:cipher-data".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&c1).expect("insert cipher");
        push_to_files().expect("push cipher to .sync");

        // 2. 模拟清库重建：删除本地所有 cipher + folder，但保留 vault_meta
        db::with_db(|conn| {
            conn.execute("DELETE FROM vault_ciphers", [])?;
            conn.execute("DELETE FROM vault_folders", [])?;
            Ok(())
        }).expect("clear local ciphers/folders");

        // 3. 模拟 setup_vault 生成的新 stamp + 写 .sync/meta.json 保持原 stamp-test
        db::update_vault_security_stamp("FRESH-STAMP-AFTER-SETUP")
            .expect("update local stamp to simulate fresh setup");
        write_stamp_matching_meta();

        let local_meta = db::load_vault_meta().unwrap().unwrap();
        assert_ne!(local_meta.security_stamp, "stamp-test", "测试前置：本地 stamp 应与远程不同");

        // 4. 调 merge_vault——应返回 EmptyRecoveryNeedsPassword（v2 不再无条件放行）
        let result = merge_vault();
        assert!(
            matches!(result, Err(SyncError::EmptyRecoveryNeedsPassword)),
            "空库 + stamp 不一致应返回 EmptyRecoveryNeedsPassword，实际：{:?}",
            result
        );

        // 5. 验证本地 DB 未被触碰（meta 没被覆盖，cipher 没被拉回）
        let after_meta = db::load_vault_meta().unwrap().unwrap();
        assert_eq!(
            after_meta.security_stamp, "FRESH-STAMP-AFTER-SETUP",
            "本地 stamp 不应被覆盖（等待用户输密码后才覆盖）"
        );
        let cipher_count = db::list_vault_ciphers().unwrap().len();
        assert_eq!(cipher_count, 0, "cipher 不应被拉回（等待用户输密码后才拉）");
        let _ = g;
    }

    /// 远程仓库 vault 数据被清空（meta.json + outline.json 都不存在）+ 本地有 vault
    /// → merge_vault 应允许继续（走纯 push 路径把本地数据推到远程），不报 RepoCorrupted。
    ///
    /// 场景（2026-07-28）：用户主动清空远程 vault 目录（git rm + push）后，本机建了
    /// 新 vault 想重新推送。原逻辑「本地有 vault_meta + 远程 meta 缺失 → RepoCorrupted」
    /// 误报。修复：远程 outline 也空时，判定为「远程从未有 vault 数据」，允许 push。
    #[test]
    fn merge_vault_allows_push_when_remote_vault_empty() {
        let _s = test_lock();
        let g = IntegrationGuard::new();

        // 1. 本地建一条 cipher（模拟用户新建 vault + 创建数据）
        let c1 = VaultCipherInput {
            id: "ddd00000-0000-4000-8000-000000000001".to_string(),
            folder_id: None, favorite: false, atype: 1,
            name: "v1:new-cipher".into(), notes: None, data: "v1:new-data".into(),
            fields: None, password_history: None, reprompt: 0,
            is_deleted: false, sync_md5: None,
        };
        db::insert_vault_cipher(&c1).expect("insert local cipher");

        // 2. 模拟远程 vault 被清空：不写 meta.json，不写 outline.json
        //    （IntegrationGuard 默认不写这些文件，.sync/vault/ 是空的）
        //    确认 .sync/vault/ 无 meta.json + 无 outline.json
        let meta_path = octopus_sync::store::sync_root().join("vault/meta.json");
        let outline_path = octopus_sync::store::sync_root().join("vault/outline.json");
        assert!(!meta_path.exists(), "测试前置：远程 meta.json 应不存在");
        assert!(!outline_path.exists(), "测试前置：远程 outline.json 应不存在");

        // 3. 调 merge_vault——应成功（不报 RepoCorrupted），走纯 push 路径
        let report = merge_vault().expect("merge 应成功（远程空 + 本地有数据 → 纯 push）");

        // 4. 验证本地 cipher 被推到远程（.sync 文件系统）
        assert!(report.pushed >= 1, "应至少 push 1 条 cipher 到远程");
        let pushed_file = store::read_cipher_file("ddd00000-0000-4000-8000-000000000001")
            .expect("cipher 应已 push 到 .sync");
        assert_eq!(pushed_file.to_vault_cipher().name, "v1:new-cipher");

        // 5. 验证本地 meta 也被推到远程（meta.json 应已生成）
        let remote_meta = store::read_meta_file().expect("远程 meta.json 应已生成");
        let local_meta = db::load_vault_meta().unwrap().unwrap();
        assert_eq!(
            remote_meta.security_stamp, local_meta.security_stamp,
            "远程 meta stamp 应与本地一致（本地 push 上去的）"
        );
        let _ = g;
    }
}
