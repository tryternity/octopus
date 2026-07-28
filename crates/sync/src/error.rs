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
    /// 空库恢复场景：本地空库（cipher=0 + folder=0）+ 远程有数据 + stamp 不一致。
    /// 需要用户输源机器主密码确认（前端弹窗），调 resolve_with_remote 校验 + 覆盖本地。
    /// v2（2026-07-28）：v1 是无条件放行，但用户输错主密码会进入「数据恢复但解不开」
    /// 的死状态。v2 改为要求密码确认，复用现有 resolve_with_remote 函数。
    EmptyRecoveryNeedsPassword,
    /// 检测到公有库——禁止作为 vault 同步仓库（含 URL）。
    ///
    /// 私有库检测（2026-07-21）：AES-256-GCM 加密虽强，但密文泄露给攻击者做
    /// 离线爆破仍是失败（弱主密码会被破）。入口处必须拦截公有库。
    PublicRepoRejected(String),
    /// API 限流——无法确认仓库可见性，硬阻断防 public repo 漏检。
    /// S-SYNC-PUBLIC-LEAK-ON-RATELIMIT 修复（2026-07-27，第七十七轮）。
    RateLimited(String),
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
            // #11 修复（2026-07-24）：各变体的 msg 是原始 git stderr，可能含
            // 本地路径、或若 remote URL 配成 https://user:pat@host 则含 PAT。
            // Display 面向前端用户，不应透传 stderr——只给分类提示。
            // msg 保留在 enum 内，Debug / log::warn! 仍可见用于诊断。
            SyncError::NetworkUnreachable(_msg) => write!(
                f,
                "网络不可达——检查网络连接 / DNS / 防火墙（详情见应用日志）"
            ),
            SyncError::SshPermissionDenied(_msg) => {
                write!(f, "SSH 权限拒绝——检查 SSH key 配置（详情见应用日志）")
            }
            SyncError::SshHostKeyUnverified => write!(
                f,
                "SSH host key 未验证——请在终端运行 `ssh -T git@github.com`（或对应 host）确认"
            ),
            SyncError::CredentialsRequired(_msg) => write!(
                f,
                "远程仓库需要认证但无法交互输入——请配 SSH key（推荐）或使用 SSH/PAT URL，避免 HTTPS 凭据"
            ),
            SyncError::RemoteNotFound(_msg) => {
                write!(f, "远程仓库不存在或 URL 错误（详情见应用日志）")
            }
            SyncError::RepoNotInitialized => write!(f, "同步仓库未初始化"),
            SyncError::RepoCorrupted(msg) => write!(f, "git 仓库状态损坏：{}", msg),
            SyncError::OutlineDamaged(msg) => write!(f, "outline.json 解析失败：{}", msg),
            SyncError::ConflictNeedsManual(_msg) => {
                write!(f, "合并冲突需手动介入——请在终端打开 ~/.octopus/.sync/ 解决（详情见应用日志）")
            }
            SyncError::MasterPasswordMismatch => write!(f, "远程 vault 用了不同主密码"),
            // ⚠️ Display 字符串含「主密码」——前端 SyncPanel.tsx 用 includes("主密码")
            // 匹配以显示冲突解决 UI（密码输入框 + 以远程/本地为准按钮）。
            SyncError::EmptyRecoveryNeedsPassword => write!(f, "本地空库恢复需确认源机器主密码"),
            SyncError::PublicRepoRejected(url) => write!(
                f,
                "拒绝添加公有库 {} 作为同步仓库——密码箱必须使用私有库。请到 GitHub/Gitee 把仓库改为 Private，或换一个私有库地址",
                redact_url(url)
            ),
            SyncError::RateLimited(reason) => write!(
                f,
                "无法确认仓库可见性：{}。请稍后重试或换用 SSH URL（如 git@github.com:owner/repo.git）",
                reason
            ),
            SyncError::LocalPathRejected => {
                write!(f, "本地路径不能作为同步 remote——请使用 GitHub/Gitee 私有库或自建 Git 服务的 URL")
            }
            SyncError::GitError(_msg) => write!(f, "git 操作失败（详情见应用日志）"),
            SyncError::Other(e) => write!(f, "同步错误：{}", e),
        }
    }
}

/// E-PUBLIC-REPO-URL-LEAKS-PAT 修复（2026-07-26）：redact URL 中的 userinfo（PAT/密码）。
///
/// `https://user:ghp_xxx@github.com/owner/repo.git` → `https://github.com/owner/repo.git`
///
/// #11 修复守了 6 个 stderr 变体的 PAT 泄露，但漏了 `PublicRepoRejected(url)` 的 url 透传。
/// 本 helper 用于 Display + log::warn!，确保含 PAT 的 URL 不泄露到前端 toast / 日志。
///
/// 非 HTTP(S) URL（scp-like `git@host:owner/repo.git`）parse 失败时原样返回——
/// scp-like 格式不含 `://user:pass@`，不可能嵌 PAT。
///
/// SafeUrl newtype（2026-07-28 实施）：返 `SafeUrl` 而非 `String`——编译期保证所有
/// 流出 crate 边界的 url 都经 redact。SafeUrl 是 newtype，private 字段，外部无法直接
/// 构造（防绕过 redact_url）。详见 [safeurl-newtype-design](../../docs/superpowers/specs/2026-07-26-safeurl-newtype-design.md)。
pub fn redact_url(url: &str) -> SafeUrl {
    let redacted = match url::Url::parse(url) {
        Ok(mut parsed) => {
            let _ = parsed.set_password(None);
            let _ = parsed.set_username("");
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    };
    SafeUrl(redacted)
}

/// 已 redact 的 URL——PAT/密码/userinfo 已剥离，可安全用于 log / Display / 流出 crate 边界。
///
/// **唯一构造器是 `redact_url`**——无法从 `&str` / `String` 直接构造（不实现
/// `From<&str>` / `From<String>`），编译期保证所有 `SafeUrl` 实例都经过 redact。
///
/// 用于：流出 octopus-sync / octopus-vault crate 边界的 url（SyncStatus.remotes /
/// list_remotes 返回值 / SyncError::PublicRepoRejected / log 宏参数）。
///
/// 第六轮 PAT 外溢链（第四十九~五十四轮）的结构性根治——newtype 让漏调 = 编译错误，
/// 而非运行时 PAT 泄露。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SafeUrl(String);

impl SafeUrl {
    /// 已 redact 的字符串引用——用于 log / Display。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SafeUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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
    } else if lower.contains("repository not found") {
        // ER1 修复（2026-07-24）：删 || lower.contains("not found")——
        // 单独的 "not found" 过宽，误匹配 "object not found"（本地 repo 损坏）、
        // "path not found" 等非远程不存在的错误 → 前端误导用户查 remote 配置。
        // git 远程不存在的标准文案是 "repository '...' not found"，已被此处覆盖。
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

    /// S-SYNC-PUBLIC-LEAK-ON-RATELIMIT 守护（2026-07-27，第七十七轮）：
    /// RateLimited Display 应给用户可操作提示（重试 / 换 SSH），不暴露内部细节。
    #[test]
    fn display_rate_limited_actionable_hint() {
        let msg = SyncError::RateLimited("GitHub API 限流（60/h/IP）".to_string()).to_string();
        assert!(msg.contains("重试"), "应提示重试：{}", msg);
        assert!(msg.contains("SSH"), "应提示换 SSH URL：{}", msg);
    }

    /// #11 修复回归守护：Display 不透传含 PAT 的 git stderr。
    ///
    /// 场景：用户把 remote URL 配成 https://user:ghp_xxx@github.com/...（含 PAT），
    /// git 失败时 stderr 会包含这个 URL。Display 面向前端，必须过滤掉——
    /// PAT 泄露到前端 toast 等于凭证泄露。
    #[test]
    fn display_does_not_leak_pat_from_stderr() {
        let stderr_with_pat =
            "fatal: Authentication failed for 'https://user:ghp_abcdef123456@github.com/owner/repo/'";
        // E-PUBLIC-REPO-URL-LEAKS-PAT（第四十九轮）：补 PublicRepoRejected 变体——
        // #11 遗漏了 url 变体（只守了 6 个 stderr 变体），含 PAT 的 URL 经 Display 泄露。
        let url_with_pat = "https://user:ghp_abcdef123456@github.com/owner/repo.git";
        let variants = [
            SyncError::GitError(stderr_with_pat.to_string()),
            SyncError::CredentialsRequired(stderr_with_pat.to_string()),
            SyncError::SshPermissionDenied(stderr_with_pat.to_string()),
            SyncError::NetworkUnreachable(stderr_with_pat.to_string()),
            SyncError::RemoteNotFound(stderr_with_pat.to_string()),
            SyncError::ConflictNeedsManual(stderr_with_pat.to_string()),
            SyncError::PublicRepoRejected(url_with_pat.to_string()),
        ];
        for v in &variants {
            let msg = v.to_string();
            assert!(
                !msg.contains("ghp_abcdef123456"),
                "#11：Display 不应透传 PAT，实际：{}",
                msg
            );
        }
    }

    /// redact_url 契约守护——E-LOG-URL-LEAKS-PAT-INCOMPLETE 系列外溢（第四十九~五十二轮）
    /// 的根因防御。前四轮靠逐处 `let safe_url = redact_url(url)` 点状修复，反复外溢；
    /// 若 redact_url 本身漏了某种 PAT 格式，所有调用点的 safe_url 全部失效。
    ///
    /// 本测试钉死契约：常见 PAT/密码/userinfo 必须被剥离；非 HTTP(S) scp-like 原样返回。
    #[test]
    fn redact_url_strips_userinfo() {
        // PAT in userinfo（最常见——用户从 GitHub 复制 token 拼到 URL）
        assert_eq!(
            redact_url("https://user:ghp_xxx@github.com/owner/repo.git").as_str(),
            "https://github.com/owner/repo.git"
        );
        // 只有 token（无 username）
        assert_eq!(
            redact_url("https://:ghp_xxx@github.com/owner/repo.git").as_str(),
            "https://github.com/owner/repo.git"
        );
        // 只有 username（无 password）
        assert_eq!(
            redact_url("https://user@github.com/owner/repo.git").as_str(),
            "https://github.com/owner/repo.git"
        );
        // password 是密码而非 PAT
        assert_eq!(
            redact_url("https://alice:s3cret@gitee.com/owner/repo.git").as_str(),
            "https://gitee.com/owner/repo.git"
        );
        // 端口必须保留
        assert_eq!(
            redact_url("https://user:pass@gitlab.example.com:8443/team/repo.git").as_str(),
            "https://gitlab.example.com:8443/team/repo.git"
        );
        // query / fragment 保留
        assert_eq!(
            redact_url("https://user:token@github.com/owner/repo?foo=bar#frag").as_str(),
            "https://github.com/owner/repo?foo=bar#frag"
        );

        // scp-like SSH URL（非 `://` 格式）——url::Url parse 失败，原样返回
        // （scp-like 不含 `://user:pass@`，不可能嵌 PAT；不 redact 安全）
        assert_eq!(
            redact_url("git@github.com:owner/repo.git").as_str(),
            "git@github.com:owner/repo.git"
        );
        // ssh:// 协议——Url::parse 成功，应剥 userinfo
        assert_eq!(
            redact_url("ssh://git@github.com/owner/repo.git").as_str(),
            "ssh://github.com/owner/repo.git"
        );
    }

    /// redact_url 守护——PAT token 永不残留输出。
    ///
    /// 这是 E-LOG-URL-LEAKS-PAT-INCOMPLETE 系列的语义层防御：
    /// 无论输入什么 URL，输出绝不含 PAT token 字面量。
    #[test]
    fn redact_url_never_leaks_pat() {
        let pat_urls = [
            "https://user:ghp_abcdef1234567890@github.com/owner/repo.git",
            "https://user:github_pat_11ABCDEF_xxx@gitee.com/owner/repo.git",
            "https://user:glpat-xxxxxxxxxxxx@gitlab.example.com/team/repo.git",
            "https://user:ghp_abcdef1234567890@github.com/owner/repo.git?foo=bar",
        ];
        for u in &pat_urls {
            let redacted = redact_url(u);
            assert!(
                !redacted.as_str().contains("ghp_abcdef1234567890")
                    && !redacted.as_str().contains("github_pat_11ABCDEF_xxx")
                    && !redacted.as_str().contains("glpat-xxxxxxxxxxxx"),
                "redact_url 泄露 PAT：{} → {}",
                u,
                redacted
            );
        }
    }
}
