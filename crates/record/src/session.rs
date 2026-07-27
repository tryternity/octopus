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

#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
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
    /// 录制真正开始的时刻（reader task 收到 RecordingStarted 事件时记）。
    /// 用于 RecordControl 浮窗 mount 时计算已录时长（浮窗创建晚于 recording-started 事件，
    /// 收不到事件，靠查这个字段 + Instant::now() 算 elapsed）。
    /// pause 时不清（保持原值，resume 后继续累计）；stop/kill 时清空。
    recording_started_at: Option<std::time::Instant>,
    /// helper 通过 HelperEvent::Error 报回的错误（reader task 收到时存）。
    /// start() 的 wait_for_state 检测到它立即短路返回 Err（避免傻等 10s 超时），
    /// 同时调用方拿到的错误带 helper 真实原因（如 permissionDenied / sourceNotFound）。
    /// 每次读取后 take 清空（避免下次 start 误读上次错误）。
    last_helper_error: Option<RecordError>,
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
                recording_started_at: None,
                last_helper_error: None,
            })),
        }
    }

    /// 强制重置到 Idle：SIGKILL helper + 清空所有运行时字段。
    ///
    /// **三个调用点共用**（曾因 start() 超时 / stop() 超时不重置 state 导致 session 卡死，
    /// 之后所有 start 撞 AlreadyRunning —— 2026-07-26 P0 修复）：
    /// - `kill()` —— 用户主动强杀
    /// - `start()` Err 路径 —— spawn 失败 / 等不到 recording-started
    /// - `stop()` Err 路径 —— helper 10s 内不退出（fallback 到强杀）
    ///
    /// **自身加锁**：调用方**不得**持有 inner 锁（避免重入死锁）。
    async fn reset_to_idle(&self) {
        let mut inner = self.inner.lock().await;
        if let Some(mut child) = inner.child.take() {
            // start_kill 是 SIGKILL，不等待优雅退出（helper 可能已卡死，wait 会再卡）
            let _ = child.start_kill();
            // 后台 reap 僵尸进程（不阻塞当前调用）——tokio task 异步 wait
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        inner.stdin = None;
        inner.state = SessionState::Idle;
        inner.recording_started_at = None;
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

    /// 当前录制已进行的秒数（用于 RecordControl 浮窗 mount 时初始化计时器）。
    ///
    /// 浮窗创建晚于 recording-started 事件，收不到事件，靠此方法 + Instant::now() 算 elapsed。
    /// pause 时不清 recording_started_at（保持原值，resume 后继续累计）——但 pause 期间
    /// elapsed 仍在增长，调用方需结合 state 判断（paused 时不再 +1）。
    /// 返回 None 表示未在录制 / paused 中（recording_started_at 为 None）。
    pub async fn elapsed_secs(&self) -> Option<u64> {
        let inner = self.inner.lock().await;
        inner.recording_started_at.map(|t| t.elapsed().as_secs())
    }

    /// 启动录制。
    /// `helper_path` 是 helper 二进制绝对路径（由 platform 模块解析）。
    /// `on_event` 回调在收到非命令响应事件时调用（如 Warning/Error）。
    ///
    /// **失败路径清理（2026-07-26 P0 修复）**：spawn 失败 / 等不到 recording-started 时，
    /// 必须 reset_to_idle（SIGKILL helper + state=Idle），否则 session 卡在 Starting，
    /// 之后所有 start 都撞 AlreadyRunning。原代码用 `?` 直接返回不清理——已修。
    pub async fn start(
        &self,
        helper_path: &PathBuf,
        request: RecordingRequest,
        on_event: impl Fn(HelperEvent) + Send + 'static,
    ) -> RecordResult<StartedInfo> {
        {
            let mut inner = self.inner.lock().await;
            if inner.state != SessionState::Idle {
                return Err(RecordError::AlreadyRunning);
            }
            inner.state = SessionState::Starting;
            inner.last_helper_error = None; // 清上次错误
            // 存快照供 hotkey/tray stop 入库用（recording_id / source / video / audio）
            inner.last_request = Some(request.clone());
        }

        let req_json = serde_json::to_string(&request)?;
        // kill_on_drop：极端情况（进程 panic / SessionInner drop）下 helper 不残留为孤儿。
        let child_result = tokio::process::Command::new(helper_path)
            .arg(&req_json)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn();

        // spawn 失败：清理已设置的 Starting state
        let mut child = match child_result {
            Ok(c) => c,
            Err(e) => {
                self.reset_to_idle().await;
                return Err(RecordError::SpawnFailed(e));
            }
        };

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        // ⚠️ stderr 必须读，否则 64KB 管道缓冲填满后 helper 阻塞在 write(stderr) →
        // 永不发 recording-started → 父进程超时。曾因 stderr piped 但从不 take/read 导致
        // 录屏 timeout（2026-07-26 P0 修复，根因之一）。
        let stderr = child.stderr.take();

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
                            HelperEvent::RecordingStarted { .. } => {
                                inner.state = SessionState::Recording;
                                inner.recording_started_at = Some(std::time::Instant::now());
                            }
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
                            HelperEvent::Error { code, message } => {
                                // helper 主动报错——存错误让 wait_for_state 立即短路返回，
                                // 避免 start() 傻等 10s 超时（helper 通常 exit(1) 后才报错，
                                // 但某些路径如 permissionDenied 是先 emit 再 exit，差距几秒）。
                                inner.last_helper_error = Some(RecordError::HelperError {
                                    code: code.clone(),
                                    message: message.clone(),
                                });
                            }
                            _ => {}
                        }
                    }
                    on_event(event);
                }
            }
        });

        // stderr reader task：每行 log::debug，防管道阻塞
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    log::debug!("[record][helper stderr] {line}");
                }
            });
        }

        {
            let mut inner = self.inner.lock().await;
            inner.child = Some(child);
            inner.stdin = Some(stdin);
        }

        // 等待 RecordingStarted 事件（state 变为 Recording）
        // 失败时：reset_to_idle 清理（避免卡死），再把错误返回给调用方
        if let Err(e) = self
            .wait_for_state(SessionState::Recording, Duration::from_secs(CMD_TIMEOUT_SECS))
            .await
        {
            // 优先返回 helper 真实错误（如 permissionDenied），而非 Timeout（更有用）
            let real_err = {
                let mut inner = self.inner.lock().await;
                inner.last_helper_error.take()
            };
            self.reset_to_idle().await;
            return Err(real_err.unwrap_or(e));
        }

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
        // 超时 fallback：reset_to_idle 强杀（2026-07-26 P0 修复——曾 stop 超时不清理，
        // state 卡 Stopping，与 start 超时同类 bug）
        let exit_status = {
            let mut inner = self.inner.lock().await;
            if let Some(mut child) = inner.child.take() {
                drop(inner);
                match tokio::time::timeout(Duration::from_secs(CMD_TIMEOUT_SECS), child.wait()).await {
                    Ok(s) => s.map_err(RecordError::SpawnFailed)?,
                    Err(_) => {
                        // 超时：child 还在跑，放回 inner 让 reset_to_idle kill
                        {
                            let mut inner = self.inner.lock().await;
                            inner.child = Some(child);
                        }
                        self.reset_to_idle().await;
                        return Err(RecordError::Timeout { event: "stop-exit" });
                    }
                }
            } else {
                return Err(RecordError::NotRunning);
            }
        };
        log::debug!("[record] helper exited: {exit_status}");

        let mut inner = self.inner.lock().await;
        inner.stdin = None;
        inner.state = SessionState::Idle;
        inner.recording_started_at = None;
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
        // 复用 reset_to_idle（SIGKILL + 清字段），保持单一清理路径
        self.reset_to_idle().await;
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
            // 短路：helper 报错时不再傻等（permissionDenied / sourceNotFound 等）
            // reader task 已把错误存入 last_helper_error，这里取出来立即返回
            {
                let mut inner = self.inner.lock().await;
                if let Some(err) = inner.last_helper_error.take() {
                    return Err(err);
                }
                if inner.state == expected {
                    return Ok(());
                }
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
