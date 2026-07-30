// PTY session——portable-pty 封装。
// 参考 Terax session.rs 设计：3 线程模型（reader/flusher/waiter）。

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use parking_lot::Mutex;

/// 单个 PTY 会话。
pub struct PtySession {
    pub id: u32,
    pub shell_pid: u32,
    pub writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    pub master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    pub killer: Mutex<Box<dyn portable_pty::ChildKiller + Send>>,
    pub exited: Arc<AtomicBool>,
}

impl PtySession {
    /// 写入 PTY stdin（用户输入）。
    pub fn write(&self, data: &[u8]) -> std::io::Result<()> {
        self.writer.lock().write_all(data)
    }

    /// 调整 PTY 尺寸。
    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master.lock().resize(portable_pty::PtySize {
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
        self.exited.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.kill();
    }
}
