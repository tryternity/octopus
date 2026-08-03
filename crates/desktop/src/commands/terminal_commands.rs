//! 内嵌终端 Tauri 命令层。
//!
//! 把 `octopus-pty` crate 的纯逻辑 spawn 桥接到 Tauri：
//! - `pty_open`：创建 on_data/on_exit Channel，spawn PTY，闭包转发输出到 Channel +
//!   agent 状态转换 emit "agent://signal" 事件。
//! - `pty_write`：raw body + `x-pty-id` header，绕过 JSON（按键是延迟敏感路径）。
//! - `pty_resize` / `pty_close`：直通。
//!
//! PtyState 挂在 Tauri State（setup.rs manage），命令通过 `tauri::State<'_, PtyState>` 取。
//!
//! macOS-only：portable-pty + OSC agent 检测都是 macOS 优先。

use std::sync::atomic::Ordering;
use std::thread;

use tauri::ipc::{Channel, InvokeBody, Response};
use tauri::{AppHandle, Emitter, State};

use octopus_pty::{spawn, AgentSignal, PtyState, Transition};

/// agent 状态信号事件名（emit 到前端，前端 listen 更新 tab 徽章）。
const AGENT_SIGNAL_EVENT: &str = "agent://signal";

/// 打开一个 PTY session。
///
/// 前端传入 cols/rows/cwd/shell + 两个 Channel（on_data 接收输出，on_exit 接收退出码）。
/// 返回 session id，后续 write/resize/close 用它寻址。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pty_open(
    app: AppHandle,
    state: State<'_, PtyState>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    shell: Option<String>,
    on_data: Channel<Response>,
    on_exit: Channel<i32>,
) -> Result<u32, String> {
    let id = state.alloc_id();
    let id_for_signal = id;
    let app_for_signal = app.clone();

    // spawn 是阻塞的（openpty + 起子进程），放 spawn_blocking 避免阻塞 async runtime。
    let result = tauri::async_runtime::spawn_blocking(move || {
        spawn(
            id,
            cols,
            rows,
            cwd.as_deref(),
            shell,
            // on_data：flusher 合并后的 chunk → Channel
            move |chunk: Vec<u8>| {
                let _ = on_data.send(Response::new(chunk));
            },
            // on_exit：waiter 收到的退出码 → Channel
            move |code: i32| {
                let _ = on_exit.send(code);
            },
            // on_signal：OSC 解析出的 Transition → emit "agent://signal"
            move |t: Transition| {
                let signal: AgentSignal = t.into_signal(id_for_signal);
                let _ = app_for_signal.emit(AGENT_SIGNAL_EVENT, signal);
            },
        )
    })
    .await
    .map_err(|e| {
        log::error!("pty_open join failed: {e}");
        e.to_string()
    })?
    .map_err(|e| {
        log::error!("pty_open failed: {e}");
        e
    })?;

    // spawn 返回 (Arc<PtySession>, PtySize)，只存 session，size 丢弃（前端用 cols/rows）。
    let (session, _size) = result;
    state.sessions.write().insert(id, session);

    // shell 可能在 insert 前就退出（rc 文件里 `exit`、瞬时失败）；waiter 的 reap
    // 跑时 id 还没注册。re-check 并 reap，避免 PTY 孤儿。
    let exited = state
        .sessions
        .read()
        .get(&id)
        .map(|s| s.exited.load(Ordering::Acquire))
        .unwrap_or(false);
    if exited {
        if let Some(s) = state.sessions.write().remove(&id) {
            thread::Builder::new()
                .name(format!("octopus-pty-drop-{id}"))
                .spawn(move || drop(s))
                .map_err(|e| format!("spawn pty drop thread: {e}"))?;
        }
    }
    log::info!("pty opened id={id} cols={cols} rows={rows}");
    Ok(id)
}

/// 写入 PTY（用户按键）。
///
/// 走 raw body + `x-pty-id` header 绕过 JSON——每次按键都 JSON 序列化会累积延迟。
/// 前端 fetch 用 `body: new Uint8Array(...)` + header `x-pty-id: <id>`。
#[tauri::command]
pub fn pty_write(
    state: State<PtyState>,
    request: tauri::ipc::Request,
) -> Result<(), String> {
    let id: u32 = request
        .headers()
        .get("x-pty-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "pty_write: missing x-pty-id header".to_string())?;
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("pty_write: expected raw body".to_string());
    };
    let session = state
        .sessions
        .read()
        .get(&id)
        .cloned()
        .ok_or_else(|| {
            log::warn!("pty_write: unknown id={id}");
            "no session".to_string()
        })?;
    // 绑定局部变量，确保 MutexGuard 在 session（Arc clone）前 drop。
    let result = session.write(bytes).map_err(|e| {
        // EPIPE 正常——子进程已退出。
        log::debug!("pty_write id={id} failed: {e}");
        e.to_string()
    });
    result
}

/// 调整 PTY 尺寸（窗口 resize 时前端调）。
#[tauri::command]
pub fn pty_resize(
    state: State<PtyState>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let session = state
        .sessions
        .read()
        .get(&id)
        .cloned()
        .ok_or_else(|| {
            log::warn!("pty_resize: unknown id={id}");
            "no session".to_string()
        })?;
    session.resize(cols, rows).map_err(|e| {
        log::warn!("pty_resize id={id} failed: {e}");
        e.to_string()
    })
}

/// 关闭 PTY session（关 tab 时前端调）。
///
/// detach drop：避免阻塞 Tauri worker 线程（portable-pty 关闭 master 可能有 IO drain）。
#[tauri::command]
pub fn pty_close(state: State<PtyState>, id: u32) -> Result<(), String> {
    let session = state.sessions.write().remove(&id);
    if let Some(s) = session {
        s.kill();
        log::info!("pty closed id={id}");
        thread::Builder::new()
            .name(format!("octopus-pty-drop-{id}"))
            .spawn(move || {
                let t0 = std::time::Instant::now();
                drop(s);
                log::info!("pty session id={id} dropped in {}ms", t0.elapsed().as_millis());
            })
            .map_err(|e| format!("spawn pty drop thread: {e}"))?;
    } else {
        log::debug!("pty_close: unknown id={id}");
    }
    Ok(())
}

// ── 文件树侧栏 ──

/// 文件树条目（camelCase，前端 FileTreePanel 消费）。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// "dir" | "file"
    pub kind: String,
}

/// 列出目录的直接子项（文件树侧栏用）。
///
/// - 目录优先 + case-insensitive 排序
/// - `show_hidden=false`：过滤 dot 前缀（`.git` / `.env` 等）
/// - gitignore 过滤：在 git repo 内用 `ignore` crate 的 Gitignore 匹配器
///   判断每个直接子项是否被 `.gitignore` 命中（命中 → 隐藏）；非 repo 不过滤
/// - 错误（无权限/不存在）返回空数组（不阻断 UI）
#[tauri::command]
pub fn terminal_list_dir(path: String, show_hidden: bool) -> Result<Vec<FileEntry>, String> {
    list_dir_inner(&path, show_hidden)
}

/// 纯逻辑核心（可单测，不依赖 Tauri State）。
fn list_dir_inner(path: &str, show_hidden: bool) -> Result<Vec<FileEntry>, String> {
    let root = std::path::Path::new(path);

    // gitignore 过滤：在 git repo 内用 ignore crate 的 Gitignore 匹配器列表，
    // 对每个直接子项判断是否被 .gitignore 命中（命中 → 隐藏）。
    // 非 git repo 不过滤（避免 ignore crate 的 WalkBuilder 触发 macOS TCC 找 .git）。
    // 每个 .gitignore 用其所在目录为 root 单独建 matcher（修复 F1：前导斜杠 pattern
    // 锚定问题——合并进单一 root 会在嵌套场景误隐藏 git 跟踪的文件）。
    let matchers = build_gitignore_matchers(root);

    let read = std::fs::read_dir(root).map_err(|e| {
        log::debug!("terminal_list_dir({}) failed: {e}", root.display());
        e.to_string()
    })?;

    let mut entries: Vec<FileEntry> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // dot 文件过滤
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            // gitignore 过滤：matchers 按优先级低→高排列。git「最后匹配胜出」语义：
            // 从高优先级（末尾，离 dir 近）向低优先级查，第一个 Match 决定。
            // Ignore → 隐藏；Whitelist(`!`) → 可见（覆盖低优先级的 Ignore）。
            if !matchers.is_empty() {
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                for m in matchers.iter().rev() {
                    match m.matched(&e.path(), is_dir) {
                        ignore::Match::Ignore(_) => return None,
                        ignore::Match::Whitelist(_) => break, // 该项被白名单保留
                        ignore::Match::None => {} // 继续查更低优先级
                    }
                }
            }
            let kind = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                "dir"
            } else {
                "file"
            };
            Some(FileEntry { name, kind: kind.to_string() })
        })
        .collect();

    // 目录优先 + case-insensitive 排序
    entries.sort_by(|a, b| {
        let dir_a = a.kind == "dir";
        let dir_b = b.kind == "dir";
        dir_b.cmp(&dir_a).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

/// 构建 gitignore 匹配器列表，用于判断 `dir` 的直接子项是否应被隐藏。
///
/// **每个 `.gitignore` 用其所在目录为 root 单独建一个 matcher**（而非全合并进 `dir`）。
/// 这样前导斜杠 pattern（`/build`）锚定到该 .gitignore 所在目录，与 git 语义一致。
/// 合并进单一 root 会导致 `/build`（actual=`build`，无 `**/`）在嵌套场景误命中——
/// `dir=repo/src` 时 repo 根的 `/build` 会误隐藏 `src/build`（git 实际跟踪它）。
///
/// 返回的 matcher 按 git 优先级排序（低 → 高）：调用方逐个 matched，任一命中即隐藏。
/// 优先级（低 → 高）：全局 excludesfile < .git/info/exclude < repo_root .gitignore <
/// ... < dir .gitignore。后查的（更高优先级）可覆盖先查的（whitelist `!` 胜出）。
///
/// 非 git repo（无 `.git`）返回空 Vec（不过滤）。
fn build_gitignore_matchers(dir: &std::path::Path) -> Vec<ignore::gitignore::Gitignore> {
    let mut matchers: Vec<ignore::gitignore::Gitignore> = Vec::new();
    let repo_root = match find_repo_root(dir) {
        Some(r) => r,
        None => return matchers, // 非 repo
    };
    // 全局 excludesfile（最低优先级）—— root 用 repo_root（D2 修复）。
    // git 语义下全局 excludesfile 的 pattern 相对工作目录根（即 repo root），而非该
    // 文件所在目录。旧用 global.parent() 在 excludesfile 恰好位于 repo 内部时（极罕见）
    // 会导致前导斜杠 pattern 锚定错误；用 repo_root 对齐 git 真实行为。
    if let Some(global) = ignore::gitignore::gitconfig_excludes_path() {
        if global.is_file() {
            let mut b = ignore::gitignore::GitignoreBuilder::new(&repo_root);
            let _ = b.add(&global);
            if let Ok(m) = b.build() {
                matchers.push(m);
            }
        }
    }
    // .git/info/exclude（root = repo_root）
    let info_exclude = repo_root.join(".git").join("info").join("exclude");
    if info_exclude.is_file() {
        let mut b = ignore::gitignore::GitignoreBuilder::new(&repo_root);
        let _ = b.add(&info_exclude);
        if let Ok(m) = b.build() {
            matchers.push(m);
        }
    }
    // 从 repo_root 向下到 dir（含），逐级 .gitignore（root = 该 .gitignore 所在目录）
    let mut chain: Vec<std::path::PathBuf> = Vec::new();
    let mut cur = Some(dir.to_path_buf());
    while let Some(p) = cur {
        chain.push(p.clone());
        if p == repo_root {
            break;
        }
        cur = p.parent().map(|x| x.to_path_buf());
    }
    for p in chain.into_iter().rev() {
        let gi = p.join(".gitignore");
        if gi.is_file() {
            let mut b = ignore::gitignore::GitignoreBuilder::new(&p);
            let _ = b.add(&gi);
            if let Ok(m) = b.build() {
                matchers.push(m);
            } else {
                log::debug!("build_gitignore_matchers: build {} 失败", gi.display());
            }
        }
    }
    matchers
}

/// 向上查找包含 `.git` 的最近祖先，返回其路径（repo root）。找不到返回 None。
fn find_repo_root(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = dir;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_test_dir() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        // 创建测试结构
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::write(root.join("main.rs"), "").unwrap();
        fs::write(root.join(".hidden"), "").unwrap();
        fs::create_dir_all(root.join(".secret")).unwrap();
        dir
    }

    #[test]
    fn list_dir_dirs_first_then_files() {
        let dir = make_test_dir();
        let entries = list_dir_inner(dir.path().to_str().unwrap(), false).unwrap();
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        // 目录在前
        let first_file_idx = kinds.iter().position(|&k| k == "file").unwrap();
        assert!(kinds[..first_file_idx].iter().all(|&k| k == "dir"));
    }

    #[test]
    fn list_dir_case_insensitive_sort() {
        let dir = make_test_dir();
        let entries = list_dir_inner(dir.path().to_str().unwrap(), false).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // docs < src（目录），main.rs < README.md（文件，case-insensitive）
        assert_eq!(names, vec!["docs", "src", "main.rs", "README.md"]);
    }

    #[test]
    fn list_dir_hide_dot_files_by_default() {
        let dir = make_test_dir();
        let entries = list_dir_inner(dir.path().to_str().unwrap(), false).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&".hidden"));
        assert!(!names.contains(&".secret"));
    }

    #[test]
    fn list_dir_show_hidden_includes_dot() {
        let dir = make_test_dir();
        let entries = list_dir_inner(dir.path().to_str().unwrap(), true).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&".hidden"));
        assert!(names.contains(&".secret"));
    }

    #[test]
    fn list_dir_nonexistent_returns_error() {
        let result = list_dir_inner("/no/such/path/xyz", false);
        assert!(result.is_err());
    }

    /// 在带 .git + .gitignore 的临时 repo 内验证 gitignore 过滤语义：
    /// - target/、node_modules/、*.log 被隐藏
    /// - src/、Cargo.toml、keep.log（!keep.log 白名单）可见
    /// 回归 #1（旧实现 WalkBuilder 语义反向，把非 ignored 项当「要隐藏的集合」）。
    #[test]
    fn list_dir_filters_gitignore_in_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        // 初始化 .git（让 find_repo_root 命中）
        fs::create_dir_all(root.join(".git")).unwrap();
        // .gitignore：目录型、glob、白名单各覆盖一种
        fs::write(
            root.join(".gitignore"),
            "target\nnode_modules/\n*.log\n!keep.log\n",
        )
        .unwrap();
        // 匹配的被忽略项
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("debug.log"), "").unwrap();
        // 白名单（!keep.log）应可见
        fs::write(root.join("keep.log"), "").unwrap();
        // 非匹配项应可见
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();

        // show_hidden=true 隔离 dot 分支，专测 gitignore
        let entries = list_dir_inner(root.to_str().unwrap(), true).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();

        // 被忽略项必须隐藏
        assert!(!names.contains(&"target"), "target 应被 gitignore 隐藏");
        assert!(!names.contains(&"node_modules"), "node_modules 应被 gitignore 隐藏");
        assert!(!names.contains(&"debug.log"), "*.log 应被 gitignore 隐藏");
        // 非忽略项必须可见（这是旧 bug 的核心：旧实现把这些隐藏了）
        assert!(names.contains(&"src"), "src 应可见（旧 bug 错误隐藏）");
        assert!(names.contains(&"Cargo.toml"), "Cargo.toml 应可见（旧 bug 错误隐藏）");
        // 白名单（!keep.log）可见
        assert!(names.contains(&"keep.log"), "keep.log 被 !keep.log 白名单保留");
        // .git 目录在 show_hidden=true 下可见（不在 gitignore 里，仅 dot 过滤会挡）
        assert!(names.contains(&".git"), ".git 在 show_hidden 下应可见");
    }

    /// 非 git repo（无 .git）不过滤，所有项可见。
    #[test]
    fn list_dir_no_gitignore_outside_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        // 有 .gitignore 但无 .git → 不算 repo
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        fs::write(root.join("debug.log"), "").unwrap();
        fs::write(root.join("keep.txt"), "").unwrap();

        let entries = list_dir_inner(root.to_str().unwrap(), true).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // 非 repo：.gitignore 不生效，两者都可见
        assert!(names.contains(&"debug.log"));
        assert!(names.contains(&"keep.txt"));
    }

    /// 回归 F1：嵌套目录 + 前导斜杠 pattern 误隐藏。
    /// repo 根 .gitignore 有 `/build`（仅忽略根 build/）。用户在 repo/src/ 打开文件树
    /// （dir=repo/src），src/build/ 应**可见**（git 跟踪它，根 /build 不匹配 src/build）。
    /// 旧实现把所有 .gitignore 合并进 dir=src 单一 root，/build 的 actual=`build`（无 **/）
    /// 在 src 下误命中 src/build → 误隐藏 git 跟踪的文件。
    #[test]
    fn list_dir_nested_leading_slash_does_not_over_hide() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        fs::create_dir_all(repo.join(".git")).unwrap();
        // repo 根 .gitignore：/build 仅忽略根 build/，*.log 全局
        fs::write(repo.join(".gitignore"), "/build\n*.log\n").unwrap();
        // 根 build/（应被忽略）+ src/build/（应可见，git 跟踪）
        fs::create_dir_all(repo.join("build")).unwrap();
        fs::create_dir_all(repo.join("src").join("build")).unwrap();
        fs::write(repo.join("src").join("build").join("file.txt"), "").unwrap();
        fs::write(repo.join("src").join("keep.txt"), "").unwrap();
        fs::write(repo.join("src").join("debug.log"), "").unwrap();

        // 列 src/（嵌套场景）
        let src = repo.join("src");
        let entries = list_dir_inner(src.to_str().unwrap(), true).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // src/build/ 必须可见（根 /build 不匹配它）——这是 F1 的核心
        assert!(
            names.contains(&"build"),
            "src/build 应可见（根 /build 不匹配嵌套目录），got: {:?}",
            names
        );
        // *.log 全局 pattern 仍生效（无斜杠 → **/，跨目录匹配）
        assert!(!names.contains(&"debug.log"), "*.log 全局应隐藏 src/debug.log");
        // 普通文件可见
        assert!(names.contains(&"keep.txt"));

        // 对照：列 repo 根（dir==repo_root），根 build/ 应被 /build 忽略
        let root_entries = list_dir_inner(repo.to_str().unwrap(), true).unwrap();
        let root_names: Vec<&str> = root_entries.iter().map(|e| e.name.as_str()).collect();
        assert!(!root_names.contains(&"build"), "根 /build 应隐藏根 build/");
        assert!(root_names.contains(&"src"), "src 应可见");
    }
}
