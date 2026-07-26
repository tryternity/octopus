//! RecordSession：录制会话控制器。
//!
//! 管理 helper 子进程的完整生命周期：
//! - start: spawn helper，等 RecordingStarted 事件
//! - pause/resume: stdin 写命令，等对应事件
//! - stop: stdin 写 stop，等 RecordingStopped + 进程退出
//! - kill: 强制 SIGKILL（文件可能损坏）

use crate::error::{RecordError, RecordResult};
use crate::protocol::{HelperEvent, RecordingRequest};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Starting,
    Recording,
    Paused,
    Stopping,
}

#[derive(serde::Serialize)]
pub struct StartedInfo {
    pub width: u32,
    pub height: u32,
}

pub struct StoppedInfo {
    pub screen_path: PathBuf,
    pub duration_ms: i64,
    pub file_size: u64,
}

/// 命令等待超时（秒）。helper 卡死时避免命令永久挂起。
const CMD_TIMEOUT_SECS: u64 = 10;

pub struct RecordSession {
    inner: Arc<Mutex<SessionInner>>,
}

struct SessionInner {
    state: SessionState,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    /// 最近一次 start 的 RecordingRequest 快照——hotkey/tray stop 路径需要这些字段
    /// 入库（recording_id / source / video / audio），但它们不在 SessionState 里。
    /// start 时存快照，stop 时 desktop 层读出来组装 RecordingMeta。
    /// None 表示从未 start 过（或上次 stop 后被清空）。
    last_request: Option<RecordingRequest>,
    /// reader task 在收到 RecordingStopped 事件时存的精确停止信息
    /// （screen_path / duration_ms / file_size，从 helper 报回的 payload 直接取）。
    /// stop() 方法在 helper 进程退出后 take 它返回给调用方。
    /// None 表示未收到 RecordingStopped（异常退出 / kill 路径），调用方 fallback。
    last_stopped: Option<StoppedInfo>,
}

impl RecordSession {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SessionInner {
                state: SessionState::Idle,
                child: None,
                stdin: None,
                last_request: None,
                last_stopped: None,
            })),
        }
    }

    /// 读最近一次 start 的 RecordingRequest 快照（None = 从未 start / 已 stop 清空）。
    ///
    /// 用途：hotkey/tray stop 路径需要 recording_id / source / video / audio 字段入库，
    /// 但这些不在 SessionState enum 里。start 时存快照，stop 后由 stop() 清空。
    pub async fn last_start_request(&self) -> Option<RecordingRequest> {
        self.inner.lock().await.last_request.clone()
    }

    pub async fn state(&self) -> SessionState {
        self.inner.lock().await.state
    }

    /// 启动录制。
    /// `helper_path` 是 helper 二进制绝对路径（由 platform 模块解析）。
    /// `on_event` 回调在收到非命令响应事件时调用（如 Warning/Error）。
    pub async fn start(
        &self,
        helper_path: &PathBuf,
        request: RecordingRequest,
        on_event: impl Fn(HelperEvent) + Send + 'static,
    ) -> RecordResult<StartedInfo> {
        let mut inner = self.inner.lock().await;
        if inner.state != SessionState::Idle {
            return Err(RecordError::AlreadyRunning);
        }
        inner.state = SessionState::Starting;
        // 存快照供 hotkey/tray stop 入库用（recording_id / source / video / audio）
        inner.last_request = Some(request.clone());

        let req_json = serde_json::to_string(&request)?;
        let mut child = tokio::process::Command::new(helper_path)
            .arg(&req_json)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(RecordError::SpawnFailed)?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        // 启动 stdout reader task：按行解析 JSON 事件
        let inner_clone = self.inner.clone();
            tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Ok(event) = serde_json::from_str::<HelperEvent>(&line) {
                    // 更新 state + 捕获 RecordingStopped payload（精确 duration_ms 等）
                    {
                        let mut inner = inner_clone.lock().await;
                        match &event {
                            HelperEvent::Ready { .. } => inner.state = SessionState::Starting,
                            HelperEvent::RecordingStarted { .. } => inner.state = SessionState::Recording,
                            HelperEvent::RecordingPaused { .. } => inner.state = SessionState::Paused,
                            HelperEvent::RecordingResumed { .. } => inner.state = SessionState::Recording,
                            HelperEvent::RecordingStopped { screen_path, duration_ms, file_size } => {
                                // 精确停止信息存入 last_stopped——stop() 方法 take 返回给调用方
                                inner.last_stopped = Some(StoppedInfo {
                                    screen_path: PathBuf::from(screen_path),
                                    duration_ms: *duration_ms,
                                    file_size: *file_size,
                                });
                            }
                            _ => {}
                        }
                    }
                    on_event(event);
                }
            }
        });

        inner.child = Some(child);
        inner.stdin = Some(stdin);

        // 等待 RecordingStarted 事件（state 变为 Recording）
        drop(inner); // 释放锁让 reader task 能更新 state
        self.wait_for_state(SessionState::Recording, Duration::from_secs(CMD_TIMEOUT_SECS)).await?;

        // 读出当前 width/height（从 StartedInfo，但 state 里没存——简化：从 request 取）
        Ok(StartedInfo {
            width: request.video.width,
            height: request.video.height,
        })
    }

    pub async fn pause(&self) -> RecordResult<()> {
        self.send_command("pause\n", SessionState::Paused).await
    }

    pub async fn resume(&self) -> RecordResult<()> {
        self.send_command("resume\n", SessionState::Recording).await
    }

    pub async fn stop(&self) -> RecordResult<StoppedInfo> {
        {
            let mut inner = self.inner.lock().await;
            if inner.state == SessionState::Idle {
                return Err(RecordError::NotRunning);
            }
            inner.state = SessionState::Stopping;
            if let Some(stdin) = inner.stdin.as_mut() {
                stdin.write_all(b"stop\n").await.map_err(RecordError::SpawnFailed)?;
            }
        }

        // 等 helper 进程退出（RecordingStopped 事件已在 reader task 处理）
        let exit_status = {
            let mut inner = self.inner.lock().await;
            if let Some(mut child) = inner.child.take() {
                drop(inner);
                tokio::time::timeout(Duration::from_secs(CMD_TIMEOUT_SECS), child.wait())
                    .await
                    .map_err(|_| RecordError::Timeout { event: "stop-exit" })?
                    .map_err(RecordError::SpawnFailed)?
            } else {
                return Err(RecordError::NotRunning);
            }
        };
        log::debug!("[record] helper exited: {exit_status}");

        let mut inner = self.inner.lock().await;
        inner.stdin = None;
        inner.state = SessionState::Idle;
        // 不清 last_request——desktop 层 stop_and_store 在 session.stop() 之后仍需读它入库。
        // 清空时机交给下次 start（start 时会覆盖 last_request = Some(new_request)）。

        // StoppedInfo：reader task 收到 RecordingStopped 时存的精确 payload。
        // take 出来返回；None 表示未收到（异常退出 / reader 还没处理到），
        // 调用方 fallback（按 recording_id 扫 recordings_dir 查文件，duration_ms 仍可能 0）。
        Ok(inner.last_stopped.take().unwrap_or(StoppedInfo {
            screen_path: PathBuf::new(),
            duration_ms: 0,
            file_size: 0,
        }))
    }

    pub async fn kill(&self) -> RecordResult<()> {
        let mut inner = self.inner.lock().await;
        if let Some(mut child) = inner.child.take() {
            child.start_kill().map_err(RecordError::SpawnFailed)?;
            let _ = child.wait().await;
        }
        inner.stdin = None;
        inner.state = SessionState::Idle;
        Ok(())
    }

    async fn send_command(&self, cmd: &str, expected: SessionState) -> RecordResult<()> {
        {
            let mut inner = self.inner.lock().await;
            if inner.state == SessionState::Idle {
                return Err(RecordError::NotRunning);
            }
            if let Some(stdin) = inner.stdin.as_mut() {
                stdin.write_all(cmd.as_bytes()).await.map_err(RecordError::SpawnFailed)?;
            }
        }
        self.wait_for_state(expected, Duration::from_secs(CMD_TIMEOUT_SECS)).await
    }

    async fn wait_for_state(&self, expected: SessionState, timeout: Duration) -> RecordResult<()> {
        let start = std::time::Instant::now();
        loop {
            let current = self.state().await;
            if current == expected {
                return Ok(());
            }
            if start.elapsed() >= timeout {
                return Err(RecordError::Timeout {
                    event: match expected {
                        SessionState::Recording => "recording-started",
                        SessionState::Paused => "recording-paused",
                        SessionState::Stopping => "stop-exit",
                        _ => "unknown",
                    },
                });
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Default for RecordSession {
    fn default() -> Self {
        Self::new()
    }
}
