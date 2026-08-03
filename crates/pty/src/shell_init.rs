// Shell init 脚本构造——注入 OSC 133 shell prompt marker。
// 参考 Terax shell_init.rs 设计，macOS-only。
//
// 核心策略（ZDOTDIR / --rcfile 方案，保留用户原有配置）：
// - zsh：把 octopus 的 .zshenv/.zshrc 写到 `~/.cache/octopus/shell-integration/zsh/`，
//   用 ZDOTDIR 环境变量指向该目录；脚本内部 source 用户原 ZDOTDIR（存到
//   OCTOPUS_USER_ZDOTDIR），starship/p10k 等 prompt 框架照常工作。
// - bash：用 --rcfile 指向我们的 bashrc，脚本内部 source 用户原有 profile/bashrc。
// - 未知 shell（fish/sh 等）不注入，spawn 时记 info 日志。

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;

const ZSHENV: &str = include_str!("scripts/zshenv.zsh");
const ZSHRC: &str = include_str!("scripts/zshrc.zsh");
const BASHRC: &str = include_str!("scripts/bashrc.bash");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shell {
    Zsh,
    Bash,
    Other,
}

impl Shell {
    fn classify(path: &str) -> Self {
        match path.rsplit('/').next().unwrap_or("") {
            "zsh" => Shell::Zsh,
            "bash" => Shell::Bash,
            _ => Shell::Other,
        }
    }

    /// 检测登录 shell：优先 getpwuid，失败回退 $SHELL，再回退 /bin/zsh。
    fn detect() -> (Shell, String) {
        let path = login_shell()
            .or_else(|| std::env::var("SHELL").ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/zsh".into());
        (Self::classify(&path), path)
    }

    /// 用户配置的 shell 覆盖仅在指向真实文件时生效，否则回退自动检测。
    fn resolve(shell_override: Option<String>) -> (Shell, String) {
        if let Some(path) = shell_override
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            if Path::new(&path).is_file() {
                return (Self::classify(&path), path);
            }
            log::warn!("configured shell '{path}' not found, using auto-detect");
        }
        Self::detect()
    }
}

/// 获取登录 shell（getpwuid_r.pw_shell）。
///
/// 用可重入版 `getpwuid_r`（而非 `getpwuid`）：后者返回进程级静态缓冲区指针，
/// 两个 PTY 并发 spawn（用户快速连开两个终端）会竞争该缓冲区，读到错误 shell 路径。
/// `getpwuid_r` 由调用方提供 `passwd` 结构 + buffer，线程安全。
fn login_shell() -> Option<String> {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        use std::ptr;
        unsafe {
            let uid = libc::getuid();
            let mut pwd: libc::passwd = std::mem::zeroed();
            // POSIX 建议 sysconf(_SC_GETPW_R_SIZE_MAX) 拿初始大小；-1 时用 4KB 兜底。
            // ERANGE 时倍增重试，上限 256KiB（远超任何合理场景）。
            let mut cap = 4096usize;
            let cap_max = 256 * 1024;
            loop {
                let mut buf = vec![0u8; cap];
                let mut result: *mut libc::passwd = ptr::null_mut();
                let ret = libc::getpwuid_r(
                    uid,
                    &mut pwd,
                    buf.as_mut_ptr() as *mut libc::c_char,
                    buf.len(),
                    &mut result,
                );
                if ret == 0 {
                    if result.is_null() {
                        // uid 未找到
                        return None;
                    }
                    let shell_ptr = (*result).pw_shell;
                    if shell_ptr.is_null() {
                        return None;
                    }
                    return CStr::from_ptr(shell_ptr).to_str().ok().map(String::from);
                }
                if ret == libc::ERANGE {
                    if cap >= cap_max {
                        log::warn!("getpwuid_r: buffer 不足（已试 {cap} 字节），放弃");
                        return None;
                    }
                    cap *= 2;
                    continue;
                }
                // 其他错误（ENOENT 等）
                log::debug!("getpwuid_r failed: errno={ret}");
                return None;
            }
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// 构造 shell 启动命令（portable_pty CommandBuilder）。
///
/// 注入环境变量：
/// - `TERM=xterm-256color`
/// - `COLORTERM=truecolor`
/// - `OCTOPUS_TERMINAL=1`（agent hook 门控——只在 octopus PTY 中发 OSC）
///
/// 注入 OSC 133 shell prompt marker（zsh 用 ZDOTDIR + precmd/preexec hook，
/// bash 用 --rcfile + PROMPT_COMMAND/PS0）。
pub fn build_command(
    cwd: Option<&str>,
    shell: Option<String>,
) -> Result<CommandBuilder, String> {
    let (shell_kind, shell_path) = Shell::resolve(shell);
    let mut cmd = CommandBuilder::new(&shell_path);

    apply_common(&mut cmd, cwd);
    apply_shell_init(&mut cmd, &shell_kind, &shell_path);

    Ok(cmd)
}

fn apply_common(cmd: &mut CommandBuilder, cwd: Option<&str>) {
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("OCTOPUS_TERMINAL", "1");
    ensure_utf8_locale(cmd);

    let resolved_cwd = cwd
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| dirs::home_dir().filter(|p| p.is_dir()));
    if let Some(dir) = resolved_cwd {
        log::info!("pty cwd: {}", dir.display());
        cmd.cwd(dir);
    } else {
        log::warn!("pty cwd: no usable directory, inheriting from process");
    }
}

fn ensure_utf8_locale(cmd: &mut CommandBuilder) {
    let is_utf8 = |v: &str| {
        let up = v.to_ascii_uppercase();
        up.contains("UTF-8") || up.contains("UTF8")
    };
    let already_utf8 = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .any(|k| std::env::var(k).ok().as_deref().is_some_and(is_utf8));
    if already_utf8 {
        return;
    }
    #[cfg(target_os = "macos")]
    let fallback = "en_US.UTF-8";
    #[cfg(all(unix, not(target_os = "macos")))]
    let fallback = "C.UTF-8";
    cmd.env("LANG", fallback);
}

fn apply_shell_init(cmd: &mut CommandBuilder, shell: &Shell, shell_path: &str) {
    match shell {
        Shell::Zsh => {
            match prepare_zdotdir() {
                Ok(zdotdir) => {
                    // Guard against octopus-in-octopus :)
                    if let Ok(user_zd) = std::env::var("ZDOTDIR") {
                        if Path::new(&user_zd) != zdotdir.as_path() {
                            cmd.env("OCTOPUS_USER_ZDOTDIR", user_zd);
                        }
                    }
                    cmd.env("ZDOTDIR", &zdotdir);
                }
                Err(e) => {
                    log::warn!("zsh shell integration disabled: {e}");
                }
            }
            // Login shell so /etc/zprofile runs path_helper on macOS — without
            // this, GUI-launched apps get a minimal PATH missing Homebrew.
            cmd.arg("-l");
        }
        Shell::Bash => {
            match prepare_bash_rcfile() {
                Ok(rc) => {
                    cmd.arg("--rcfile");
                    cmd.arg(rc);
                }
                Err(e) => {
                    log::warn!("bash shell integration disabled: {e}");
                }
            }
            // bash ignores --rcfile under -l, so we use -i and source
            // /etc/profile from inside our rcfile to emulate login init.
            cmd.arg("-i");
        }
        Shell::Other => {
            log::info!(
                "unsupported shell '{}', spawning without integration",
                shell_path
            );
        }
    }
}

fn integration_root() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "could not resolve home dir".to_string())?;
    let root = home.join(".cache").join("octopus").join("shell-integration");
    fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
    Ok(root)
}

fn prepare_zdotdir() -> Result<PathBuf, String> {
    let dir = integration_root()?.join("zsh");
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    write_if_changed(&dir.join(".zshenv"), ZSHENV)?;
    write_if_changed(&dir.join(".zshrc"), ZSHRC)?;
    Ok(dir)
}

fn prepare_bash_rcfile() -> Result<PathBuf, String> {
    let dir = integration_root()?.join("bash");
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let rc = dir.join("bashrc");
    write_if_changed(&rc, BASHRC)?;
    Ok(rc)
}

fn write_if_changed(path: &Path, content: &str) -> Result<(), String> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == content {
            return Ok(());
        }
    }
    // Atomic replace: a parallel shell startup must never source a half-written file.
    let mut tmp: OsString = path.as_os_str().to_owned();
    tmp.push(".__octopus_tmp__");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, content).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename {} -> {}: {e}", tmp.display(), path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_known_shells() {
        assert!(matches!(Shell::classify("/bin/zsh"), Shell::Zsh));
        assert!(matches!(Shell::classify("/usr/bin/bash"), Shell::Bash));
        assert!(matches!(Shell::classify("/bin/sh"), Shell::Other));
        assert!(matches!(Shell::classify("/usr/bin/fish"), Shell::Other));
    }

    /// 回归 #12：getpwuid（非可重入）返回进程级静态缓冲区，并发调用会竞争。
    /// 改用 getpwuid_r 后，10 线程并发应全部拿到一致结果且无 panic。
    #[test]
    fn login_shell_concurrent_safe() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // 先拿到单线程基准值（若环境无 passwd 条目则跳过并发断言）
        let baseline = login_shell();

        let n = 10;
        let mismatches = Arc::new(AtomicUsize::new(0));
        let panics = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let mismatches = mismatches.clone();
            let panics = panics.clone();
            let baseline = baseline.clone();
            handles.push(std::thread::spawn(move || {
                // catch_unwind 捕获 panic（竞态损坏可能触发 UB，至少不静默崩溃）
                let res = std::panic::catch_unwind(|| login_shell());
                match res {
                    Ok(v) => {
                        if v != baseline {
                            mismatches.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    Err(_) => {
                        panics.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }
        for h in handles {
            h.join().expect("thread join");
        }
        assert_eq!(panics.load(Ordering::SeqCst), 0, "并发调用不应 panic");
        assert_eq!(
            mismatches.load(Ordering::SeqCst),
            0,
            "并发结果应与单线程基准一致"
        );
    }

    #[test]
    fn resolve_uses_an_existing_override() {
        let exe = std::env::current_exe().unwrap();
        let path = exe.to_string_lossy().into_owned();
        let (_, resolved) = Shell::resolve(Some(path.clone()));
        assert_eq!(resolved, path);
    }

    #[test]
    fn resolve_falls_back_when_override_missing() {
        let (_, path) = Shell::resolve(Some("/no/such/shell/xyz".into()));
        assert!(!path.is_empty());
        assert_ne!(path, "/no/such/shell/xyz");
    }

    #[test]
    fn resolve_falls_back_on_empty_override() {
        let (_, fallback) = Shell::resolve(Some("   ".into()));
        let (_, detected) = Shell::detect();
        assert_eq!(fallback, detected);
    }

    #[test]
    fn build_command_injects_zdotdir_for_zsh() {
        // build_command 对 zsh 应设置 ZDOTDIR env（指向 integration_root/zsh）。
        let zsh = which_zsh_for_test();
        let cmd = build_command(None, zsh).unwrap();
        let env: std::collections::HashMap<&str, &str> = cmd
            .iter_extra_env_as_str()
            .map(|(k, v)| (k, v))
            .collect();
        assert!(
            env.contains_key("ZDOTDIR"),
            "ZDOTDIR must be set for zsh, got env keys: {:?}",
            env.keys().collect::<Vec<_>>()
        );
        assert_eq!(env.get("OCTOPUS_TERMINAL"), Some(&"1"));
    }

    #[test]
    fn build_command_injects_rcfile_arg_for_bash() {
        let bash = which_bash_for_test();
        if bash.is_none() {
            return; // 没 bash 的环境跳过（CI 可能无 bash）
        }
        let cmd = build_command(None, bash).unwrap();
        let argv: Vec<_> = cmd
            .get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let has_rcfile = argv
            .windows(2)
            .any(|w| w[0] == "--rcfile" && w[1].contains("octopus"));
        assert!(has_rcfile, "--rcfile arg missing, argv={:?}", argv);
    }

    #[cfg(target_os = "macos")]
    fn which_zsh_for_test() -> Option<String> {
        // macOS 自带 /bin/zsh，测试用绝对路径保证可复现。
        Some("/bin/zsh".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    fn which_zsh_for_test() -> Option<String> {
        std::env::var("SHELL").ok()
    }

    fn which_bash_for_test() -> Option<String> {
        for p in ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"] {
            if Path::new(p).is_file() {
                return Some(p.to_string());
            }
        }
        None
    }
}
