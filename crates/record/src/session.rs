//! RecordSession：录制会话控制器（完整实现在 Task 5）。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Starting,
    Recording,
    Paused,
    Stopping,
}
