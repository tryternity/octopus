// PTY session——portable-pty 封装。
// 参考 Terax session.rs 设计：3 线程模型（reader/flusher/waiter）。
//
// 与 Terax 的关键差异：
// - **不依赖 tauri**：on_data/on_exit/on_signal 是闭包回调（Send + 'static），
//   而非 tauri::ipc::Channel。pty crate 保持纯逻辑，desktop 层把闭包桥接到
//   Channel/emit。符合 octopus「独立 crate 不依赖 tauri」的分层。
// - macOS-only：去掉 Windows ConPTY/Job、WSL。
// - 去掉 da_filter（DA（Device Attributes）请求过滤，xterm 特性，octopus 不需要）。
//
// 3 线程职责：
// - **reader**：阻塞读 PTY master → OSC 解析（agent_detect）→ push 到 pending buffer
// - **flusher**：Condvar 等待 pending → 4ms coalesce → on_data(chunk)。批量化降低 IPC 压力
// - **waiter**：child.wait() → 等 reader join（EOF）→ flush tail → on_exit(code)
//
// 字段 drop 顺序：killer 先 drop（kill 子进程）→ writer（关输入管道）→ master（最后）。
// 这保证 reader 先因 EOF 退出，不会在读半途 master 被 drop。

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use parking_lot::Mutex as PMutex;
use portable_pty::{native_pty_system, ChildKiller, MasterPty, PtySize};

use crate::agent_detect::{AgentDetector, Transition};

// Flusher 在首字节到达后合并一个短窗口，这样发出的是 chunk 而非单字节。
// MAX_IDLE 是漏掉信号的兜底（Condvar 超时唤醒）。
const FLUSH_COALESCE: Duration = Duration::from_millis(4);
const FLUSH_MAX_IDLE: Duration = Duration::from_millis(50);
const READ_BUF: usize = 16 * 1024;
// pending buffer 上限。溢出时丢弃整个 buffer 并 emit SGR-reset + 提示。
// 只丢部分前缀会把 CSI 序列切成两半，破坏 xterm 屏幕状态。4 MiB ≈ 1000 个满屏 80x24。
const MAX_PENDING: usize = 4 * 1024 * 1024;
// 硬重置（ESC c）+ 暗色提示。背压丢弃 backlog 时原样写入流。
const OVERFLOW_NOTICE: &[u8] =
    b"\x1bc\x1b[2m[octopus: dropped output due to backpressure]\x1b[0m\r\n";

/// 单个 PTY 会话。
pub struct PtySession {
    pub id: u32,
    /// shell 进程 PID。0 表示未知；调用方检查时须跳过。
    pub shell_pid: u32,
    pub killer: PMutex<Box<dyn ChildKiller + Send>>,
    pub writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub master: PMutex<Box<dyn MasterPty + Send>>,
    /// waiter 线程在子进程退出后置位，pty_open 可据此 reap 提前死掉的 shell。
    pub exited: Arc<AtomicBool>,
}

impl PtySession {
    /// 写入 PTY stdin（用户输入）。
    pub fn write(&self, data: &[u8]) -> std::io::Result<()> {
        self.writer.lock().unwrap().write_all(data)
    }

    /// 调整 PTY 尺寸。
    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master.lock().resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// 终止子进程。
    pub fn kill(&self) {
        let _ = self.killer.lock().kill();
    }

    /// 子进程是否已退出。
    pub fn is_exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // 若 session Arc 被 drop 但没显式 pty_close（前端断连、窗口崩溃、dev HMR），
        // reader/flusher 线程会一直挂着持有子进程。这里 kill 让 reader 撞 EOF 退出。
        let _ = self.killer.lock().kill();
    }
}

/// spawn 出错时 kill 子进程的 RAII guard（避免半成品 setup 让 shell 泄漏）。
struct ChildKillGuard {
    killer: Option<Box<dyn ChildKiller + Send>>,
}

impl ChildKillGuard {
    fn new(killer: Box<dyn ChildKiller + Send>) -> Self {
        Self {
            killer: Some(killer),
        }
    }

    fn disarm(&mut self) {
        self.killer = None;
    }
}

impl Drop for ChildKillGuard {
    fn drop(&mut self) {
        if let Some(mut k) = self.killer.take() {
            let _ = k.kill();
        }
    }
}

/// spawn 一个 PTY session。
///
/// 回调（闭包，`Send + 'static`，由调用方桥接到 Tauri Channel/emit）：
/// - `on_data`：flusher 合并后的输出 chunk（Vec<u8>）
/// - `on_exit`：子进程退出码
/// - `on_signal`：OSC 解析出的 agent 状态转换
///
/// 返回 `(Arc<PtySession>, PtySize)`——session 供调用方注册到 PtyState，size 供回传前端。
#[allow(clippy::too_many_arguments)]
pub fn spawn<O, E, S>(
    id: u32,
    cols: u16,
    rows: u16,
    cwd: Option<&str>,
    shell: Option<String>,
    on_data: O,
    on_exit: E,
    on_signal: S,
) -> Result<(Arc<PtySession>, PtySize), String>
where
    O: Fn(Vec<u8>) + Send + Sync + 'static,
    E: Fn(i32) + Send + 'static,
    S: Fn(Transition) + Send + Sync + 'static,
{
    let pty_system = native_pty_system();
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };
    let pair = pty_system.openpty(size).map_err(|e| e.to_string())?;

    let cmd = crate::shell_init::build_command(cwd, shell)?;
    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave); // 关键——slave 必须在 master 读取前 drop

    // 若下面的管道 setup 失败，guard 会 kill 已 spawn 的 shell，防泄漏。
    let mut guard = ChildKillGuard::new(child.clone_killer());
    let killer = child.clone_killer();
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(
        pair.master.take_writer().map_err(|e| e.to_string())?,
    ));
    guard.disarm(); // setup 成功，解除 kill guard

    let shell_pid = child.process_id().unwrap_or(0) as u32;
    let exited = Arc::new(AtomicBool::new(false));

    let session = Arc::new(PtySession {
        id,
        shell_pid,
        killer: PMutex::new(killer),
        writer: writer.clone(),
        master: PMutex::new(pair.master),
        exited: exited.clone(),
    });

    let pending: Arc<(Mutex<Vec<u8>>, Condvar)> =
        Arc::new((Mutex::new(Vec::with_capacity(READ_BUF)), Condvar::new()));
    let done = Arc::new(AtomicBool::new(false));

    // 三个回调用 Arc 共享：on_data 被 flusher + waiter 都调用，
    // on_signal 被 reader 循环 + finish() 都调用。Arc 避免多次 move。
    let on_data = Arc::new(on_data);
    let on_signal = Arc::new(on_signal);

    // ── reader 线程 ──
    // 阻塞读 master → agent_detect OSC 解析（on_signal 回调）→ push pending buffer。
    let pending_r = pending.clone();
    let on_signal_reader = on_signal.clone();
    let reader_thread = thread::Builder::new()
        .name("octopus-pty-reader".into())
        .spawn(move || {
            let mut buf = [0u8; READ_BUF];
            let mut agent_detect = AgentDetector::new();
            let mut dropped_bytes: u64 = 0;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        agent_detect.process(&buf[..n], |t| on_signal_reader(t));
                        let (lock, cv) = &*pending_r;
                        let mut g = lock.lock().unwrap();
                        if g.len() + n > MAX_PENDING {
                            dropped_bytes += g.len() as u64;
                            g.clear();
                            g.extend_from_slice(OVERFLOW_NOTICE);
                        }
                        g.extend_from_slice(&buf[..n]);
                        cv.notify_one();
                    }
                    Err(e) => {
                        log::debug!("pty reader ended: {e}");
                        break;
                    }
                }
            }
            // PTY 关闭：若 armed 发 Exited（shell 中途死，没发 133;D 的情况）。
            agent_detect.finish(|t| on_signal_reader(t));
            pending_r.1.notify_one();
            if dropped_bytes > 0 {
                log::warn!(
                    "pty backpressure: dropped {dropped_bytes} bytes (cap {MAX_PENDING})"
                );
            }
        })
        .map_err(|e| format!("spawn reader thread: {e}"))?;

    // ── flusher 线程 ──
    // Condvar 等 pending → 4ms coalesce → on_data(chunk)。
    let pending_f = pending.clone();
    let done_f = done.clone();
    let on_data_flush = on_data.clone();
    thread::Builder::new()
        .name("octopus-pty-flusher".into())
        .spawn(move || {
            let (lock, cv) = &*pending_f;
            loop {
                {
                    let mut g = lock.lock().unwrap();
                    while g.is_empty() {
                        if done_f.load(Ordering::Acquire) {
                            return;
                        }
                        let (next, _) = cv.wait_timeout(g, FLUSH_MAX_IDLE).unwrap();
                        g = next;
                    }
                }
                // 合并短窗口，让突发输入作为一个 chunk 发出。
                thread::sleep(FLUSH_COALESCE);
                let chunk = std::mem::take(&mut *lock.lock().unwrap());
                if chunk.is_empty() {
                    continue;
                }
                on_data_flush(chunk);
            }
        })
        .map_err(|e| format!("spawn flusher thread: {e}"))?;

    // ── waiter 线程 ──
    // child.wait() → 等 reader join（EOF）→ flush tail → on_exit(code)。
    let pending_e = pending;
    let done_e = done;
    let exited_w = exited;
    let on_data_exit = on_data; // 复用 on_data 处理 tail（与 Terax 一致）
    thread::Builder::new()
        .name("octopus-pty-waiter".into())
        .spawn(move || {
            let code = match child.wait() {
                Ok(status) => status.exit_code() as i32,
                Err(e) => {
                    log::warn!("pty child wait failed: {e}");
                    -1
                }
            };
            exited_w.store(true, Ordering::Release);
            // 等 reader 撞 EOF 后再取 pending 快照，最后一行输出不会和 Exit 事件竞争。
            if let Err(e) = reader_thread.join() {
                log::error!("pty reader thread panicked: {e:?}");
            }
            let (lock, cv) = &*pending_e;
            let tail = std::mem::take(&mut *lock.lock().unwrap());
            if !tail.is_empty() {
                on_data_exit(tail);
            }
            done_e.store(true, Ordering::Release);
            cv.notify_all();
            on_exit(code);
        })
        .map_err(|e| format!("spawn waiter thread: {e}"))?;

    log::info!("pty spawned id={id} cols={cols} rows={rows} shell_pid={shell_pid}");
    Ok((session, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::CommandBuilder;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    /// 辅助：spawn 一个一次性命令，等退出 + 收集输出。
    /// 返回 (exit_code, collected_output)。
    fn spawn_collect(cwd: Option<&str>, cmd_str: &str) -> (i32, Vec<u8>) {
        let out: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let exit_code = Arc::new(AtomicI32::new(-999));
        let out_clone = out.clone();
        let exit_clone = exit_code.clone();

        // ⚠️ 必须持有 session 直到命令退出——PtySession::Drop 会 kill 子进程，
        // 若 `let _ =` 立即 drop，waiter 拿到的是被 kill 的退出码而非真实码。
        let _session = spawn_impl_for_test(
            1,
            24,
            80,
            cwd,
            cmd_str,
            move |chunk: Vec<u8>| {
                out_clone.lock().unwrap().extend(chunk);
            },
            move |code: i32| {
                exit_clone.store(code, Ordering::SeqCst);
            },
            |_t: Transition| {},
        )
        .expect("spawn");

        // 等退出（最多 5s）。
        let deadline = Instant::now() + Duration::from_secs(5);
        while exit_code.load(Ordering::SeqCst) == -999 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let code = exit_code.load(Ordering::SeqCst);
        let data = out.lock().unwrap().clone();
        (code, data)
        // _session 在此 drop，命令已退出，kill 是 no-op。
    }

    // 测试用 spawn：直接 CommandBuilder（绕过 shell_init 的交互 shell 设置），
    // 跑 `echo hello` 这类一次性命令。
    #[allow(clippy::too_many_arguments)]
    fn spawn_impl_for_test<O, E, S>(
        id: u32,
        rows: u16,
        cols: u16,
        cwd: Option<&str>,
        cmd_str: &str,
        on_data: O,
        on_exit: E,
        on_signal: S,
    ) -> Result<(Arc<PtySession>, PtySize), String>
    where
        O: Fn(Vec<u8>) + Send + Sync + 'static,
        E: Fn(i32) + Send + 'static,
        S: Fn(Transition) + Send + Sync + 'static,
    {
        let pty_system = native_pty_system();
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system.openpty(size).map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg(cmd_str);
        if let Some(d) = cwd {
            cmd.cwd(d);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut guard = ChildKillGuard::new(child.clone_killer());
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(
            pair.master.take_writer().map_err(|e| e.to_string())?,
        ));
        guard.disarm();
        let shell_pid = child.process_id().unwrap_or(0) as u32;
        let exited = Arc::new(AtomicBool::new(false));
        let session = Arc::new(PtySession {
            id,
            shell_pid,
            killer: PMutex::new(killer),
            writer,
            master: PMutex::new(pair.master),
            exited: exited.clone(),
        });
        let pending: Arc<(Mutex<Vec<u8>>, Condvar)> =
            Arc::new((Mutex::new(Vec::with_capacity(READ_BUF)), Condvar::new()));
        let done = Arc::new(AtomicBool::new(false));
        let on_data = Arc::new(on_data);
        let on_signal = Arc::new(on_signal);
        let pending_r = pending.clone();
        let on_signal_reader = on_signal.clone();
        let reader_thread = thread::Builder::new()
            .name("octopus-pty-reader-test".into())
            .spawn(move || {
                let mut buf = [0u8; READ_BUF];
                let mut agent_detect = AgentDetector::new();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            agent_detect.process(&buf[..n], |t| on_signal_reader(t));
                            let (lock, cv) = &*pending_r;
                            let mut g = lock.lock().unwrap();
                            g.extend_from_slice(&buf[..n]);
                            cv.notify_one();
                        }
                        Err(_) => break,
                    }
                }
                agent_detect.finish(|t| on_signal_reader(t));
                pending_r.1.notify_one();
            })
            .map_err(|e| format!("spawn reader: {e}"))?;
        let pending_f = pending.clone();
        let done_f = done.clone();
        let on_data_flush = on_data.clone();
        thread::Builder::new()
            .name("octopus-pty-flusher-test".into())
            .spawn(move || {
                let (lock, cv) = &*pending_f;
                loop {
                    {
                        let mut g = lock.lock().unwrap();
                        while g.is_empty() {
                            if done_f.load(Ordering::Acquire) {
                                return;
                            }
                            let (next, _) = cv.wait_timeout(g, FLUSH_MAX_IDLE).unwrap();
                            g = next;
                        }
                    }
                    thread::sleep(FLUSH_COALESCE);
                    let chunk = std::mem::take(&mut *lock.lock().unwrap());
                    if chunk.is_empty() {
                        continue;
                    }
                    on_data_flush(chunk);
                }
            })
            .map_err(|e| format!("spawn flusher: {e}"))?;
        let pending_e = pending;
        let done_e = done;
        let exited_w = exited;
        let on_data_exit = on_data;
        thread::Builder::new()
            .name("octopus-pty-waiter-test".into())
            .spawn(move || {
                let code = child
                    .wait()
                    .map(|s| s.exit_code() as i32)
                    .unwrap_or(-1);
                exited_w.store(true, Ordering::Release);
                let _ = reader_thread.join();
                let (lock, cv) = &*pending_e;
                let tail = std::mem::take(&mut *lock.lock().unwrap());
                if !tail.is_empty() {
                    on_data_exit(tail);
                }
                done_e.store(true, Ordering::Release);
                cv.notify_all();
                on_exit(code);
            })
            .map_err(|e| format!("spawn waiter: {e}"))?;
        Ok((session, size))
    }

    use std::sync::atomic::AtomicI32;

    #[test]
    fn spawn_echo_and_read_output() {
        // echo 成功输出 + 显式 exit 0，验证 reader 收到输出 + waiter 收到退出码。
        let (code, data) = spawn_collect(None, "echo hello_pty_test; exit 0");
        let s = String::from_utf8_lossy(&data);
        assert_eq!(code, 0, "exit code should be 0");
        assert!(
            s.contains("hello_pty_test"),
            "output should contain echo output, got: {s:?}"
        );
    }

    #[test]
    fn drop_kills_child_process() {
        let pty_system = native_pty_system();
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system.openpty(size).expect("openpty");
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg("sleep 30");
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);

        let killer = child.clone_killer();
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer().expect("writer")));
        let session = Arc::new(PtySession {
            id: 1,
            shell_pid: child.process_id().unwrap_or(0) as u32,
            killer: PMutex::new(killer),
            writer,
            master: PMutex::new(pair.master),
            exited: Arc::new(AtomicBool::new(false)),
        });

        assert!(
            child.try_wait().unwrap().is_none(),
            "child must be alive before drop"
        );
        drop(session);

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut exited = false;
        while Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                exited = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(exited, "child still running 2s after Session drop");
    }

    #[test]
    fn spawn_exit_code_propagates() {
        // 非零退出码（42）要正确传到 on_exit，验证 waiter 线程的退出码捕获。
        let (code, data) = spawn_collect(None, "echo done; exit 42");
        assert_eq!(code, 42, "non-zero exit code must propagate");
        let s = String::from_utf8_lossy(&data);
        assert!(s.contains("done"), "output should arrive before exit, got: {s:?}");
    }
}
