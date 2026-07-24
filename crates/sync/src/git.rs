//! git 命令 wrapper——shell out 系统 `git` 命令。
//!
//! 不嵌入 libgit2（git2-rs structs 非 Send/Sync，与 Tauri async 整合麻烦 +
//! Windows push 有已知问题）。要求用户机器装了 git（启动时检测，无 git 则同步
//! 功能禁用）。
//!
//! 所有命令在指定 path 下执行（`.current_dir(path)`）。stderr 透传给
//! `classify_git_error` 分类。

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::error::{classify_git_error, SyncError};

// === 辅助 ===

/// 构造一个非交互的 git Command（设禁用 prompt 的环境变量 + stdin /dev/null）。
///
/// **关键不变量**：octopus 在 Tauri 后端进程里跑 git，**stdin 脱离终端**——任何
/// 交互式凭据 prompt（用户名/密码）都会让进程卡死，UI 上看到的是"无限转圈"，
/// 用户根本无法输入。
///
/// 禁用 prompt 的三层防御：
/// 1. `GIT_TERMINAL_PROMPT=0`——git 遇到 HTTPS 凭据需求立即失败（不读 stdin）
/// 2. `GIT_ASKPASS=` + `SSH_ASKPASS=`——禁用外部 askpass 程序（macOS Keychain 等）
/// 3. stdin `/dev/null`——双保险，即使前两个被忽略 git 也立即读到 EOF 失败
///
/// 失败后由 classify_git_error 把"terminal prompts disabled"/"Authentication failed"
/// 等错误信息翻译为 SyncError，前端 toast 显示用户可读消息。
fn git_command(args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        // stdin → /dev/null：git 读不到任何输入立即失败，而非阻塞等 TTY
        .stdin(std::process::Stdio::null());
    cmd
}

/// 跑 git 命令——成功返 stdout，失败返 SyncError（按 stderr 分类）。
fn run_git(path: &Path, args: &[&str]) -> Result<String, SyncError> {
    let output = git_command(args)
        .current_dir(path)
        .output()
        .context("git 命令调用失败（git 未安装?）")
        .map_err(SyncError::Other)?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(classify_git_error(&stderr))
    }
}

/// 跑 git 命令但允许特定退出码——用于"nothing to commit"等非错误场景。
///
/// `success_codes` 是允许的退出码（除了标准的 0）。命中时返 Ok(stdout)。
fn run_git_allow_codes(
    path: &Path,
    args: &[&str],
    allow_exit_codes: &[i32],
    allow_stderr_contains: &[&str],
) -> Result<String, SyncError> {
    let output = git_command(args)
        .current_dir(path)
        .output()
        .context("git 命令调用失败")
        .map_err(SyncError::Other)?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // 检查 stderr 是否包含允许的关键词（如 "nothing to commit"）
    let lower = stderr.to_lowercase();
    for keyword in allow_stderr_contains {
        if lower.contains(keyword) {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
    }

    if allow_exit_codes.contains(&code) {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    Err(classify_git_error(&stderr))
}

// === T3.1: 检测 git 可用性 ===

/// 检测系统是否装了 git——`git --version` 成功即返 true。
pub fn check_git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// === T3.2: init / remote ===

/// `git init`——初始化 vault 目录为 git repo。
pub fn git_init(path: &Path) -> Result<(), SyncError> {
    run_git(path, &["init"])?;
    // 设默认分支为 main（避免 master/main 不一致）
    run_git(path, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
    Ok(())
}

/// `git remote add <name> <url>`——添加 remote。
pub fn git_remote_add(path: &Path, name: &str, url: &str) -> Result<(), SyncError> {
    run_git(path, &["remote", "add", name, url])?;
    Ok(())
}

/// `git remote remove <name>`——删除 remote。
pub fn git_remote_remove(path: &Path, name: &str) -> Result<(), SyncError> {
    run_git(path, &["remote", "remove", name])?;
    Ok(())
}

/// `git remote set-url <name> <url>`——改 remote 的 URL。
///
/// 用于 sync_now 兜底改写：当 .git/config 里的 remote URL 是 HTTPS（用户在自动
/// 改写功能加上之前 add 的，或 SSH key 后装的），sync_now 入口先 set-url 改成 SSH，
/// 避免 push 时卡在 GitHub HTTPS 用户名 prompt。
pub fn git_remote_set_url(path: &Path, name: &str, url: &str) -> Result<(), SyncError> {
    run_git(path, &["remote", "set-url", name, url])?;
    Ok(())
}

/// `git remote -v`——列出所有 remote（返 (name, url) pairs）。
pub fn git_remote_list(path: &Path) -> Result<Vec<(String, String)>, SyncError> {
    let output = run_git(path, &["remote", "-v"])?;
    let mut remotes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in output.lines() {
        // 格式: "origin\tgit@github.com:... (fetch)" / "origin\tgit@github.com:... (push)"
        if let Some((name, rest)) = line.split_once('\t') {
            if let Some((url, _)) = rest.split_once(' ') {
                let key = name.to_string();
                if seen.insert(key.clone()) {
                    remotes.push((key, url.to_string()));
                }
            }
        }
    }
    Ok(remotes)
}

// === T3.3: fetch / merge / rebase ===

/// `git fetch --all --prune`——拉所有 remote 的最新 refs。
pub fn git_fetch_all(path: &Path) -> Result<(), SyncError> {
    run_git(path, &["fetch", "--all", "--prune"])?;
    Ok(())
}

/// `git merge --ff-only <ref>` 的结果——区分 3 种情况让上层精确处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeFfResult {
    /// 成功 fast-forward（远程领先本地，已合并到本地）。
    FastForwarded,
    /// 不能 ff——远程与本地分叉，需 rebase 兜底。
    CannotFastForward,
    /// upstream 不存在——远程仓库是空的（首次推送场景）。
    /// 跳过 merge/rebase，直接 push -u 设 upstream。
    NoUpstream,
}

/// `git merge --ff-only <ref>`——fast-forward 合并。
///
/// 返 [`MergeFfResult`] 让上层区分 3 种情况：
/// - `FastForwarded`：远程领先，已合并到本地
/// - `CannotFastForward`：分叉，上层走 rebase
/// - `NoUpstream`：远程无此分支（首次推送场景），上层跳过 merge 直接 push -u
pub fn git_merge_ff(path: &Path, ref_name: &str) -> Result<MergeFfResult, SyncError> {
    let result = run_git_allow_codes(
        path,
        &["merge", "--ff-only", ref_name],
        &[],
        &[], // ff-only 失败时 stderr 含 "Not possible to fast-forward"
    );
    match result {
        Ok(_) => Ok(MergeFfResult::FastForwarded),
        Err(SyncError::GitError(stderr)) => {
            let lower = stderr.to_lowercase();
            // "not something we can merge" / "invalid upstream" / "not a valid ref"
            // 都表示远程没有该分支（首次推送场景）
            if lower.contains("not something we can merge")
                || lower.contains("invalid upstream")
                || lower.contains("not a valid ref")
                || lower.contains("unknown revision")
            {
                Ok(MergeFfResult::NoUpstream)
            } else {
                Ok(MergeFfResult::CannotFastForward)
            }
        }
        Err(e) => Err(e),
    }
}

/// `git rebase <ref>`——变基兜底。
pub fn git_rebase(path: &Path, ref_name: &str) -> Result<(), SyncError> {
    run_git(path, &["rebase", ref_name])?;
    Ok(())
}

// === T3.4: add / commit / push ===

/// `git add -A`——暂存所有变化。
pub fn git_add_all(path: &Path) -> Result<(), SyncError> {
    run_git(path, &["add", "-A"])?;
    Ok(())
}

/// `git commit -m <msg>`——提交。
///
/// 返 Ok(true) 成功 commit，Ok(false) nothing to commit（工作区干净）。
/// G2 修复（2026-07-24）：精确化错误处理——
/// 之前 allow_exit_codes: &[1] 无条件放行 exit 1（不只限于 nothing to commit），
/// + Err(GitError) => Ok(false) 兜底吞掉真实失败（index.lock/磁盘满/hook 拒绝）。
/// 现在直接处理 git commit 输出：成功（exit 0）→ true；无变化（stdout/stderr 含
/// "nothing to commit"，exit 1）→ false；其余失败 → Err。
pub fn git_commit(path: &Path, msg: &str) -> Result<bool, SyncError> {
    let output = git_command(&["commit", "-m", msg])
        .current_dir(path)
        .output()
        .context("git commit 调用失败")
        .map_err(SyncError::Other)?;

    if output.status.success() {
        return Ok(true);
    }

    // 非零退出——检查是否「无变化」（消息在 stdout 不在 stderr）
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!(
        "{} {}",
        stdout.to_lowercase(),
        String::from_utf8_lossy(&output.stderr).to_lowercase()
    );
    if combined.contains("nothing to commit") || combined.contains("no changes") {
        return Ok(false);
    }

    // 真实失败（hook 拒绝 / index.lock / 磁盘满）→ Err，不吞
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output.status.code().unwrap_or(-1);
    Err(classify_git_error(&format!(
        "git commit 失败（exit {}）：{}",
        code, stderr
    )))
}

/// `git push <remote> <ref>`——推送。
pub fn git_push(path: &Path, remote: &str, ref_name: &str) -> Result<(), SyncError> {
    run_git(path, &["push", remote, ref_name])?;
    Ok(())
}

/// `git push -u <remote> <ref>`——首次推送（设 upstream）。
pub fn git_push_set_upstream(
    path: &Path,
    remote: &str,
    ref_name: &str,
) -> Result<(), SyncError> {
    run_git(path, &["push", "-u", remote, ref_name])?;
    Ok(())
}

// === T3.5: clone ===

/// `git clone <url> <path>`——克隆远程仓库到指定路径。
///
/// 在 url 的父目录下执行（clone 会创建最后一层目录）。
pub fn git_clone(url: &str, path: &Path) -> Result<(), SyncError> {
    let parent = path
        .parent()
        .context("clone path 无父目录")
        .map_err(SyncError::Other)?;
    let dir_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("clone path 无文件名")
        .map_err(SyncError::Other)?;
    run_git(parent, &["clone", url, dir_name])?;
    Ok(())
}

// === T3.6: status / ls-remote ===

/// `git status --porcelain`——检测工作区有无变化。
pub fn git_status_has_changes(path: &Path) -> Result<bool, SyncError> {
    let output = run_git(path, &["status", "--porcelain"])?;
    Ok(!output.is_empty())
}

/// `git ls-remote --heads <url>`——测试连接（成功返 true）。
///
/// 不需要本地 repo——直接对远程 URL 操作。用于「测试连接」按钮。
///
/// **关键**：用 `git_command` 构造（禁用 prompt + stdin /dev/null）——
/// 避免私有 HTTPS 库的凭据 prompt 卡死 Tauri 后端进程（用户已踩坑）。
pub fn git_ls_remote(url: &str) -> Result<bool, SyncError> {
    let output = git_command(&["ls-remote", "--heads", url])
        .output()
        .context("git ls-remote 调用失败")
        .map_err(SyncError::Other)?;
    if output.status.success() {
        Ok(true)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(classify_git_error(&stderr))
    }
}

/// 验证本机 SSH key 能否认证指定 host（HTTPS → SSH 自动转换的前置检查）。
///
/// 跑 `ssh -T -o ConnectTimeout=5 -o StrictHostKeyChecking=accept-new git@<host>`：
/// - **GitHub**：返 exit 1 + stderr "Hi <user>! You've successfully authenticated..."
///   （GitHub 不允许 shell 访问，exit 1 但 stderr 是问候语）
/// - **Gitee**：返 exit 0 + 类似问候
/// - **失败**：exit 255 + "Permission denied (publickey)" / "Could not resolve hostname" 等
///
/// 返 `Ok(true)` = SSH key 可用；`Ok(false)` = 不可用（key 未配 / host 不通）；
/// `Err` = ssh 命令本身调不动（系统无 ssh）。
///
/// 关键：`-T` 禁用 TTY 分配（避免阻塞）；`StrictHostKeyChecking=accept-new`
/// 自动接受首次连接的 host key（用户不必先手动 ssh -T 一次）。
pub fn verify_ssh_key_for_host(host: &str) -> Result<bool, SyncError> {
    use std::process::Command;
    let output = Command::new("ssh")
        .args([
            "-T",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "BatchMode=yes", // 永不交互——避免卡密码 prompt
            format!("git@{}", host).as_str(),
        ])
        .output()
        .context("ssh 命令调用失败（系统未装 ssh?）")
        .map_err(SyncError::Other)?;

    // GitHub: exit 1 + stderr 含 "successfully authenticated"（不允许 shell，故 exit 非 0）
    // Gitee: exit 0
    // 失败: exit 255（Permission denied / host unreachable）
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let combined = format!("{} {}", stdout, stderr).to_lowercase();

    // 关键成功标志（覆盖 GitHub / Gitee 两种问候）
    let success = combined.contains("successfully authenticated")
        || combined.contains("welcome to gitee")
        || combined.contains("you've successfully authenticated");
    Ok(success)
}

/// `git ls-remote` 结果——区分成功（返 refs）、失败、超时。
///
/// 私有库检测（2026-07-21）用 exit 0 + 非空 refs 判定「公有」。
#[derive(Debug, Clone)]
pub struct LsRemoteResult {
    /// 成功列出 refs（公有库可访问）。
    pub success: bool,
    /// refs 数量（success 时 > 0）。
    pub refs_count: usize,
    /// 失败时的 stderr（用于分类 / 调试）。
    pub stderr: String,
}

/// `git ls-remote --heads <url>`——带超时（macOS 无 `timeout` 命令，必须代码层控制）。
///
/// 超时返回 `Ok(LsRemoteResult { success: false, stderr: "timeout" })`，不返 Err
/// （超时本质是网络问题，由上层 `Ambiguous` 兜底，不当作"硬失败"）。
///
/// **关键环境变量**：`GIT_TERMINAL_PROMPT=0`——私有 HTTPS 库会被 git 拦住要用户名，
/// 设 0 后立即失败而非卡死等输入（实测见 spec §5）。
///
/// 实现：`spawn` 起子进程 → 主线程轮询 `try_wait` → 超时后 `kill` 子进程。
/// 用 `spawn`+`kill`（而非 `mpsc`+`thread`）——超时后能真正终结 git 进程，不留僵尸。
pub fn git_ls_remote_with_timeout(url: &str, timeout_secs: u64) -> Result<LsRemoteResult, SyncError> {
    use std::time::{Duration, Instant};

    let mut child = git_command(&["ls-remote", "--heads", url])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("git ls-remote 调用失败（spawn）")
        .map_err(SyncError::Other)?;

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break false, // 已退出
            Ok(None) => {
                if Instant::now() >= deadline {
                    break true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(SyncError::Other(anyhow::anyhow!(
                    "git try_wait 失败: {}",
                    e
                )));
            }
        }
    };

    if timed_out {
        // 超时——kill 子进程避免僵尸；忽略 kill 本身的错误
        let _ = child.kill();
        let _ = child.wait();
        return Ok(LsRemoteResult {
            success: false,
            refs_count: 0,
            stderr: format!("git ls-remote 超时（{}s）", timeout_secs),
        });
    }

    let output = child
        .wait_with_output()
        .context("git ls-remote 读取输出失败")
        .map_err(SyncError::Other)?;
    let refs_count = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(LsRemoteResult {
        success: output.status.success(),
        refs_count,
        stderr,
    })
}

// === T3.7: 崩溃恢复 ===

/// `git merge --abort`——取消进行中的 merge（崩溃恢复）。
pub fn git_merge_abort(path: &Path) -> Result<(), SyncError> {
    // merge --abort 在无 merge 进行时返非 0 但无害——忽略错误
    let _ = run_git_allow_codes(path, &["merge", "--abort"], &[1, 2, 128], &[]);
    Ok(())
}

/// `git rebase --abort`——取消进行中的 rebase（崩溃恢复）。
pub fn git_rebase_abort(path: &Path) -> Result<(), SyncError> {
    let _ = run_git_allow_codes(path, &["rebase", "--abort"], &[1, 2, 128], &[]);
    Ok(())
}

/// 检测并清理进行中的 merge / rebase / stale index.lock——sync_now 入口调用。
///
/// - `.git/MERGE_HEAD` 存在 → merge 进行中 → abort
/// - `.git/rebase-merge` 或 `.git/rebase-apply` 存在 → rebase 进行中 → abort
/// - `.git/index.lock` 存在且 mtime > 60s 前 → 崩溃残留 → 删除（G3 修复）
///   （mtime 阈值区分「崩溃残留」vs「并发 git 进程持有」——SYNC_LOCK 保证单进程同步
///   串行，但用户手动 git 与 octopus 并发仍可能。60s 阈值足够区分。）
pub fn cleanup_in_progress_ops(path: &Path) -> Result<(), SyncError> {
    let git_dir = path.join(".git");
    if git_dir.join("MERGE_HEAD").exists() {
        log::warn!("检测到进行中的 merge，执行 abort 清理");
        git_merge_abort(path)?;
    }
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        log::warn!("检测到进行中的 rebase，执行 abort 清理");
        git_rebase_abort(path)?;
    }
    // G3 修复（2026-07-24）：清理 stale index.lock（崩溃残留）
    let index_lock = git_dir.join("index.lock");
    if index_lock.exists() {
        if let Ok(metadata) = std::fs::metadata(&index_lock) {
            if let Ok(mtime) = metadata.modified() {
                if mtime.elapsed().unwrap_or_default().as_secs() > 60 {
                    log::warn!(
                        "检测到 stale index.lock（mtime > 60s，崩溃残留），删除清理"
                    );
                    let _ = std::fs::remove_file(&index_lock);
                }
            }
        }
    }
    Ok(())
}

// === T3.8: 分支操作 ===

/// `git rev-parse --abbrev-ref HEAD`——取当前分支名。
pub fn git_current_branch(path: &Path) -> Result<String, SyncError> {
    let branch = run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok(branch)
}

/// `git checkout <branch>`——切换分支。
pub fn git_checkout(path: &Path, branch: &str) -> Result<(), SyncError> {
    run_git(path, &["checkout", branch])?;
    Ok(())
}

// === T3.9: 辅助查询 ===

/// 检测 path 是否是 git repo（`.git` 存在）。
pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

/// `git rev-parse HEAD`——取当前 HEAD commit SHA（用于调试 / 状态显示）。
pub fn git_head_sha(path: &Path) -> Result<String, SyncError> {
    let sha = run_git(path, &["rev-parse", "HEAD"])?;
    Ok(sha)
}

/// `git log --oneline -1`——取最近一条 commit（用于状态显示「上次同步时间」）。
pub fn git_last_commit_info(path: &Path) -> Result<Option<(String, String)>, SyncError> {
    let result = run_git_allow_codes(
        path,
        &["log", "--oneline", "-1", "--format=%H|%cI"],
        &[128],
        &["does not have any commits"],
    );
    match result {
        Ok(output) => {
            if output.is_empty() {
                return Ok(None);
            }
            // 格式: "<sha>|<iso-timestamp>"
            if let Some((sha, ts)) = output.split_once('|') {
                Ok(Some((sha.to_string(), ts.to_string())))
            } else {
                Ok(None)
            }
        }
        Err(SyncError::GitError(_)) => Ok(None), // 无 commit
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 检测测试环境是否有 git——CI 可能没装。
    fn has_git() -> bool {
        check_git_available()
    }

    /// 创建临时 git repo（init + 设 main 分支）。
    fn init_repo() -> TempDir {
        let tmp = TempDir::new().expect("tempdir");
        git_init(tmp.path()).expect("git init");
        // 配 user.email / user.name（commit 需要）
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(tmp.path())
            .output()
            .expect("config email");
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .expect("config name");
        tmp
    }

    #[test]
    fn check_git_available_works() {
        // 这个测试总是跑——git --version 在 CI 和本地都应有
        let _ = check_git_available();
    }

    #[test]
    fn git_init_creates_repo() {
        if !has_git() {
            return;
        }
        let tmp = TempDir::new().expect("tempdir");
        git_init(tmp.path()).expect("init");
        assert!(is_git_repo(tmp.path()));
    }

    #[test]
    fn git_add_commit_workflow() {
        if !has_git() {
            return;
        }
        let tmp = init_repo();
        let path = tmp.path();

        // 写文件
        std::fs::write(path.join("test.txt"), "hello").unwrap();
        git_add_all(path).expect("add");
        let committed = git_commit(path, "init").expect("commit");
        assert!(committed, "应成功 commit");

        // 再 commit 无变化
        let committed2 = git_commit(path, "noop").expect("commit noop");
        assert!(!committed2, "无变化时应返 false");
    }

    /// `git_merge_ff` 检测空 upstream——首次推送场景。
    /// 模拟：本地 commit 后 merge --ff-only origin/main（remote 不存在 main）。
    #[test]
    fn git_merge_ff_returns_no_upstream_when_branch_missing() {
        if !has_git() {
            return;
        }
        let tmp = init_repo();
        let path = tmp.path();

        // 本地 commit
        std::fs::write(path.join("f"), "x").unwrap();
        git_add_all(path).unwrap();
        git_commit(path, "init").unwrap();

        // merge --ff-only origin/main（remote 没配过，更没有 main 分支）
        let result = git_merge_ff(path, "origin/main").expect("merge_ff 不应 Err");
        assert_eq!(
            result, MergeFfResult::NoUpstream,
            "远程无 main 分支应识别为 NoUpstream（首次推送场景），实际：{:?}",
            result
        );
    }

    #[test]
    fn git_status_detects_changes() {
        if !has_git() {
            return;
        }
        let tmp = init_repo();
        let path = tmp.path();

        // 初始无变化
        std::fs::write(path.join("a.txt"), "a").unwrap();
        git_add_all(path).expect("add");
        git_commit(path, "first").expect("commit");
        assert!(!git_status_has_changes(path).unwrap(), "commit 后应无变化");

        // 改文件 → 有变化
        std::fs::write(path.join("a.txt"), "b").unwrap();
        assert!(git_status_has_changes(path).unwrap(), "改文件后应有变化");
    }

    #[test]
    fn git_remote_add_and_list() {
        if !has_git() {
            return;
        }
        let tmp = init_repo();
        let path = tmp.path();

        git_remote_add(path, "origin", "git@github.com:user/repo.git").expect("add remote");
        let remotes = git_remote_list(path).expect("list");
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].0, "origin");
        assert_eq!(remotes[0].1, "git@github.com:user/repo.git");
    }

    /// `git remote set-url` 把 HTTPS remote 改成 SSH（sync_now 兜底逻辑的基础）。
    #[test]
    fn git_remote_set_url_updates_url() {
        if !has_git() {
            return;
        }
        let tmp = init_repo();
        let path = tmp.path();

        git_remote_add(path, "origin", "https://github.com/user/repo.git").expect("add remote");
        git_remote_set_url(path, "origin", "git@github.com:user/repo.git").expect("set-url");

        let remotes = git_remote_list(path).expect("list");
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].1, "git@github.com:user/repo.git", "set-url 后应是新 URL");
    }

    #[test]
    fn git_current_branch_after_init() {
        if !has_git() {
            return;
        }
        let tmp = init_repo();
        // init 后还没 commit，分支可能还没创建——先 commit
        std::fs::write(tmp.path().join("f"), "x").unwrap();
        git_add_all(tmp.path()).unwrap();
        git_commit(tmp.path(), "init").unwrap();
        let branch = git_current_branch(tmp.path()).expect("branch");
        assert_eq!(branch, "main");
    }

    #[test]
    fn git_last_commit_info_empty_repo() {
        if !has_git() {
            return;
        }
        let tmp = init_repo();
        // 无 commit → None
        let info = git_last_commit_info(tmp.path()).expect("info");
        assert!(info.is_none());
    }

    #[test]
    fn git_last_commit_info_after_commit() {
        if !has_git() {
            return;
        }
        let tmp = init_repo();
        std::fs::write(tmp.path().join("f"), "x").unwrap();
        git_add_all(tmp.path()).unwrap();
        git_commit(tmp.path(), "init").unwrap();
        let info = git_last_commit_info(tmp.path()).expect("info");
        assert!(info.is_some(), "commit 后应有 info");
        let (sha, ts) = info.unwrap();
        assert!(!sha.is_empty());
        assert!(!ts.is_empty());
    }

    #[test]
    fn cleanup_in_progress_ops_clean_repo_is_noop() {
        if !has_git() {
            return;
        }
        let tmp = init_repo();
        // 干净 repo 调 cleanup 不应 Err
        cleanup_in_progress_ops(tmp.path()).expect("cleanup should be noop");
    }

    #[test]
    fn is_git_repo_detects_correctly() {
        if !has_git() {
            return;
        }
        let tmp = TempDir::new().expect("tempdir");
        assert!(!is_git_repo(tmp.path()), "非 git 目录");
        git_init(tmp.path()).unwrap();
        assert!(is_git_repo(tmp.path()), "init 后应是 git repo");
    }
}
