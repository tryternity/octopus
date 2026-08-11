//! PTT（Push-to-Talk）按键监听——单键三模式状态机（spec 2026-07-31）。
//!
//! 用 handy-keys crate（Handy 同款），macOS 底层 CGEventTap 绕过
//! Tauri 插件只发 keydown 的限制。
//!
//! ## 单键三模式
//!
//! 一个键（默认右 Alt / OptRight）通过**按键时长 + 双击检测**区分三种录音模式：
//!
//! | 当前状态 | 短按（<260ms 松开） | 长按（≥260ms 不松） | 双击（260ms 内两次按下） |
//! |---|---|---|---|
//! | **idle** | → hands-free | → PTT（松开识别+粘贴） | → toggle（弹 result_window） |
//! | **toggle 录音中** | → 立即润色 | → 结束 toggle | → 结束 toggle |
//! | **hands-free 录音中** | → 停止 | → 停止 | → 停止 |
//! | **PTT 录音中** | —（按着键呢） | keyup → 停止+粘贴 | — |
//!
//! ## 架构
//!
//! ```text
//! ┌─────────────────┐   命令（mpsc）   ┌──────────────────────┐
//! │   主线程        │ ───────────────▶ │   manager 线程       │
//! │                 │                  │                      │
//! │ register_ptt    │                  │ - 持有 HotkeyManager │
//! │ unregister_ptt  │                  │ - try_recv 事件      │
//! └─────────────────┘                  │ - 驱动 PttFsm 状态机 │
//!                                      │ - 派发 coordinator   │
//!                                      └──────────────────────┘
//! ```
//!
//! HotkeyManager 单线程持有，命令通过 mpsc channel 传递（同 Handy
//! `handy_keys.rs` 的 manager_thread 模式）。PttFsm 状态机也常驻 manager 线程
//! （局部变量），跨事件保持状态；通过 `coordinator::recording_mode()` 读取当前
//! 录音状态（AtomicU8，Relaxed）。

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use handy_keys::{Hotkey, HotkeyEvent, HotkeyManager, HotkeyState};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

use crate::engine::coordinator::{recording_mode, Coordinator};

/// 短按 / 双击判定窗口（毫秒）。
///
/// - keydown 后 < 此值松开 = 短按；≥ 此值不松 = 长按（进入 PTT）。
/// - 短按松开后 < 此值再 keydown = 双击。
///
/// 后续可开放为配置项（spec 不变量 3）。
const TAP_TIMEOUT_MS: u64 = 260;

/// PTX 热键的固定 binding id（仅一个 PTT 键，写死即可）。
const PTT_BINDING_ID: &str = "ptt";

/// 发往 manager 线程的命令。
#[allow(dead_code)]  // Unregister/Shutdown 预留给热重载/清理
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
#[allow(dead_code)]  // thread_handle 预留给将来 join
struct PttState {
    command_sender: Sender<ManagerCommand>,
    thread_handle: Option<JoinHandle<()>>,
}

static PTT_STATE: Lazy<Mutex<Option<PttState>>> = Lazy::new(|| Mutex::new(None));

/// PTT 状态机 6 态（spec 2026-07-31 §PTT 状态机）。
///
/// 状态机常驻 manager 线程（局部变量），通过 `recording_mode()` 读取 coordinator
/// 的当前录音态（AtomicU8，Relaxed 足够——判定窗口 260ms，coordinator 命令处理
/// <<1ms）。
///
/// 注意：FSM 的 `Idle` 指「等待下一次按键序列」，**不等于** coordinator 的
/// `RECORDING_MODE==0`。toggle 录音中用户短按（polish_now）后 FSM 回 Idle，
/// 但 `RECORDING_MODE` 仍为 1（toggle 录音未停）。
#[derive(Debug, Clone, Copy)]
enum PttFsm {
    /// 空闲：等待 keydown 开始新序列。
    Idle,
    /// keydown 后等判定：长按（≥TAP_TIMEOUT 进 PTT）or 短按（<TAP_TIMEOUT 松开进 ShortPressWait）。
    Pending { timer_start: Instant },
    /// 短按松开后等判定：双击（TAP_TIMEOUT 内再 keydown → toggle）or 确认 hands-free（超时）。
    ShortPressWait { timer_start: Instant },
    /// PTT 录音中（长按已确认）。keyup → instant_stop → Idle。
    PttRecording,
    /// toggle 录音中（RECORDING_MODE==1）keydown，等判定：长按结束 or 短按润色。
    ToggleInWait { timer_start: Instant },
    /// hands-free 录音中（RECORDING_MODE==3）按键，等判定：任何结果都 → hands_free_stop。
    HandsFreeInWait { timer_start: Instant },
}

/// FSM 转移动作——纯逻辑，不依赖 Coordinator（便于单测）。
///
/// `on_keydown` / `on_keyup` / `drive_timeouts` 先计算 next state + action，
/// 再由调用方把 action 派发到 Coordinator。这样 FSM 流转可脱离 Coordinator 单测。
#[derive(Debug, PartialEq, Eq)]
enum FsmAction {
    /// 无副作用（仅状态转移）。
    None,
    /// 启动 toggle 录音（双击 idle 触发）。
    ToggleStart,
    /// 结束 toggle 录音（双击 toggle 中 / 长按 toggle 中触发）。
    ToggleEnd,
    /// PTT 开始（长按 idle 触发）。
    InstantStart,
    /// PTT 结束（keyup 触发）。
    InstantStop,
    /// 立即润色（toggle 中单击确认触发，录音继续）。
    PolishNow,
    /// hands-free 开始（短按 idle 触发）。
    HandsFreeStart,
    /// hands-free 停止（hands-free 中任意按键 / 超时触发）。
    HandsFreeStop,
}

impl PttFsm {
    fn new() -> Self {
        Self::Idle
    }

    /// 距 timer_start 是否已超 TAP_TIMEOUT_MS。
    fn timed_out(timer_start: Instant) -> bool {
        timer_start.elapsed() >= Duration::from_millis(TAP_TIMEOUT_MS)
    }

    /// keydown 的纯逻辑：返回 (新状态, 动作)。不调 Coordinator。
    fn next_on_keydown(self, mode: u8) -> (PttFsm, FsmAction) {
        match self {
            PttFsm::Idle => {
                if mode == 1 {
                    (PttFsm::ToggleInWait { timer_start: Instant::now() }, FsmAction::None)
                } else if mode == 3 {
                    (PttFsm::HandsFreeInWait { timer_start: Instant::now() }, FsmAction::None)
                } else {
                    (PttFsm::Pending { timer_start: Instant::now() }, FsmAction::None)
                }
            }
            // 真 idle 短按后双击 → 启动 toggle
            PttFsm::ShortPressWait { .. } => (PttFsm::Idle, FsmAction::ToggleStart),
            // 其他态（重复/异常）：复位到 Idle
            _ => (PttFsm::Idle, FsmAction::None),
        }
    }

    /// keyup 的纯逻辑：返回 (新状态, 动作)。不调 Coordinator。
    fn next_on_keyup(self) -> (PttFsm, FsmAction) {
        match self {
            PttFsm::Pending { timer_start } => {
                if timer_start.elapsed() < Duration::from_millis(TAP_TIMEOUT_MS) {
                    (PttFsm::ShortPressWait { timer_start: Instant::now() }, FsmAction::None)
                } else {
                    // 防御：≥ timeout 才松开应已是 PttRecording，当作 PTT stop
                    (PttFsm::Idle, FsmAction::InstantStop)
                }
            }
            PttFsm::PttRecording => (PttFsm::Idle, FsmAction::InstantStop),
            PttFsm::ToggleInWait { timer_start } => {
                if timer_start.elapsed() < Duration::from_millis(TAP_TIMEOUT_MS) {
                    // 短按（单击）→ 立即润色（polish_now），toggle 录音继续。
                    // 注：toggle 中不识别双击——曾试过 ToggleShortWait 等双击判定，
                    // 但 260ms 窗口下手感不佳（双击难触发）。改为单击润色 + 长按结束。
                    (PttFsm::Idle, FsmAction::PolishNow)
                } else {
                    // 长按：drive_timeouts 已 toggle 结束；keyup 仅复位
                    (PttFsm::Idle, FsmAction::None)
                }
            }
            PttFsm::HandsFreeInWait { .. } => (PttFsm::Idle, FsmAction::HandsFreeStop),
            // Idle / ShortPressWait 收到 keyup（无对应 keydown 或事件丢失）：忽略
            other => (other, FsmAction::None),
        }
    }

    /// 超时的纯逻辑：返回 (新状态, 动作)。不调 Coordinator。
    /// 仅当时态超时才返回转移；否则返回 (原状态, None)。
    fn next_on_timeout(&self) -> (PttFsm, FsmAction) {
        match self {
            PttFsm::Pending { timer_start } if PttFsm::timed_out(*timer_start) => {
                (PttFsm::PttRecording, FsmAction::InstantStart)
            }
            PttFsm::ShortPressWait { timer_start } if PttFsm::timed_out(*timer_start) => {
                (PttFsm::Idle, FsmAction::HandsFreeStart)
            }
            PttFsm::ToggleInWait { timer_start } if PttFsm::timed_out(*timer_start) => {
                (PttFsm::Idle, FsmAction::ToggleEnd)
            }
            PttFsm::HandsFreeInWait { timer_start } if PttFsm::timed_out(*timer_start) => {
                (PttFsm::Idle, FsmAction::HandsFreeStop)
            }
            other => (*other, FsmAction::None),
        }
    }
}

/// manager 线程主体：独占 HotkeyManager + PttFsm，循环 try_recv 事件 + 处理命令。
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
    // PTT 状态机——跨事件保持状态。
    let mut fsm = PttFsm::new();

    loop {
        // 1) 非阻塞轮询热键事件（callback 在此线程内同步执行）。
        while let Some(event) = manager.try_recv() {
            handle_hotkey_event(&app, &event, &registered, &mut fsm);
        }

        // 2) 驱动状态机的超时转移（pending / short_press_wait / *_in_wait）。
        drive_timeouts(&app, &mut fsm);

        // 3) 非阻塞 + 超时轮询命令，保证事件不会被饿死。
        match cmd_rx.recv_timeout(Duration::from_millis(10)) {
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

/// 处理热键事件 → 驱动 PttFsm 状态转移 + 派发 coordinator 命令。
///
/// `try_recv` 在 manager 线程内同步返回，因此可直接获取 coordinator 状态
/// 并调用命令——无需跨线程 emit。
fn handle_hotkey_event(
    app: &AppHandle,
    event: &HotkeyEvent,
    registered: &Option<(String, handy_keys::HotkeyId)>,
    fsm: &mut PttFsm,
) {
    // 只处理我们注册的 PTT 热键。
    let Some((_binding_id, id)) = registered.as_ref() else {
        return;
    };
    if event.id != *id {
        return;
    }

    let Some(coordinator) = app.try_state::<Coordinator>() else {
        log::error!("[ptt] Coordinator not found in Tauri state");
        return;
    };

    let mode = recording_mode();
    log::debug!("[ptt] event={:?} fsm={:?} mode={}", event.state, fsm, mode);

    match event.state {
        HotkeyState::Pressed => on_keydown(&coordinator, fsm, mode),
        HotkeyState::Released => on_keyup(&coordinator, fsm),
    }
}

/// keydown 事件处理：按当前 FSM 态 + RECORDING_MODE 派发。
fn on_keydown(coordinator: &Coordinator, fsm: &mut PttFsm, mode: u8) {
    let prev = std::mem::replace(fsm, PttFsm::Idle);
    let (next, action) = prev.next_on_keydown(mode);
    *fsm = next;
    log::info!("[ptt] keydown mode={} → {:?} action={:?}", mode, fsm, action);
    dispatch_action(coordinator, action);
}

/// keyup 事件处理。
fn on_keyup(coordinator: &Coordinator, fsm: &mut PttFsm) {
    let prev = std::mem::replace(fsm, PttFsm::Idle);
    let (next, action) = prev.next_on_keyup();
    *fsm = next;
    log::info!("[ptt] keyup → {:?} action={:?}", fsm, action);
    dispatch_action(coordinator, action);
}

/// 超时驱动：每个 tick（~10ms）检查各计时态是否超时，触发对应转移。
fn drive_timeouts(app: &AppHandle, fsm: &mut PttFsm) {
    let Some(coordinator) = app.try_state::<Coordinator>() else { return };
    let (next, action) = fsm.next_on_timeout();
    if action != FsmAction::None {
        *fsm = next;
        log::info!("[ptt] timeout → {:?} action={:?}", fsm, action);
        dispatch_action(&coordinator, action);
    }
}

/// 把 FSM 计算出的 action 派发到 Coordinator。
fn dispatch_action(coordinator: &Coordinator, action: FsmAction) {
    match action {
        FsmAction::None => {}
        FsmAction::ToggleStart | FsmAction::ToggleEnd => {
            // toggle() 在 Idle 态开始录音，在活跃态停止——同一个命令，coordinator 据 stage 分流。
            coordinator.toggle();
        }
        FsmAction::InstantStart => coordinator.instant_start(),
        FsmAction::InstantStop => coordinator.instant_stop(),
        FsmAction::PolishNow => coordinator.polish_now(),
        FsmAction::HandsFreeStart => coordinator.hands_free_start(),
        FsmAction::HandsFreeStop => coordinator.hands_free_stop(),
    }
}

/// 启动 manager 线程（如未启动），返回命令通道 sender。
///
/// HotkeyManager 必须固定在 manager 线程内（不可跨线程 move），
/// 因此线程闭包内创建并独占持有它。
///
/// 第二十三轮 P2-d3：spawn 失败不再 .expect panic，改为返 Err——调用方（register_ptt）
/// 返 Err 给 Tauri 命令（用户看到「PTT 启动失败」而非 app crash）。spawn 失败=系统极端
/// 状态，PTT 不可用可接受（其他功能继续），panic 让整个 app 消失对用户更糟。
fn ensure_thread(app: &AppHandle) -> Result<Sender<ManagerCommand>, String> {
    let mut guard = PTT_STATE.lock();
    // 第三十一轮 B1：检测 manager 线程是否已死亡（HotkeyManager 创建失败/panic）。
    // 线程死后 PTT_STATE 保留 stale sender，后续 send() 立即 Disconnected → PTT 永久失效。
    // 检测 is_finished() 后清空 PTT_STATE，让下方重新 spawn。
    if let Some(state) = guard.as_ref() {
        if let Some(ref handle) = state.thread_handle {
            if handle.is_finished() {
                log::warn!("[ptt] manager 线程已死亡，清理 stale state 重新 spawn");
                *guard = None;
            } else {
                return Ok(state.command_sender.clone());
            }
        } else {
            return Ok(state.command_sender.clone());
        }
    }

    let (cmd_tx, cmd_rx) = mpsc::channel::<ManagerCommand>();
    let app_clone = app.clone();
    let handle = std::thread::Builder::new()
        .name("octopus-ptt".into())
        .spawn(move || manager_thread(cmd_rx, app_clone))
        .map_err(|e| {
            log::error!("[ptt] failed to spawn manager thread: {}——PTT 功能将不可用", e);
            format!("PTT manager thread spawn failed: {}", e)
        })?;

    *guard = Some(PttState {
        command_sender: cmd_tx.clone(),
        thread_handle: Some(handle),
    });
    log::info!("[ptt] manager thread started");
    Ok(cmd_tx)
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
/// `key` 为 PTT 键名（如 "OptRight" / "AltRight" / "ShiftRight" / "ControlRight" /
/// "MetaRight"）。首次调用启动 manager 线程；后续调用复用同一线程，
/// 仅更新注册的热键。
///
/// HotkeyManager 是单线程的，命令通过 mpsc channel 传递给 manager 线程，
/// 避免跨线程 move。
pub fn register_ptt(app: &AppHandle, key: &str) -> Result<(), String> {
    log::info!("[ptt] register_ptt: key={}", key);

    let sender = ensure_thread(app)?;

    let (tx, rx) = mpsc::channel();
    sender
        .send(ManagerCommand::Register {
            hotkey_string: key.to_string(),
            response: tx,
        })
        .map_err(|_| "[ptt] failed to send register command".to_string())?;

    // 第三十一轮 B2：recv_timeout 防 manager 卡住冻结主线程（register_ptt 是同步 pub fn，
    // set_config 在 Tauri 主线程调它）。5s 超时——正常 register <100ms，5s 充裕。
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("[ptt] failed to receive register response: {}", e))?
}

/// 注销 PTT 键监听。
///
/// 仅注销当前热键，manager 线程常驻（等待下一次注册）。若希望彻底关闭
/// 线程，可扩展为发送 Shutdown 命令。
#[allow(dead_code)]  // 预留给热重载（asr_shortcut 热重载时注销旧键）
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── PttFsm 初始态 ──

    #[test]
    fn fsm_starts_idle() {
        let fsm = PttFsm::new();
        assert!(matches!(fsm, PttFsm::Idle));
    }

    #[test]
    fn tap_timeout_is_260ms() {
        assert_eq!(TAP_TIMEOUT_MS, 260);
    }

    // ── timed_out 判定 ──

    #[test]
    fn timed_out_true_after_threshold() {
        let start = Instant::now() - Duration::from_millis(TAP_TIMEOUT_MS + 10);
        assert!(PttFsm::timed_out(start));
    }

    #[test]
    fn timed_out_false_before_threshold() {
        let start = Instant::now();
        assert!(!PttFsm::timed_out(start));
    }

    #[test]
    fn timed_out_false_at_zero_elapsed() {
        let start = Instant::now();
        assert!(!PttFsm::timed_out(start));
    }

    // ── TAP_TIMEOUT 边界 ──
    // ≥ TAP_TIMEOUT 触发长按；< TAP_TIMEOUT 为短按。

    #[test]
    fn long_press_threshold_is_inclusive() {
        // elapsed == TAP_TIMEOUT → timed_out = true（≥ 语义）
        let start = Instant::now() - Duration::from_millis(TAP_TIMEOUT_MS);
        assert!(PttFsm::timed_out(start), "elapsed == timeout 应判超时（≥）");
    }

    // ── toggle 录音中按键行为（spec 2026-07-31，e2e 后定稿）──
    // toggle 中不识别双击（曾试过 ToggleShortWait 等双击判定，260ms 窗口手感不佳，
    // 双击难触发）。最终行为：单击 → 立即润色（录音继续）；长按 → 结束录音。

    /// toggle 录音中**短按（单击）** keyup → 立即 PolishNow（润色，录音继续）。
    #[test]
    fn toggle_short_click_polishes_immediately() {
        // toggle 录音中（mode==1）：keydown → ToggleInWait
        let (fsm, a) = PttFsm::Idle.next_on_keydown(1);
        assert!(matches!(fsm, PttFsm::ToggleInWait { .. }));
        assert_eq!(a, FsmAction::None);

        // 短按 keyup（timer_start 刚设，必 < 260ms）→ 立即 PolishNow + 回 Idle
        let (fsm, a) = fsm.next_on_keyup();
        assert!(matches!(fsm, PttFsm::Idle));
        assert_eq!(a, FsmAction::PolishNow, "短按 keyup 应立即润色");
    }

    /// toggle 录音中**长按**（≥260ms 不松）→ ToggleEnd（结束录音）。
    #[test]
    fn toggle_long_press_ends_recording() {
        // toggle 录音中：keydown → ToggleInWait
        let (_fsm, _) = PttFsm::Idle.next_on_keydown(1);
        // 模拟长按超时（≥260ms 未松开）
        let expired = PttFsm::ToggleInWait {
            timer_start: Instant::now() - Duration::from_millis(TAP_TIMEOUT_MS + 10),
        };
        let (fsm, a) = expired.next_on_timeout();
        assert!(matches!(fsm, PttFsm::Idle));
        assert_eq!(a, FsmAction::ToggleEnd, "长按应结束 toggle 录音");
    }

    /// toggle 录音中长按 keyup（超时后松开）→ 无动作（drive_timeouts 已 toggle 结束）。
    #[test]
    fn toggle_long_press_keyup_after_timeout_noop() {
        // 模拟 ToggleInWait 已超时（长按期间 drive_timeouts 已触发 ToggleEnd）
        let expired = PttFsm::ToggleInWait {
            timer_start: Instant::now() - Duration::from_millis(TAP_TIMEOUT_MS + 10),
        };
        let (fsm, a) = expired.next_on_keyup();
        assert!(matches!(fsm, PttFsm::Idle));
        assert_eq!(a, FsmAction::None, "长按超时后 keyup 不应重复发命令");
    }

    // ── 真 idle 双击 → 启动 toggle（对照测试，确认两条双击路径不混淆）──

    #[test]
    fn idle_double_click_starts_toggle() {
        // 真 idle（mode==0）：第一次 keydown → Pending
        let (fsm, _) = PttFsm::Idle.next_on_keydown(0);
        assert!(matches!(fsm, PttFsm::Pending { .. }));
        // 短按 keyup → ShortPressWait
        let (fsm, _) = fsm.next_on_keyup();
        assert!(matches!(fsm, PttFsm::ShortPressWait { .. }));
        // 260ms 内再 keydown = 双击 → ToggleStart（启动 toggle 录音）
        let (fsm, a) = fsm.next_on_keydown(0);
        assert!(matches!(fsm, PttFsm::Idle));
        assert_eq!(a, FsmAction::ToggleStart, "真 idle 双击应启动 toggle");
    }

    // ── FsmAction PartialEq 验证（测试基础设施）──

    #[test]
    fn fsm_action_eq_works() {
        assert_eq!(FsmAction::None, FsmAction::None);
        assert_ne!(FsmAction::ToggleStart, FsmAction::ToggleEnd);
        assert_ne!(FsmAction::PolishNow, FsmAction::ToggleEnd);
    }
}
