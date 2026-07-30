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
