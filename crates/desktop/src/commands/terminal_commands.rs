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
/// - gitignore 过滤：用 `ignore` crate（在 git repo 内尊重 .gitignore）
/// - 错误（无权限/不存在）返回空数组（不阻断 UI）
#[tauri::command]
pub fn terminal_list_dir(path: String, show_hidden: bool) -> Result<Vec<FileEntry>, String> {
    list_dir_inner(&path, show_hidden)
}

/// 纯逻辑核心（可单测，不依赖 Tauri State）。
fn list_dir_inner(path: &str, show_hidden: bool) -> Result<Vec<FileEntry>, String> {
    let root = std::path::Path::new(path);

    // gitignore 过滤：在 git repo 内用 ignore crate 列非忽略项；
    // 非 git repo 直接列全部（ignore crate 的 WalkBuilder 在非 repo 目录也工作，但
    // 会尝试找 .git 触发 macOS TCC——这里先判断是否在 repo 内）。
    let ignored_names = git_ignored_names(root);

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
            // gitignore 过滤
            if !ignored_names.is_empty() && ignored_names.contains(&name) {
                return None;
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

/// 在 git repo 内返回被 gitignore 的直接子项名；非 repo 返回空集。
fn git_ignored_names(dir: &std::path::Path) -> std::collections::HashSet<String> {
    if !in_git_repo(dir) {
        return std::collections::HashSet::new();
    }
    ignore::WalkBuilder::new(dir)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(false)
        .parents(true)
        .max_depth(Some(1))
        .follow_links(false)
        .build()
        .flatten()
        .filter_map(|d| {
            let name = d.file_name();
            name.to_str().map(|s| s.to_string())
        })
        .collect()
}

/// 判断目录是否在 git repo 内（向上找 .git，不向下递归）。
fn in_git_repo(dir: &std::path::Path) -> bool {
    let mut cur = dir;
    loop {
        if cur.join(".git").exists() {
            return true;
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return false,
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
}
