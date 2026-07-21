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

use crate::sync::error::{classify_git_error, SyncError};

// === 辅助 ===

/// 跑 git 命令——成功返 stdout，失败返 SyncError（按 stderr 分类）。
fn run_git(path: &Path, args: &[&str]) -> Result<String, SyncError> {
    let output = Command::new("git")
        .args(args)
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
    let output = Command::new("git")
        .args(args)
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

/// `git merge --ff-only <ref>`——fast-forward 合并。
///
/// 返 Ok(true) 成功 ff，Ok(false) 不能 ff（需 rebase）。
pub fn git_merge_ff(path: &Path, ref_name: &str) -> Result<bool, SyncError> {
    let result = run_git_allow_codes(
        path,
        &["merge", "--ff-only", ref_name],
        &[],
        &[], // ff-only 失败时 stderr 含 "Not possible to fast-forward"
    );
    match result {
        Ok(_) => Ok(true),
        Err(SyncError::GitError(_)) => Ok(false), // 不能 ff——非致命，让上层走 rebase
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
pub fn git_commit(path: &Path, msg: &str) -> Result<bool, SyncError> {
    let result = run_git_allow_codes(
        path,
        &["commit", "-m", msg],
        &[1],
        &["nothing to commit", "no changes added"],
    );
    match result {
        Ok(stdout) => {
            // 检查 stdout/stderr 是否提示 nothing to commit
            if stdout.to_lowercase().contains("nothing to commit") {
                Ok(false)
            } else {
                Ok(true)
            }
        }
        Err(SyncError::GitError(_)) => Ok(false),
        Err(e) => Err(e),
    }
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
pub fn git_ls_remote(url: &str) -> Result<bool, SyncError> {
    // ls-remote 不需要 current_dir，但 Command 要求一个工作目录——用 /tmp 兜底
    let output = Command::new("git")
        .args(["ls-remote", "--heads", url])
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

/// 检测并清理进行中的 merge / rebase——sync_now 入口调用。
///
/// - `.git/MERGE_HEAD` 存在 → merge 进行中 → abort
/// - `.git/rebase-merge` 或 `.git/rebase-apply` 存在 → rebase 进行中 → abort
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
