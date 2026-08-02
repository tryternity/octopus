// crates/pty/src/lib.rs
// octopus 内嵌终端 PTY 后端——参考 Terax pty 模块设计。
// portable-pty 跨平台 PTY + OSC agent 状态感知。

pub mod session;
pub mod agent_detect;
pub mod shell_init;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use parking_lot::RwLock;

pub use session::{spawn, PtySession};
pub use agent_detect::{AgentSignal, AgentDetector, Transition};

/// PTY session 注册表。Tauri State 挂载。
pub struct PtyState {
    pub sessions: RwLock<HashMap<u32, std::sync::Arc<PtySession>>>,
    next_id: AtomicU32,
}

impl PtyState {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            next_id: AtomicU32::new(1),
        }
    }
    pub fn alloc_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// 回收所有已退出（`is_exited()==true`）的 session，返回被移除的 id 列表。
    ///
    /// 兜底前端崩溃/路由切换不调 `pty_close` 的场景——否则 sessions map 残留
    /// `Arc<PtySession>` + 死 PTY fd 直到应用退出。由 desktop 层的 reaper 线程
    /// 周期调用（见 setup.rs `init_pty`）。
    ///
    /// 只 reap `exited==true` 的（waiter 已跑完，reader/flusher 已退或已超时强制退）；
    /// 写锁互斥，不与 `pty_close` 竞争。
    pub fn reap_exited(&self) -> Vec<u32> {
        let dead: Vec<u32> = {
            let sessions = self.sessions.read();
            sessions
                .iter()
                .filter(|(_, s)| s.is_exited())
                .map(|(id, _)| *id)
                .collect()
        };
        if !dead.is_empty() {
            let mut sessions = self.sessions.write();
            for id in &dead {
                sessions.remove(id);
            }
        }
        dead
    }
}

impl Default for PtyState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    /// 构造一个仅用于 reap 测试的 PtySession（exited 可控，其余字段最小化）。
    /// killer/master/writer 不可真正使用，但 reap_exited 只读 exited + remove，不触发。
    fn make_session(id: u32, exited: bool) -> Arc<PtySession> {
        // 复用 session.rs 测试的 portable_pty openpty 拿真实 killer/master/writer，
        // 避免构造 mock trait object 的复杂度。用 sh -c true（跨平台通用）。
        let pty_system = portable_pty::native_pty_system();
        let size = portable_pty::PtySize {
            rows: 1,
            cols: 1,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system.openpty(size).expect("openpty");
        let mut cmd = portable_pty::CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "true"]);
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        let killer: Box<dyn portable_pty::ChildKiller + Send> = child.clone_killer();
        let killer = parking_lot::Mutex::new(killer);
        let writer: Arc<std::sync::Mutex<Box<dyn std::io::Write + Send>>> =
            Arc::new(std::sync::Mutex::new(
                pair.master.take_writer().expect("writer"),
            ));
        let session = Arc::new(PtySession {
            id,
            shell_pid: child.process_id().unwrap_or(0) as u32,
            killer,
            writer,
            master: parking_lot::Mutex::new(pair.master),
            exited: Arc::new(AtomicBool::new(exited)),
        });
        // 让 child 退出（sh -c true 立即退），避免泄漏
        let _ = child.wait();
        session
    }

    #[test]
    fn reap_exited_removes_only_exited_sessions() {
        let state = PtyState::new();
        let alive = make_session(1, false);
        let dead1 = make_session(2, true);
        let dead2 = make_session(3, true);
        {
            let mut sessions = state.sessions.write();
            sessions.insert(1, alive.clone());
            sessions.insert(2, dead1.clone());
            sessions.insert(3, dead2.clone());
        }
        let removed = state.reap_exited();
        // 被移除的应是 exited=true 的（顺序不确定，sort 比较）
        let mut removed_sorted = removed.clone();
        removed_sorted.sort();
        assert_eq!(removed_sorted, vec![2, 3], "应移除 exited=true 的 session");

        let sessions = state.sessions.read();
        assert!(sessions.contains_key(&1), "exited=false 应保留");
        assert!(!sessions.contains_key(&2), "exited=true 应被移除");
        assert!(!sessions.contains_key(&3), "exited=true 应被移除");
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn reap_exited_empty_when_none_exited() {
        let state = PtyState::new();
        let s = make_session(1, false);
        state.sessions.write().insert(1, s);
        let removed = state.reap_exited();
        assert!(removed.is_empty(), "无 exited session 时返回空");
        assert_eq!(state.sessions.read().len(), 1);
    }
}
