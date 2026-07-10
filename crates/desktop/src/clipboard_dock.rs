//! 剪贴板浮窗 dock（吸附收缩）鼠标穿透控制。
//!
//! 方案演进：
//! 1. setIgnoresMouseEvents(true) — 全窗口穿透，细条也收不到事件 ❌
//! 2. 轮询鼠标位置动态切换 — run_on_main_thread 调度延迟导致穿透状态不同步 ❌
//! 3. 当前方案：收缩态不设 ignore，用前端 onDocumentLeave 逻辑控制 ❌
//! 4. 终方案：不穿透——收缩态窗口完全在屏幕边缘只留 8px，其余透明不可见但不穿透。
//!    用户不需要点击透明区域（那里没有可见内容），细条可正常交互。

/// 收缩态：不做任何 NSWindow 操作。
/// 透明区域视觉不可见（无 border/shadow/背景），细条通过 CSS pointer-events 可交互。
/// 不追求鼠标穿透——用户不会去点击一个看不见的区域。
pub fn apply_dock_collapsed(_window: &tauri::WebviewWindow) {}

/// 展开态：不做任何 NSWindow 操作。
pub fn apply_dock_expanded(_window: &tauri::WebviewWindow) {}
