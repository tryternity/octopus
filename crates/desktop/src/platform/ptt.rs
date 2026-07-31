//! PTT（Push-to-Talk）按键监听——跨平台 keydown/keyup 全局监听。
//!
//! 用 handy-keys crate（Handy 同款），macOS 底层 CGEventTap 绕过
//! Tauri 插件只发 keydown 的限制。
//!
//! ## 架构
//!
//! ```text
//! ┌─────────────────┐   命令（mpsc）   ┌──────────────────────┐
//! │   主线程        │ ───────────────▶ │   manager 线程       │
//! │                 │                  │                      │
//! │ register_ptt    │                  │ - 持有 HotkeyManager │
//! │ unregister_ptt  │                  │ - try_recv 事件      │
//! └─────────────────┘                  │ - 派发 coordinator   │
//!                                      └──────────────────────┘
//! ```
//!
//! HotkeyManager 单线程持有，命令通过 mpsc channel 传递（同 Handy
//! `handy_keys.rs` 的 manager_thread 模式）。
//!
//! ## 首版语义
//!
//! keydown → `coordinator.toggle()`（开始录音）
//! keyup   → `coordinator.toggle()`（停止录音）
//!
//! PTT 模式下 toggle() 被快速连续调用（keydown + keyup），行为上等同
//! 「按一次开始 + 按一次停止」。后续如需 instant 专属路径（跳过
//! result_window），再新增 Command。

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use handy_keys::{Hotkey, HotkeyEvent, HotkeyManager, HotkeyState};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use crate::engine::coordinator::Coordinator;

/// PTX 热键的固定 binding id（仅一个 PTT 键，写死即可）。
const PTT_BINDING_ID: &str = "ptt";

/// 发往 manager 线程的命令。
enum ManagerCommand {
    /// 注册 PTT 热键；返回注册结果。
    Register {
        hotkey_string: String,
        response: Sender<Result<(), String>>,
    },
    /// 注销当前注册的 PTT 热键。
    Unregister {
        response: Sender<Result<(), String>>,
    },
    /// 关闭 manager 线程。
    Shutdown,
}

/// 全局 PTT 监听状态：到 manager 线程的命令通道 + 线程句柄。
///
/// 用 `Lazy<Mutex<...>>`（同 octopus 现有 static 模式，如
/// `ui/tray.rs`、`platform/activation.rs`）。首次 register 时启动
/// manager 线程；unregister 仅发注销命令，线程常驻等待下一次注册
/// （避免反复 spawn CGEventTap）。进程退出由 channel 断开自然回收。
struct PttState {
    command_sender: Sender<ManagerCommand>,
    thread_handle: Option<JoinHandle<()>>,
}

static PTT_STATE: Lazy<Mutex<Option<PttState>>> = Lazy::new(|| Mutex::new(None));

/// 启动 manager 线程（如未启动），返回命令通道 sender。
///
/// HotkeyManager 必须固定在 manager 线程内（不可跨线程 move），
/// 因此线程闭包内创建并独占持有它。
fn ensure_thread(app: &AppHandle) -> Sender<ManagerCommand> {
    let mut guard = PTT_STATE.lock();
    if let Some(state) = guard.as_ref() {
        return state.command_sender.clone();
    }

    let (cmd_tx, cmd_rx) = mpsc::channel::<ManagerCommand>();
    let app_clone = app.clone();
    let handle = std::thread::Builder::new()
        .name("octopus-ptt".into())
        .spawn(move || manager_thread(cmd_rx, app_clone))
        .expect("[ptt] failed to spawn manager thread");

    *guard = Some(PttState {
        command_sender: cmd_tx.clone(),
        thread_handle: Some(handle),
    });
    log::info!("[ptt] manager thread started");
    cmd_tx
}

/// manager 线程主体：独占 HotkeyManager，循环 try_recv 事件 + 处理命令。
fn manager_thread(cmd_rx: Receiver<ManagerCommand>, app: AppHandle) {
    // 在本线程内创建 HotkeyManager（不可跨线程 move）。
    let manager = match HotkeyManager::new_with_blocking() {
        Ok(m) => m,
        Err(e) => {
            // macOS 未授权输入监控权限会走到这里；首次运行可能静默失败。
            log::warn!(
                "[ptt] failed to create HotkeyManager: {} \
                 (macOS: check Input Monitoring permission)",
                e
            );
            return;
        }
    };

    // 当前已注册的 (binding_id → HotkeyId)。PTT 只有一个热键，仍保留 map
    // 以便将来扩展，并和 Handy 实现保持一致。
    let mut registered: Option<(String, handy_keys::HotkeyId)> = None;

    loop {
        // 1) 非阻塞轮询热键事件（callback 在此线程内同步执行）。
        while let Some(event) = manager.try_recv() {
            handle_hotkey_event(&app, &event, &registered);
        }

        // 2) 非阻塞 + 超时轮询命令，保证事件不会被饿死。
        match cmd_rx.recv_timeout(std::time::Duration::from_millis(10)) {
            Ok(cmd) => match cmd {
                ManagerCommand::Register {
                    hotkey_string,
                    response,
                } => {
                    let result = do_register(
                        &manager,
                        &mut registered,
                        PTT_BINDING_ID,
                        &hotkey_string,
                    );
                    let _ = response.send(result);
                }
                ManagerCommand::Unregister { response } => {
                    let result = do_unregister(&manager, &mut registered);
                    let _ = response.send(result);
                }
                ManagerCommand::Shutdown => {
                    log::info!("[ptt] manager thread shutting down");
                    break;
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // 无命令，继续循环。
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                log::info!("[ptt] command channel disconnected, shutting down");
                break;
            }
        }
    }

    log::info!("[ptt] manager thread stopped");
}

/// 派发热键事件到 coordinator。
///
/// try_recv 在 manager 线程内同步返回，因此可直接获取 coordinator 状态
/// 并调用 toggle()——无需跨线程 emit。
fn handle_hotkey_event(
    app: &AppHandle,
    event: &HotkeyEvent,
    registered: &Option<(String, handy_keys::HotkeyId)>,
) {
    // 只处理我们注册的 PTT 热键。
    let Some((_binding_id, id)) = registered.as_ref() else {
        return;
    };
    if event.id != *id {
        return;
    }

    match app.try_state::<Coordinator>() {
        Some(coordinator) => {
            // 首版语义：keydown/keyup 都调 toggle()。
            //   Pressed  → toggle 开始录音
            //   Released → toggle 停止录音
            match event.state {
                HotkeyState::Pressed => {
                    log::info!("[ptt] keydown → coordinator.toggle() (start)");
                }
                HotkeyState::Released => {
                    log::info!("[ptt] keyup → coordinator.toggle() (stop)");
                }
            }
            coordinator.toggle();
        }
        None => {
            log::error!("[ptt] Coordinator not found in Tauri state");
        }
    }
}

/// 在 manager 线程内执行注册。
fn do_register(
    manager: &HotkeyManager,
    registered: &mut Option<(String, handy_keys::HotkeyId)>,
    binding_id: &str,
    hotkey_string: &str,
) -> Result<(), String> {
    let hotkey: Hotkey = hotkey_string.parse().map_err(|e| {
        format!("[ptt] failed to parse hotkey '{}': {}", hotkey_string, e)
    })?;

    let id = manager
        .register(hotkey)
        .map_err(|e| format!("[ptt] failed to register hotkey: {}", e))?;

    *registered = Some((binding_id.to_string(), id));
    log::info!("[ptt] registered hotkey '{}' → id={:?}", hotkey_string, id);
    Ok(())
}

/// 在 manager 线程内执行注销。
fn do_unregister(
    manager: &HotkeyManager,
    registered: &mut Option<(String, handy_keys::HotkeyId)>,
) -> Result<(), String> {
    if let Some((_binding_id, id)) = registered.take() {
        manager
            .unregister(id)
            .map_err(|e| format!("[ptt] failed to unregister hotkey: {}", e))?;
        log::info!("[ptt] unregistered hotkey id={:?}", id);
    }
    Ok(())
}

/// 注册 PTT 键监听。
///
/// `key` 为 PTT 键名（如 "AltRight" / "ShiftRight" / "ControlRight" /
/// "MetaRight"）。首次调用启动 manager 线程；后续调用复用同一线程，
/// 仅更新注册的热键。
///
/// HotkeyManager 是单线程的，命令通过 mpsc channel 传递给 manager 线程，
/// 避免跨线程 move。
pub fn register_ptt(app: &AppHandle, key: &str) -> Result<(), String> {
    log::info!("[ptt] register_ptt: key={}", key);

    let sender = ensure_thread(app);

    let (tx, rx) = mpsc::channel();
    sender
        .send(ManagerCommand::Register {
            hotkey_string: key.to_string(),
            response: tx,
        })
        .map_err(|_| "[ptt] failed to send register command".to_string())?;

    rx.recv()
        .map_err(|_| "[ptt] failed to receive register response".to_string())?
}

/// 注销 PTT 键监听。
///
/// 仅注销当前热键，manager 线程常驻（等待下一次注册）。若希望彻底关闭
/// 线程，可扩展为发送 Shutdown 命令。
pub fn unregister_ptt(app: &AppHandle) -> Result<(), String> {
    log::info!("[ptt] unregister_ptt");

    let sender = {
        let guard = PTT_STATE.lock();
        match guard.as_ref() {
            Some(state) => state.command_sender.clone(),
            None => {
                // 未启动，无需注销。
                return Ok(());
            }
        }
    };
    // app 仅用于日志/未来扩展，这里取引用即可。
    let _ = app;

    let (tx, rx) = mpsc::channel();
    sender
        .send(ManagerCommand::Unregister { response: tx })
        .map_err(|_| "[ptt] failed to send unregister command".to_string())?;

    rx.recv()
        .map_err(|_| "[ptt] failed to receive unregister response".to_string())?
}
