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
}

impl Default for PtyState {
    fn default() -> Self {
        Self::new()
    }
}
