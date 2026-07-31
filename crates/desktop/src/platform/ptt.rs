//! PTT（Push-to-Talk）按键监听——跨平台 keydown/keyup 全局监听。
//!
//! 用 handy-keys crate（Handy 同款），macOS 底层 CGEventTap 绕过
//! Tauri 插件只发 keydown 的限制。
//!
//! keydown → coordinator InstantStart
//! keyup   → coordinator InstantStop

use tauri::AppHandle;

/// 注册 PTT 键监听。
/// key: PTT 键名（如 "AltRight" / "ShiftRight" / "ControlRight" / "MetaRight"）。
///
/// 在独立线程持有 HotkeyManager（同 Handy handy_keys.rs manager_thread 模式），
/// 命令通过 mpsc channel 传递，避免 HotkeyManager 跨线程问题。
pub fn register_ptt(_app: &AppHandle, _key: &str) -> Result<(), String> {
    // TODO Task 2: 实现 HotkeyManager 线程 + keydown/keyup callback
    log::info!("[ptt] register_ptt: key={} (skeleton)", _key);
    Ok(())
}

/// 注销 PTT 键监听。
pub fn unregister_ptt(_app: &AppHandle) -> Result<(), String> {
    // TODO Task 2: 关闭 HotkeyManager 线程
    log::info!("[ptt] unregister_ptt (skeleton)");
    Ok(())
}
