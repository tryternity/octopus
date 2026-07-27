//! 私有库检测——拒绝把公有库作为 vault 同步仓库（2026-07-21）。
//!
//! ## 背景
//!
//! vault 同步用 AES-256-GCM 加密，理论上密文推到公网也安全。但密文泄露给攻击者
//! 做离线爆破仍是失败——主密码弱时 KDF（Argon2id）也挡不住算力攻击。所以入口
//! 处必须拦截公有库。
//!
//! ## 策略（Phase 1：未认证 API）
//!
//! | URL 类型 | 检测方法 | 判定 |
//! |---|---|---|
//! | `file://` / 本地路径 | 直接拒绝 | 暴露本地路径无意义 |
//! | `github.com` / `gitee.com` HTTPS | HTTP API 查 `private` | 200 + private:false → Public；其他 → Ambiguous |
//! | 其他 host HTTPS | `git ls-remote --heads` | exit 0 + refs → Public；其他 → Ambiguous |
//! | SSH (`git@host:...`) | 无法匿名嗅探 | SshUnverifiable（允许 + 警告） |
//!
//! **关键不变量**：检测到公有必拒；歧义/私有/网络错误一律放行（不阻断用户），
//! 因为不会泄露给公众。SSH 路径完全无法检测，UI 显式提示用户自检。
//!
//! ## GitHub/Gitee 404 歧义
//!
//! 未认证查询私有库返 404（与"不存在"无法区分，是有意设计避免信息泄漏）。
//! 所以 Phase 1 只能"确认公有"，不能"确认私有"。Phase 2 加 PAT 后能区分。

use crate::error::SyncError;
use crate::git::{git_ls_remote_with_timeout, LsRemoteResult};
use std::time::Duration;

// === 判定结果 ===

/// 私有库检测结果。
#[derive(Debug, Clone)]
pub enum PrivacyVerdict {
    /// 确认公有库——**必须拒绝**。
    Public,
    /// 确认私有库（Phase 1 不会返——未认证 API 无法确认私有）。
    Private,
    /// 歧义——可能是私有 / 不存在。**放行 + UI 提示**。
    ///
    /// 注意：限流（403）不再归入 Ambiguous——见 [`PrivacyVerdict::RateLimited`]。
    Ambiguous(String),
    /// API 限流（403）——**硬阻断**（S-SYNC-PUBLIC-LEAK-ON-RATELIMIT 修复，2026-07-27）。
    ///
    /// 限流时无法确认仓库可见性——若放行，用户误配的 public repo 会被 push 密文
    /// 导致不可逆泄漏。限流是临时的（用户重试即可恢复），用「不可逆密钥泄漏」换取
    /// 「用户少等几分钟」是错误的代价权衡。硬阻断让用户重试或换用 SSH URL。
    RateLimited(String),
    /// SSH URL——无法自动检测。**放行 + UI 强提示**。
    SshUnverifiable,
    /// 网络错误——可能是 host 不通 / DNS 失败。**放行（用户可重试）**。
    NetworkError(String),
}

// === URL 解析 ===

/// git remote URL 协议分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitScheme {
    Https,
    Ssh,
    /// `file://` / 本地路径——**禁止作为同步 remote**。
    File,
}

/// 解析后的 git remote URL。
#[derive(Debug, Clone)]
pub struct GitRemoteUrl {
    pub scheme: GitScheme,
    pub host: String,
    /// `owner/repo`——仅 github.com / gitee.com 等平台型 host 有意义，自建可能为 None。
    pub owner_repo: Option<(String, String)>,
    pub raw: String,
}

/// SCP-like SSH URL 正则：`git@github.com:owner/repo.git`
///
/// 用 `regex` 而非 `url` crate——`url` 不认 scp-like 语法（无 `ssh://` 前缀）。
const SSH_SCP_RE: &str = r"^([^@]+)@([^:]+):(.+?)(?:\.git)?/?$";

impl GitRemoteUrl {
    /// 解析 git remote URL 字符串。
    ///
    /// 支持 5 种形式：
    /// - `https://github.com/owner/repo.git`（含 `https://user:token@...`）
    /// - `http://...`（按 HTTPS 处理，HTTP 极少用于现代 git 同步）
    /// - `git@github.com:owner/repo.git`（scp-like）
    /// - `ssh://git@github.com/owner/repo.git`
    /// - `file:///path` / `/abs/path` / `./rel/path` / `rel/path`（→ File）
    pub fn parse(url: &str) -> Result<Self, SyncError> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err(SyncError::Other(anyhow::anyhow!("remote URL 为空")));
        }

        // 显式 file:// 或本地路径
        if trimmed.starts_with("file://") || is_local_path(trimmed) {
            return Ok(GitRemoteUrl {
                scheme: GitScheme::File,
                host: String::new(),
                owner_repo: None,
                raw: trimmed.to_string(),
            });
        }

        // 显式 ssh://
        if trimmed.starts_with("ssh://") {
            // ssh://[user@]host[:port]/path
            let parsed = url::Url::parse(trimmed).map_err(|e| {
                SyncError::Other(anyhow::anyhow!("ssh:// URL 解析失败: {}", e))
            })?;
            let host = parsed.host_str().unwrap_or("").to_string();
            let owner_repo = parse_owner_repo_from_path(parsed.path());
            return Ok(GitRemoteUrl {
                scheme: GitScheme::Ssh,
                host,
                owner_repo,
                raw: trimmed.to_string(),
            });
        }

        // scp-like: git@host:path（最常见 SSH 形式）
        if let Some(caps) = regex::Regex::new(SSH_SCP_RE)
            .expect("ssh scp regex")
            .captures(trimmed)
        {
            let host = caps.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
            let path = caps.get(3).map(|m| m.as_str().to_string()).unwrap_or_default();
            let owner_repo = parse_owner_repo_from_path(&format!("/{}", path));
            return Ok(GitRemoteUrl {
                scheme: GitScheme::Ssh,
                host,
                owner_repo,
                raw: trimmed.to_string(),
            });
        }

        // http(s)://
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            let parsed = url::Url::parse(trimmed).map_err(|e| {
                SyncError::Other(anyhow::anyhow!("HTTP(S) URL 解析失败: {}", e))
            })?;
            let host = parsed.host_str().unwrap_or("").to_string();
            let owner_repo = parse_owner_repo_from_path(parsed.path());
            return Ok(GitRemoteUrl {
                scheme: GitScheme::Https,
                host,
                owner_repo,
                raw: trimmed.to_string(),
            });
        }

        // 其他形式（如 git://）——视为非法
        Err(SyncError::Other(anyhow::anyhow!(
            "无法识别的 git URL 格式: {}",
            trimmed
        )))
    }
}

/// 判断字符串是否是本地路径（非 URL）。
///
/// - `/abs/path`：以 `/` 开头
/// - `./rel/path` 或 `../rel/path`：以 `.` 开头
/// - `~/path`：home 目录
/// - `rel/path`：相对路径（含 `/`，且第二段是目录名——保守判断，避免误吞 hostname）
fn is_local_path(s: &str) -> bool {
    if s.starts_with('/') || s.starts_with("./") || s.starts_with("../") || s.starts_with("~/") {
        return true;
    }
    // 不以已知 scheme 开头，但含 `/` 且看起来像路径（无 host、无 port）——保守按 URL 处理
    // 不识别裸 `rel/path` 避免误吞 `github.com/owner/repo`
    false
}

/// 从 URL path 解析 `owner/repo`，去掉 trailing `/` 和 `.git` 后缀。
///
/// `/owner/repo.git/` → Some(("owner", "repo"))
/// `/owner/repo` → Some(("owner", "repo"))
/// `/a/b/c/repo` → Some(("a/b/c", "repo"))（不限制层级，但仅前 2 段被使用）
/// `/` 或空 → None
///
/// 顺序：先 strip trailing `/`，再 strip `.git`（避免 `repo.git/` 时 `.git` 不在末尾）。
fn parse_owner_repo_from_path(path: &str) -> Option<(String, String)> {
    let path = path.trim_start_matches('/');
    let path = path.trim_end_matches('/');
    let path = path.trim_end_matches(".git");
    if path.is_empty() {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return None;
    }
    // 取最后两段——前缀（如 enterprise 子分组）由调用方处理
    let repo = parts.last().unwrap().to_string();
    let owner = parts[parts.len() - 2].to_string();
    Some((owner, repo))
}

// === 检测引擎 ===

/// ls-remote 嗅探超时（秒）。
///
/// macOS 无 `timeout` 命令，超时由代码层控制。10s 足够覆盖正常 HTTPS 握手 +
/// refs 传输；慢网络/不通 host 必须及时终止避免 UI 卡死。
const LS_REMOTE_TIMEOUT_SECS: u64 = 10;

/// HTTP API 超时（秒）——GitHub/Gitee API 通常 < 1s 响应，10s 留余量。
const HTTP_TIMEOUT_SECS: u64 = 10;

/// 主入口——根据 URL 类型分流检测私有性。
///
/// 返 `Ok(Public)` 时调用方必须返 `Err(SyncError::PublicRepoRejected)`。
/// 返其他 verdict 时调用方记录日志、放行。
pub fn check_privacy(url: &str) -> Result<PrivacyVerdict, SyncError> {
    let parsed = GitRemoteUrl::parse(url)?;

    match parsed.scheme {
        GitScheme::File => Err(SyncError::LocalPathRejected),
        GitScheme::Ssh => Ok(PrivacyVerdict::SshUnverifiable),
        GitScheme::Https => check_https(&parsed),
    }
}

/// HTTPS URL 的私有性检测——按 host 分流。
fn check_https(parsed: &GitRemoteUrl) -> Result<PrivacyVerdict, SyncError> {
    let host = parsed.host.as_str();
    let (owner, repo) = match &parsed.owner_repo {
        Some(or) => or.clone(),
        None => {
            // 路径无法解析出 owner/repo——fallback 到 ls-remote
            return check_via_ls_remote(&parsed.raw);
        }
    };

    match host {
        "github.com" => check_via_github_api(&owner, &repo),
        "gitee.com" => check_via_gitee_api(&owner, &repo),
        _ => check_via_ls_remote(&parsed.raw),
    }
}

/// GitHub API 查询——`GET https://api.github.com/repos/{owner}/{repo}`。
///
/// 返回策略：
/// - 200 + `private: false` → `Public`
/// - 200 + `private: true` → `Private`（Phase 1 未认证不会到这）
/// - 404 → `Ambiguous`（私有 vs 不存在无法区分）
/// - 403 → `RateLimited`（任意 403 一并硬阻断——未认证查询的 403 实践中即 API 限流；
///   即使成因非限流（如 abuse detection 触发的 403）也阻断，方向更保守。S-SYNC-PUBLIC-LEAK-ON-RATELIMIT）
/// - 网络错误 → `NetworkError`
/// - 其他状态码 → fallback ls-remote
fn check_via_github_api(owner: &str, repo: &str) -> Result<PrivacyVerdict, SyncError> {
    let api_url = format!("https://api.github.com/repos/{}/{}", owner, repo);
    let resp = http_get_json(&api_url, HTTP_TIMEOUT_SECS);

    match resp {
        HttpResult::Ok(json) => {
            let private = json
                .get("private")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if private {
                Ok(PrivacyVerdict::Private)
            } else {
                Ok(PrivacyVerdict::Public)
            }
        }
        HttpResult::Status(404) => Ok(PrivacyVerdict::Ambiguous(
            "GitHub 返 404（可能是私有库，也可能不存在）".to_string(),
        )),
        HttpResult::Status(403) => Ok(PrivacyVerdict::RateLimited(
            "GitHub API 限流（60/h/IP）——请稍后重试或换用 SSH URL".to_string(),
        )),
        HttpResult::Status(code) => {
            log::warn!("[sync] GitHub API 返 {} —— fallback 到 ls-remote", code);
            check_via_ls_remote(&format!(
                "https://github.com/{}/{}.git",
                owner, repo
            ))
        }
        HttpResult::NetworkError(msg) => Ok(PrivacyVerdict::NetworkError(msg)),
    }
}

/// Gitee API 查询——`GET https://gitee.com/api/v5/repos/{owner}/{repo}`。
///
/// Gitee 三态：public / private / internal（企业内部库）。
/// `private || internal` → Private；`public == true` → Public。
fn check_via_gitee_api(owner: &str, repo: &str) -> Result<PrivacyVerdict, SyncError> {
    let api_url = format!("https://gitee.com/api/v5/repos/{}/{}", owner, repo);
    let resp = http_get_json(&api_url, HTTP_TIMEOUT_SECS);

    match resp {
        HttpResult::Ok(json) => {
            let private = json
                .get("private")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let internal = json
                .get("internal")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if private || internal {
                Ok(PrivacyVerdict::Private)
            } else {
                Ok(PrivacyVerdict::Public)
            }
        }
        HttpResult::Status(404) => Ok(PrivacyVerdict::Ambiguous(
            "Gitee 返 404（可能是私有库，也可能不存在）".to_string(),
        )),
        HttpResult::Status(403) => Ok(PrivacyVerdict::RateLimited(
            "Gitee API 限流——请稍后重试或换用 SSH URL".to_string(),
        )),
        HttpResult::Status(code) => {
            log::warn!("[sync] Gitee API 返 {} —— fallback 到 ls-remote", code);
            check_via_ls_remote(&format!("https://gitee.com/{}/{}.git", owner, repo))
        }
        HttpResult::NetworkError(msg) => Ok(PrivacyVerdict::NetworkError(msg)),
    }
}

/// `git ls-remote --heads <url>` 嗅探。
///
/// - exit 0 + refs > 0 → `Public`（公有库可匿名读取）
/// - exit 128 + `terminal prompts disabled` → `Ambiguous`（私有 HTTPS 或不存在）
/// - exit 128 + `Could not resolve host` → `NetworkError`
/// - 超时 → `NetworkError`
/// - 其他失败 → `Ambiguous`（保守归歧义，不阻断用户）
fn check_via_ls_remote(url: &str) -> Result<PrivacyVerdict, SyncError> {
    let result = git_ls_remote_with_timeout(url, LS_REMOTE_TIMEOUT_SECS)?;
    interpret_ls_remote(result)
}

/// 把 `LsRemoteResult` 解读为 `PrivacyVerdict`——抽出来便于测试。
fn interpret_ls_remote(result: LsRemoteResult) -> Result<PrivacyVerdict, SyncError> {
    if result.success && result.refs_count > 0 {
        return Ok(PrivacyVerdict::Public);
    }
    let stderr_lower = result.stderr.to_lowercase();
    if stderr_lower.contains("could not resolve host")
        || stderr_lower.contains("timed out")
        || result.stderr.contains("超时")
    {
        return Ok(PrivacyVerdict::NetworkError(result.stderr));
    }
    // terminal prompts disabled = HTTPS 私有/404；Repository not found = SSH 不存在
    // 两者都归歧义
    Ok(PrivacyVerdict::Ambiguous(result.stderr))
}

// === HTTP 客户端 ===

/// HTTP GET 结果。
enum HttpResult {
    /// 200 + JSON body。
    Ok(serde_json::Value),
    /// 非 200 状态码。
    Status(u16),
    /// 网络错误（连接失败 / DNS 失败 / 超时）。
    NetworkError(String),
}

/// 简单 HTTP GET JSON——用 ureq（同步）。
///
/// 设 User-Agent——GitHub API 强制要求。
fn http_get_json(url: &str, timeout_secs: u64) -> HttpResult {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(timeout_secs))
        .build();
    let resp = agent
        .get(url)
        .set("User-Agent", "octopus-vault-sync")
        .set("Accept", "application/vnd.github+json")
        .call();
    match resp {
        Ok(r) => match r.into_json::<serde_json::Value>() {
            Ok(v) => HttpResult::Ok(v),
            Err(e) => {
                log::warn!("[sync] HTTP body 解析失败: {}", e);
                HttpResult::Status(0) // 视为非 2xx
            }
        },
        Err(ureq::Error::Status(code, _)) => HttpResult::Status(code),
        Err(e) => HttpResult::NetworkError(format!("{}", e)),
    }
}

// === HTTPS → SSH 自动转换（2026-07-21 增补） ===

/// 支持 HTTPS → SSH 自动转换的 host 白名单。
///
/// 仅这些 host 的 HTTPS URL 会被尝试转 SSH——它们是主流托管平台，
/// 双协议一定可用、SSH 端口（22）必然开放。自建 GitLab/Gitea 不在列
/// （SSH 端口可能被封 / 改端口 / 未启用）。
const HTTPS_TO_SSH_HOSTS: &[&str] = &["github.com", "gitee.com"];

/// 把 github.com / gitee.com 的 HTTPS URL 转成 SSH URL（scp-like）。
///
/// 转换规则（owner/repo 从 path 最后两段提取）：
/// - `https://github.com/owner/repo` → `git@github.com:owner/repo.git`
/// - `https://github.com/owner/repo.git` → `git@github.com:owner/repo.git`
/// - `https://user:token@github.com/owner/repo` → `git@github.com:owner/repo.git`（丢 userinfo）
/// - `https://gitee.com/owner/repo` → `git@gitee.com:owner/repo.git`
///
/// 返回 `None` 的情况：
/// - URL 不是 HTTPS 协议
/// - host 不在白名单（github.com/gitee.com）
/// - 解析不出 owner/repo（路径太短）
///
/// 设计动机：GitHub 自 2021-08 起禁用 HTTPS 密码认证，仅支持 PAT。
/// 用户从浏览器复制的 URL 默认是 HTTPS——octopus 自动转 SSH 用 `~/.ssh/` 私钥，
/// 避免「HTTPS 不支持密码认证」的死局（用户已踩坑）。
pub fn try_convert_https_to_ssh(url: &str) -> Option<String> {
    let parsed = GitRemoteUrl::parse(url).ok()?;
    if parsed.scheme != GitScheme::Https {
        return None;
    }
    if !HTTPS_TO_SSH_HOSTS.contains(&parsed.host.as_str()) {
        return None;
    }
    let (owner, repo) = parsed.owner_repo?;
    Some(format!("git@{}:{}/{}.git", parsed.host, owner, repo))
}

#[cfg(test)]
mod tests {
    use super::*;

    // === GitRemoteUrl::parse ===

    #[test]
    fn parse_https_simple() {
        let u = GitRemoteUrl::parse("https://github.com/owner/repo.git").unwrap();
        assert_eq!(u.scheme, GitScheme::Https);
        assert_eq!(u.host, "github.com");
        assert_eq!(u.owner_repo, Some(("owner".to_string(), "repo".to_string())));
    }

    #[test]
    fn parse_https_with_userinfo() {
        // 凭据型 URL——必须不崩
        let u = GitRemoteUrl::parse("https://user:token@github.com/owner/repo.git").unwrap();
        assert_eq!(u.scheme, GitScheme::Https);
        assert_eq!(u.host, "github.com");
        assert_eq!(u.owner_repo, Some(("owner".to_string(), "repo".to_string())));
    }

    #[test]
    fn parse_https_no_git_suffix() {
        let u = GitRemoteUrl::parse("https://github.com/owner/repo").unwrap();
        assert_eq!(u.owner_repo, Some(("owner".to_string(), "repo".to_string())));
    }

    #[test]
    fn parse_https_trailing_slash() {
        let u = GitRemoteUrl::parse("https://github.com/owner/repo.git/").unwrap();
        assert_eq!(u.owner_repo, Some(("owner".to_string(), "repo".to_string())));
    }

    #[test]
    fn parse_ssh_scp_like() {
        let u = GitRemoteUrl::parse("git@github.com:owner/repo.git").unwrap();
        assert_eq!(u.scheme, GitScheme::Ssh);
        assert_eq!(u.host, "github.com");
        assert_eq!(u.owner_repo, Some(("owner".to_string(), "repo".to_string())));
    }

    #[test]
    fn parse_ssh_explicit() {
        let u = GitRemoteUrl::parse("ssh://git@github.com/owner/repo.git").unwrap();
        assert_eq!(u.scheme, GitScheme::Ssh);
        assert_eq!(u.host, "github.com");
        assert_eq!(u.owner_repo, Some(("owner".to_string(), "repo".to_string())));
    }

    #[test]
    fn parse_ssh_custom_port() {
        let u = GitRemoteUrl::parse("ssh://git@gitee.com:2222/owner/repo.git").unwrap();
        assert_eq!(u.scheme, GitScheme::Ssh);
        assert_eq!(u.host, "gitee.com");
    }

    #[test]
    fn parse_file_scheme() {
        let u = GitRemoteUrl::parse("file:///path/to/repo").unwrap();
        assert_eq!(u.scheme, GitScheme::File);
    }

    #[test]
    fn parse_abs_local_path() {
        let u = GitRemoteUrl::parse("/Users/me/repo").unwrap();
        assert_eq!(u.scheme, GitScheme::File);
    }

    #[test]
    fn parse_relative_local_path() {
        let u = GitRemoteUrl::parse("./repo").unwrap();
        assert_eq!(u.scheme, GitScheme::File);
        let u = GitRemoteUrl::parse("../parent/repo").unwrap();
        assert_eq!(u.scheme, GitScheme::File);
        let u = GitRemoteUrl::parse("~/repos/vault").unwrap();
        assert_eq!(u.scheme, GitScheme::File);
    }

    #[test]
    fn parse_empty_url_rejected() {
        assert!(GitRemoteUrl::parse("").is_err());
        assert!(GitRemoteUrl::parse("   ").is_err());
    }

    #[test]
    fn parse_unknown_scheme_rejected() {
        // git:// 协议极少用，不识别
        assert!(GitRemoteUrl::parse("git://github.com/owner/repo.git").is_err());
    }

    #[test]
    fn parse_self_hosted_github() {
        let u = GitRemoteUrl::parse("https://github.mycompany.com/team/vault.git").unwrap();
        assert_eq!(u.host, "github.mycompany.com");
        assert_eq!(u.owner_repo, Some(("team".to_string(), "vault".to_string())));
    }

    // === owner/repo 路径解析 ===

    #[test]
    fn owner_repo_from_simple_path() {
        let r = parse_owner_repo_from_path("/owner/repo.git").unwrap();
        assert_eq!(r, ("owner".to_string(), "repo".to_string()));
    }

    #[test]
    fn owner_repo_from_nested_path() {
        // 企业 Gitea 子分组——取最后两段
        let r = parse_owner_repo_from_path("/group/subgroup/repo").unwrap();
        assert_eq!(r, ("subgroup".to_string(), "repo".to_string()));
    }

    #[test]
    fn owner_repo_from_empty_returns_none() {
        assert!(parse_owner_repo_from_path("").is_none());
        assert!(parse_owner_repo_from_path("/").is_none());
        assert!(parse_owner_repo_from_path("/onlyone").is_none());
    }

    // === interpret_ls_remote ===

    #[test]
    fn ls_remote_success_with_refs_is_public() {
        let r = LsRemoteResult {
            success: true,
            refs_count: 3,
            stderr: String::new(),
        };
        let v = interpret_ls_remote(r).unwrap();
        assert!(matches!(v, PrivacyVerdict::Public));
    }

    #[test]
    fn ls_remote_success_zero_refs_is_ambiguous() {
        // 空 repo（刚创建无 commit）也是合法私有库场景
        let r = LsRemoteResult {
            success: true,
            refs_count: 0,
            stderr: String::new(),
        };
        let v = interpret_ls_remote(r).unwrap();
        assert!(matches!(v, PrivacyVerdict::Ambiguous(_)));
    }

    #[test]
    fn ls_remote_terminal_prompts_is_ambiguous() {
        let r = LsRemoteResult {
            success: false,
            refs_count: 0,
            stderr: "fatal: could not read Username for 'https://github.com': terminal prompts disabled".to_string(),
        };
        let v = interpret_ls_remote(r).unwrap();
        assert!(matches!(v, PrivacyVerdict::Ambiguous(_)));
    }

    #[test]
    fn ls_remote_dns_failure_is_network_error() {
        let r = LsRemoteResult {
            success: false,
            refs_count: 0,
            stderr: "fatal: unable to access ...: Could not resolve host: foo.invalid".to_string(),
        };
        let v = interpret_ls_remote(r).unwrap();
        assert!(matches!(v, PrivacyVerdict::NetworkError(_)));
    }

    #[test]
    fn ls_remote_timeout_is_network_error() {
        let r = LsRemoteResult {
            success: false,
            refs_count: 0,
            stderr: "git ls-remote 超时（10s）".to_string(),
        };
        let v = interpret_ls_remote(r).unwrap();
        assert!(matches!(v, PrivacyVerdict::NetworkError(_)));
    }

    // === check_privacy 分流（mock 不打真实网络） ===

    #[test]
    fn check_privacy_file_rejected() {
        let e = check_privacy("file:///path/to/repo").unwrap_err();
        assert!(matches!(e, SyncError::LocalPathRejected));
    }

    #[test]
    fn check_privacy_abs_path_rejected() {
        let e = check_privacy("/Users/me/repo").unwrap_err();
        assert!(matches!(e, SyncError::LocalPathRejected));
    }

    #[test]
    fn check_privacy_ssh_is_unverifiable() {
        let v = check_privacy("git@github.com:owner/repo.git").unwrap();
        assert!(matches!(v, PrivacyVerdict::SshUnverifiable));
    }

    #[test]
    fn check_privacy_ssh_explicit_is_unverifiable() {
        let v = check_privacy("ssh://git@github.com/owner/repo.git").unwrap();
        assert!(matches!(v, PrivacyVerdict::SshUnverifiable));
    }

    // === 真实网络集成测试（CI 不稳定，标 #[ignore]） ===

    #[test]
    #[ignore = "真实 GitHub API——CI/无网环境不稳，手动跑"]
    fn integration_github_public_repo_detected() {
        // octocat/Hello-World 是 GitHub 官方公开示例
        let v = check_privacy("https://github.com/octocat/Hello-World.git").unwrap();
        assert!(matches!(v, PrivacyVerdict::Public), "公有库应被检测出: {:?}", v);
    }

    #[test]
    #[ignore = "真实 Gitee API"]
    fn integration_gitee_public_repo_detected() {
        // mirrors/kubernetes 是 Gitee 公开镜像
        let v = check_privacy("https://gitee.com/mirrors/kubernetes.git").unwrap();
        assert!(matches!(v, PrivacyVerdict::Public), "公有库应被检测出: {:?}", v);
    }

    #[test]
    #[ignore = "真实 GitHub 不存在库 → Ambiguous（404 歧义）"]
    fn integration_github_nonexistent_is_ambiguous() {
        let v = check_privacy("https://github.com/octocat/this-does-not-exist-xyz.git").unwrap();
        assert!(
            matches!(v, PrivacyVerdict::Ambiguous(_)),
            "不存在/私有库应是 Ambiguous: {:?}",
            v
        );
    }

    // === try_convert_https_to_ssh（2026-07-21 增补） ===

    #[test]
    fn convert_github_https_to_ssh() {
        assert_eq!(
            try_convert_https_to_ssh("https://github.com/owner/repo.git").as_deref(),
            Some("git@github.com:owner/repo.git"),
        );
        // 无 .git 后缀也要能转
        assert_eq!(
            try_convert_https_to_ssh("https://github.com/owner/repo").as_deref(),
            Some("git@github.com:owner/repo.git"),
        );
    }

    #[test]
    fn convert_gitee_https_to_ssh() {
        assert_eq!(
            try_convert_https_to_ssh("https://gitee.com/owner/repo.git").as_deref(),
            Some("git@gitee.com:owner/repo.git"),
        );
    }

    #[test]
    fn convert_https_with_userinfo_strips_credentials() {
        // 用户在 URL 内嵌 token 的写法——转 SSH 后必须丢 userinfo
        assert_eq!(
            try_convert_https_to_ssh("https://user:token@github.com/owner/repo.git").as_deref(),
            Some("git@github.com:owner/repo.git"),
        );
    }

    #[test]
    fn convert_returns_none_for_ssh_input() {
        // 已经是 SSH URL——不应再转
        assert_eq!(try_convert_https_to_ssh("git@github.com:owner/repo.git"), None);
        assert_eq!(
            try_convert_https_to_ssh("ssh://git@github.com/owner/repo.git"),
            None
        );
    }

    #[test]
    fn convert_returns_none_for_self_hosted() {
        // 自建 GitLab/Gitea 不在白名单
        assert_eq!(
            try_convert_https_to_ssh("https://github.mycompany.com/owner/repo.git"),
            None,
            "自建 GitHub Enterprise 不应自动转——SSH 端口可能不通"
        );
        assert_eq!(
            try_convert_https_to_ssh("https://gitlab.com/owner/repo.git"),
            None,
            "gitlab.com 不在白名单（避免误改用户特意配的 HTTPS）"
        );
    }

    #[test]
    fn convert_returns_none_for_file_url() {
        assert_eq!(try_convert_https_to_ssh("file:///path/to/repo"), None);
        assert_eq!(try_convert_https_to_ssh("/abs/path"), None);
    }

    #[test]
    fn convert_returns_none_for_empty_or_invalid() {
        assert_eq!(try_convert_https_to_ssh(""), None);
        // path 只有一段，没有 owner/repo
        assert_eq!(try_convert_https_to_ssh("https://github.com/onlyone"), None);
    }

    #[test]
    #[ignore = "真实 ssh -T GitHub——需联网且本机已配 SSH key"]
    fn integration_verify_ssh_key_for_github() {
        let ok = crate::git::verify_ssh_key_for_host("github.com").unwrap();
        assert!(ok, "本机 SSH key 应能认证 GitHub");
    }

    /// S-SYNC-PUBLIC-LEAK-ON-RATELIMIT 守护（2026-07-27，第七十七轮）：
    /// RateLimited 是独立变体，不是 Ambiguous——确保 enum 变体存在 + 类型区分。
    /// 限流硬阻断（非放行）依赖此类型区分。
    #[test]
    fn rate_limited_is_distinct_from_ambiguous() {
        let r = PrivacyVerdict::RateLimited("test".to_string());
        let a = PrivacyVerdict::Ambiguous("test".to_string());
        // 两者是不同变体（matches! 互斥）
        assert!(matches!(r, PrivacyVerdict::RateLimited(_)));
        assert!(!matches!(r, PrivacyVerdict::Ambiguous(_)));
        assert!(matches!(a, PrivacyVerdict::Ambiguous(_)));
        assert!(!matches!(a, PrivacyVerdict::RateLimited(_)));
    }
}
