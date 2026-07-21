//! Vault 同步错误类型。
//!
//! SyncError 用于把 git / 网络 / SSH 等外部错误分类，让前端能给出 user-friendly
//! 提示（如「请在终端跑 ssh -T git@github.com」而不是裸 stderr）。

use anyhow::Result;

/// 同步错误分类。
///
/// 不实现 `std::error::Error`——vault crate 统一用 anyhow，SyncError 主要用于
/// `Display` 给用户看的分类消息。命令层（desktop crate）会把它转成 Tauri String。
#[derive(Debug)]
pub enum SyncError {
    /// 系统 git 命令不可用（未安装 / PATH 找不到）。
    GitNotInstalled,
    /// 网络不可达（DNS 解析失败 / 连接超时）。
    NetworkUnreachable(String),
    /// SSH 权限拒绝（key 未配 / key 错误）。
    SshPermissionDenied(String),
    /// SSH host key 未验证（首次连 github.com 等）。
    SshHostKeyUnverified,
    /// HTTPS 凭据需求——remote 要求用户名/密码/PAT，但 octopus 禁用了交互 prompt。
    ///
    /// 典型 stderr：`could not read Username for 'https://...'` / `Authentication failed`
    /// / `Password authentication is not supported`。
    /// GitHub 自 2021-08 起禁用 HTTPS 密码认证——必须用 SSH key 或 PAT。
    CredentialsRequired(String),
    /// 远程仓库不存在 / URL 错误。
    RemoteNotFound(String),
    /// `~/.octopus/.vault/` 未初始化（git init 未跑）。
    RepoNotInitialized,
    /// git repo 状态损坏（非 git 仓库 / HEAD 丢失等）。
    RepoCorrupted(String),
    /// outline.json 解析失败。
    OutlineDamaged(String),
    /// 合并冲突需要用户手动介入（rebase 失败）。
    ConflictNeedsManual(String),
    /// security_stamp 不一致——远程 vault 用了不同主密码。
    MasterPasswordMismatch,
    /// 检测到公有库——禁止作为 vault 同步仓库（含 URL）。
    ///
    /// 私有库检测（2026-07-21）：AES-256-GCM 加密虽强，但密文泄露给攻击者做
    /// 离线爆破仍是失败（弱主密码会被破）。入口处必须拦截公有库。
    PublicRepoRejected(String),
    /// 本地路径不能作为同步 remote——`file://` / `/abs/path` / `./rel/path`。
    ///
    /// 同步意义为 0（本地路径无需 git remote），且会暴露本地文件结构。
    LocalPathRejected,
    /// 其他 git 错误（stderr 透传）。
    GitError(String),
    /// 其他 IO / 序列化错误。
    Other(anyhow::Error),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::GitNotInstalled => write!(f, "系统未安装 git 命令"),
            SyncError::NetworkUnreachable(msg) => write!(f, "网络不可达：{}", msg),
            SyncError::SshPermissionDenied(msg) => {
                write!(f, "SSH 权限拒绝（检查 SSH key 配置）：{}", msg)
            }
            SyncError::SshHostKeyUnverified => write!(
                f,
                "SSH host key 未验证——请在终端运行 `ssh -T git@github.com`（或对应 host）确认"
            ),
            SyncError::CredentialsRequired(_msg) => write!(
                f,
                "远程仓库需要认证但无法交互输入——请配 SSH key（推荐）或使用 SSH/PAT URL，避免 HTTPS 凭据"
            ),
            SyncError::RemoteNotFound(msg) => write!(f, "远程仓库不存在或 URL 错误：{}", msg),
            SyncError::RepoNotInitialized => write!(f, "同步仓库未初始化"),
            SyncError::RepoCorrupted(msg) => write!(f, "git 仓库状态损坏：{}", msg),
            SyncError::OutlineDamaged(msg) => write!(f, "outline.json 解析失败：{}", msg),
            SyncError::ConflictNeedsManual(msg) => {
                write!(f, "合并冲突需手动介入（请在终端打开 ~/.octopus/.vault/ 解决）：{}", msg)
            }
            SyncError::MasterPasswordMismatch => write!(f, "远程 vault 用了不同主密码"),
            SyncError::PublicRepoRejected(url) => write!(
                f,
                "拒绝添加公有库 {} 作为同步仓库——密码箱必须使用私有库。请到 GitHub/Gitee 把仓库改为 Private，或换一个私有库地址",
                url
            ),
            SyncError::LocalPathRejected => {
                write!(f, "本地路径不能作为同步 remote——请使用 GitHub/Gitee 私有库或自建 Git 服务的 URL")
            }
            SyncError::GitError(msg) => write!(f, "git 错误：{}", msg),
            SyncError::Other(e) => write!(f, "同步错误：{}", e),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<anyhow::Error> for SyncError {
    fn from(e: anyhow::Error) -> Self {
        SyncError::Other(e)
    }
}

// 注意：不实现 `From<SyncError> for anyhow::Error`——anyhow 已有 blanket impl
// `From<E> for anyhow::Error where E: std::error::Error`，再加会冲突（E0119）。
// SyncError 实现了 `std::error::Error` 后自动可用 `?` 转 anyhow::Error。

/// 从 git 命令的 stderr 分类 SyncError。
///
/// 常见 git/ssh 错误信息 → SyncError 变体映射。未匹配的归 `GitError(stderr)`。
pub fn classify_git_error(stderr: &str) -> SyncError {
    let lower = stderr.to_lowercase();
    if lower.contains("host key verification failed") || lower.contains("authenticity of host") {
        SyncError::SshHostKeyUnverified
    } else if lower.contains("terminal prompts disabled")
        || lower.contains("could not read username")
        || lower.contains("could not read password")
        || lower.contains("authentication failed")
        || lower.contains("password authentication is not supported")
        || lower.contains("invalid username or token")
    {
        // HTTPS 凭据需求——octopus 禁用了交互 prompt，git 无法读输入直接失败
        SyncError::CredentialsRequired(stderr.to_string())
    } else if lower.contains("permission denied (publickey)")
        || lower.contains("permission denied")
        || lower.contains("could not read from remote repository")
    {
        SyncError::SshPermissionDenied(stderr.to_string())
    } else if lower.contains("could not resolve host")
        || lower.contains("connection timed out")
        || lower.contains("network is unreachable")
    {
        SyncError::NetworkUnreachable(stderr.to_string())
    } else if lower.contains("repository not found") || lower.contains("not found") {
        SyncError::RemoteNotFound(stderr.to_string())
    } else if lower.contains("conflict") {
        SyncError::ConflictNeedsManual(stderr.to_string())
    } else {
        SyncError::GitError(stderr.to_string())
    }
}

/// 便利类型别名——sync 模块内部统一返 `Result<T, SyncError>`。
pub type SyncResult<T> = Result<T>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_host_key() {
        assert!(matches!(
            classify_git_error("Host key verification failed."),
            SyncError::SshHostKeyUnverified
        ));
    }

    #[test]
    fn classify_permission_denied() {
        let e = classify_git_error("Permission denied (publickey).");
        assert!(matches!(e, SyncError::SshPermissionDenied(_)));
    }

    #[test]
    fn classify_network() {
        let e = classify_git_error("Could not resolve host: github.com");
        assert!(matches!(e, SyncError::NetworkUnreachable(_)));
    }

    #[test]
    fn classify_repo_not_found() {
        let e = classify_git_error("repository not found");
        assert!(matches!(e, SyncError::RemoteNotFound(_)));
    }

    #[test]
    fn classify_credentials_required() {
        // GitHub HTTPS 失败的典型 stderr——octopus 禁用 prompt 后 git 立即失败
        let e = classify_git_error(
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
        );
        assert!(matches!(e, SyncError::CredentialsRequired(_)));

        // push 失败带完整错误链
        let e = classify_git_error(
            "remote: Invalid username or token. Password authentication is not supported for Git operations.\nfatal: Authentication failed for 'https://github.com/owner/repo/'",
        );
        assert!(matches!(e, SyncError::CredentialsRequired(_)));
    }

    #[test]
    fn classify_conflict() {
        let e = classify_git_error("CONFLICT (content): Merge conflict in outline.json");
        assert!(matches!(e, SyncError::ConflictNeedsManual(_)));
    }

    #[test]
    fn classify_other_git_error() {
        let e = classify_git_error("fatal: not a git repository");
        assert!(matches!(e, SyncError::GitError(_)));
    }

    #[test]
    fn display_messages_are_user_friendly() {
        assert_eq!(
            SyncError::GitNotInstalled.to_string(),
            "系统未安装 git 命令"
        );
        assert!(SyncError::SshHostKeyUnverified
            .to_string()
            .contains("ssh -T git@github.com"));
    }

    #[test]
    fn display_public_repo_rejected_includes_url() {
        let msg = SyncError::PublicRepoRejected("https://github.com/x/y.git".to_string()).to_string();
        assert!(msg.contains("https://github.com/x/y.git"));
        assert!(msg.contains("私有库"));
    }

    #[test]
    fn display_local_path_rejected_hint() {
        let msg = SyncError::LocalPathRejected.to_string();
        assert!(msg.contains("GitHub/Gitee"));
    }
}
