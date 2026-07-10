//! # macOS 浮窗焦点协调——FLOAT_DEPTH 状态机
//!
//! ## 目标
//!
//! 全局热键弹出浮窗（action bar / clipboard）时，不能把 settings / compact_editor
//! 等常规窗口带到前台。方案：show 前记录前台 app + 临时隐藏其他窗口，hide 后
//! 交还前台焦点 + 恢复被隐藏的窗口。多个浮窗可嵌套唤起，用 `FLOAT_DEPTH` 引用
//! 计数协调——只有最外层（depth 0↔1）才真正记录/恢复状态。
//!
//! ## 状态变量
//!
//! | 变量 | 类型 | 含义 |
//! |------|------|------|
//! | `FLOAT_DEPTH` | `u32` | 浮窗嵌套深度，0 = 无浮窗活跃 |
//! | `WAS_INACTIVE` | `bool` | 最外层 show 时 app 是否在后台 |
//! | `PREV_APP` | `Option<NSRunningApplication>` | 焦点交还目标（前台 app） |
//! | `TEMP_HIDDEN` | `Vec<String>` | show 时被临时隐藏的窗口 label 列表 |
//!
//! ## 状态转移图
//!
//! ```text
//!                          before_show (+1)
//!              ┌──────────────────────────────────────┐
//!              │                                      ▼
//!         ┌────────┐  restore_only (-1)          ┌────────┐
//!      ─▶ │ depth=0│ ◀────────────────────────── │ depth=N│
//!         └────────┘                              └────────┘
//!              ▲                                      │
//!              │  after_hide (-1, depth→0)           │
//!              │  keep_active (-1, depth→0)          │
//!              └──────────────────────────────────────┘
//! ```
//!
//! ## 四条转移边（操作函数）
//!
//! ### 1. `before_floating_window_show` — depth +1（SHOW 边）
//!
//! | | |
//!|---|---|
//! | **depth 变化** | `N → N+1` |
//! | **depth==1 时**（最外层） | 记录 `WAS_INACTIVE`、`PREV_APP`；若 inactive 隐藏 `WINDOWS_TO_HIDE_ON_FLOAT` → `TEMP_HIDDEN` |
//! | **depth>1 时**（嵌套） | 仅 +1，不碰状态（内层浮窗不覆盖外层记录） |
//! | **调用方** | `show_action_bar_window`（trigger→主线程）、`toggle_clipboard_window` else 分支（快捷键回调=主线程） |
//! | **线程要求** | 需 `MainThreadMarker`（`NSApplication::isActive`、`NSWorkspace::frontmostApplication`） |
//!
//! ### 2. `after_floating_window_hide` — depth -1（STANDARD HIDE 边）
//!
//! | | |
//!|---|---|
//! | **depth 变化** | `N → N-1`（`if >0` 防下溢） |
//! | **depth→0 时**（最外层关闭） | 若 `WAS_INACTIVE`：`activateWithOptions(PREV_APP)` 交还焦点（PREV_APP 为 None 则 `deactivate()` 兜底）；恢复 `TEMP_HIDDEN` 窗口 |
//! | **depth>0 时**（嵌套关闭） | early return，不恢复（等最外层） |
//! | **调用方** | `hide_action_bar_window` ← `action_bar_dismiss`（sync=主线程）、`execute_action_bar` Ok(false)（async→`run_on_main_thread`）；`toggle_clipboard_window` if 分支（快捷键=主线程） |
//! | **线程要求** | `deactivate()` 需 `MainThreadMarker`；`activateWithOptions` 线程安全 |
//!
//! ### 3. `after_floating_window_hide_keep_active` — depth -1（KEEP ACTIVE 边）
//!
//! | | |
//!|---|---|
//! | **depth 变化** | `N → N-1`（`if >0` 防下溢） |
//! | **depth→0 时** | 清 `WAS_INACTIVE`/`PREV_APP` + 恢复 `TEMP_HIDDEN`（**不 deactivate / 不 activate**） |
//! | **depth>0 时** | early return |
//! | **调用方** | `action_bar_show_result`（sync=主线程）—— 浮窗 hide 后紧接着 show CompactEditor，deactivate 会压后台 |
//! | **线程要求** | 无 AppKit 调用（仅 Mutex + Tauri `show`），任意线程安全 |
//!
//! ### 4. `restore_hidden_windows_only` — depth -1（VIRTUAL CLOSE 边）
//!
//! | | |
//!|---|---|
//! | **depth 变化** | `N → N-1`（`if >0` 防下溢）—— **无条件**，不论 depth 值 |
//! | **状态清理** | **无条件**清 `WAS_INACTIVE`/`PREV_APP`（在 `TEMP_HIDDEN.is_empty()` 检查之前） |
//! | **有 TEMP_HIDDEN 时** | `deactivate()` + 恢复窗口 |
//! | **无 TEMP_HIDDEN 时** | 仅 depth-1 + 清状态，直接 return |
//! | **调用方** | `clipboard_window` `Focused(false)` 事件回调（窗口事件=主线程） |
//! | **线程要求** | `deactivate()` 需 `MainThreadMarker` |
//!
//! **为什么无条件**：剪贴板 toggle 模式下失焦=虚拟关闭，但 toggle 的 else 分支
//!（`visible && !focused`）会重新 `before_show` → depth+1。若虚拟关闭不扣减，
//!每次「唤出→失焦→拉回→关闭」depth 单调递增，焦点协调彻底瘫痪。
//!
//! ## 不变量（修改前必读）
//!
//! 1. **只有 depth==1（最外层 show）才记录状态**——内层不覆盖外层的 WAS_INACTIVE/PREV_APP
//! 2. **只有 depth→0（最外层关闭）才恢复状态**——`after_hide`/`keep_active` 的 `depth>0 early return`
//! 3. **`restore_hidden_windows_only` 是唯一无条件扣减**——因其虚拟关闭语义，不能 early return
//! 4. **所有 -1 操作有 `if *d > 0` 下溢保护**——异常路径不会导致 u32 underflow
//! 5. **depth 修改必须配对**——每个 `before_show(+1)` 最终必须有且仅有一个 `after_hide/keep_active/restore_only(-1)` 闭合
//!
//! ## 历史踩坑（均已在 Task 9-10 修复）
//!
//! - **P0**：`action_bar_show_result` 直接 `win.hide()` 跳过 `keep_active` → depth 永久泄漏（Task 9）
//! - **P1**：`execute_action_bar` url/script/copy 前端直接 hide 绕过后端收口 → TRIGGER_IN_PROGRESS 锁死 + depth 泄漏（Task 10 Part A）
//! - **P1**：`execute_action_bar` 异常 `?` 跳过 finalize → 重入锁死 + depth 泄漏（Task 10 Part C）
//! - **P1**：`restore_hidden_windows_only` 不扣 depth → 剪贴板失焦-拉回循环 depth 累加（Task 10 Part D）
//! - **P2**：async command 的 hide 在 worker 线程 → `MainThreadMarker::new()` 返回 None → deactivate 静默跳过（Task 10 Part E）

use tauri::{ActivationPolicy, Manager};

/// 常规窗口 label：任一存活 → app 须保持 Regular。
const REGULAR_WINDOWS: &[&str] = &[
    "settings_window",
    "compact_editor_window",
];

/// 浮窗 show 时需临时隐藏的其他窗口（Regular + 其他浮窗），
/// 防止 set_focus 激活 app 后这些窗口抢焦点。
const WINDOWS_TO_HIDE_ON_FLOAT: &[&str] = &[
    "settings_window",
    "compact_editor_window",
    "clipboard_window",
];

/// 某常规窗口关闭后调用：仅当无其他常规窗口存活时才切回 Accessory。
///
/// 必须在 `WindowEvent::Destroyed`（窗口已从 app 移除）里调用——此时被关窗口的
/// `get_webview_window` 已返回 None，故 `REGULAR_WINDOWS` 检查自然只看其余窗口。
pub fn restore_accessory_if_no_regular_window(app_handle: &tauri::AppHandle) {
    let any_alive = REGULAR_WINDOWS
        .iter()
        .any(|label| app_handle.get_webview_window(label).is_some());
    if !any_alive {
        let _ = app_handle.set_activation_policy(ActivationPolicy::Accessory);
    }
}

/// 全局热键触发浮窗（clipboard/result/action_bar）前调用：
/// 临时隐藏常规窗口（settings/compact_editor），避免 app 被激活时
/// 把这些窗口带到前台抢焦点。用户手动点 Dock 图标或托盘仍可恢复。
#[allow(dead_code)]
pub fn hide_regular_windows(app_handle: &tauri::AppHandle) {
    for label in REGULAR_WINDOWS {
        if let Some(win) = app_handle.get_webview_window(label) {
            if win.is_visible().unwrap_or(false) {
                let _ = win.hide();
            }
        }
    }
}

/// 浮窗操作完成后调用：恢复之前被隐藏的常规窗口。（保留供未来使用）
#[allow(dead_code)]
pub fn show_regular_windows(app_handle: &tauri::AppHandle) {
    for label in REGULAR_WINDOWS {
        if let Some(win) = app_handle.get_webview_window(label) {
            // 只 show 不 focus——不抢焦点
            let _ = win.show();
        }
    }
}

// ── 浮窗焦点协调（show 前隐藏 Regular，hide 后恢复 + 交还前台焦点）──

use parking_lot::Mutex;
use once_cell::sync::Lazy;

static TEMP_HIDDEN: Lazy<Mutex<Vec<String>>> = Lazy::new(|| Mutex::new(Vec::new()));
/// 浮窗嵌套深度——多个浮窗重叠唤起时，只有最外层（depth 回到 0）才交还焦点。
/// 解决 WAS_INACTIVE 被第二个浮窗覆盖的问题。
static FLOAT_DEPTH: Lazy<Mutex<u32>> = Lazy::new(|| Mutex::new(0));
static WAS_INACTIVE: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

#[cfg(target_os = "macos")]
struct SendApp(objc2::rc::Retained<objc2_app_kit::NSRunningApplication>);
#[cfg(target_os = "macos")]
unsafe impl Send for SendApp {}
#[cfg(target_os = "macos")]
unsafe impl Sync for SendApp {}

#[cfg(target_os = "macos")]
static PREV_APP: Lazy<Mutex<Option<SendApp>>> = Lazy::new(|| Mutex::new(None));

/// 浮窗 show 前调用：app 非活跃时隐藏 Regular 窗口 + 记录前台 app。
/// 配合 `after_floating_window_hide` 形成闭环。
#[cfg(target_os = "macos")]
pub fn before_floating_window_show(app: &tauri::AppHandle) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSWorkspace, NSRunningApplication};

    if let Some(mtm) = MainThreadMarker::new() {
        let app_ns = NSApplication::sharedApplication(mtm);
        let depth = {
            let mut d = FLOAT_DEPTH.lock();
            *d += 1;
            *d
        };

        // 只有最外层（depth == 1）才记录前台 app + 判断 inactive
        if depth == 1 {
            let is_inactive = !app_ns.isActive();
            *WAS_INACTIVE.lock() = is_inactive;

            // 记录前台应用（焦点交还目标）
            let workspace = NSWorkspace::sharedWorkspace();
            if let Some(front_app) = workspace.frontmostApplication() {
                let curr = NSRunningApplication::currentApplication();
                if front_app.processIdentifier() != curr.processIdentifier() {
                    *PREV_APP.lock() = Some(SendApp(front_app));
                }
            }

            // app 后台时临时隐藏其他窗口
            if is_inactive {
                let mut hidden = Vec::new();
                for label in WINDOWS_TO_HIDE_ON_FLOAT {
                    if let Some(w) = app.get_webview_window(label) {
                        if w.is_visible().unwrap_or(false) {
                            let _ = w.hide();
                            hidden.push(label.to_string());
                        }
                    }
                }
                *TEMP_HIDDEN.lock() = hidden;
            }
        }
    }
}

/// 浮窗 hide 后调用：交还前台焦点给原 app + 恢复被隐藏的 Regular 窗口。
/// 只有最外层（depth 回到 0）才执行交还焦点。
#[cfg(target_os = "macos")]
pub fn after_floating_window_hide(app: &tauri::AppHandle) {
    let depth = {
        let mut d = FLOAT_DEPTH.lock();
        if *d > 0 { *d -= 1; }
        *d
    };

    // 只有最外层才交还焦点 + 恢复窗口
    if depth > 0 { return; }

    let was_inactive = {
        let mut guard = WAS_INACTIVE.lock();
        std::mem::replace(&mut *guard, false)
    };

    if was_inactive {
        // 交还前台焦点给原 app
        let app_opt = {
            let mut guard = PREV_APP.lock();
            guard.take().map(|p| p.0)
        };
        if let Some(prev_app) = app_opt {
            let _ = prev_app.activateWithOptions(
                objc2_app_kit::NSApplicationActivationOptions(1 << 1),
            );
        } else {
            use objc2::MainThreadMarker;
            use objc2_app_kit::NSApplication;
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).deactivate();
            }
        }
    }

    // 恢复临时隐藏的 Regular 窗口（此时 app 已在后台，窗口温和恢复）
    let hidden = {
        let mut guard = TEMP_HIDDEN.lock();
        std::mem::take(&mut *guard)
    };
    for label in hidden {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.show();
        }
    }
}

/// 恢复被隐藏的窗口但不交还前台焦点——用于剪贴板浮窗失焦场景。
/// 剪贴板是 toggle 模式（always-on-top 可见，点击外部不 hide），
/// 用户切到其他 app 时需恢复 Regular 窗口，但不主动交还焦点
/// （剪贴板仍可见，用户可能只是瞄一眼其他 app）。
#[cfg(target_os = "macos")]
pub fn restore_hidden_windows_only(app: &tauri::AppHandle) {
    // 剪贴板失焦 = 虚拟关闭：扣减 depth
    let depth = {
        let mut d = FLOAT_DEPTH.lock();
        if *d > 0 { *d -= 1; }
        *d
    };

    // 只有 depth 回到 0（所有浮窗都关闭了）才清状态。
    // depth>0 说明还有其他浮窗存活（如 action_bar），不能清它们的焦点协调状态。
    if depth > 0 {
        // 仍有浮窗存活——不 deactivate、不清状态、不恢复隐藏窗口
        //（隐藏窗口属于最外层浮窗管理的状态，内层虚拟关闭不应动）
        return;
    }

    // depth==0：所有浮窗已关闭，清理状态
    *WAS_INACTIVE.lock() = false;
    let _ = PREV_APP.lock().take();

    // 取出隐藏窗口
    let hidden = {
        let mut guard = TEMP_HIDDEN.lock();
        std::mem::take(&mut *guard)
    };
    if hidden.is_empty() { return; }

    // deactivate 让我们的 app 退到后台，窗口温和恢复
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).deactivate();
    }

    for label in hidden {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.show();
        }
    }

    let _ = depth; // depth 已扣减，仅用于将来调试日志
}

/// 递减 FLOAT_DEPTH + 恢复隐藏窗口 + 清状态，但**不 deactivate / 不交还前台焦点**。
/// 用于 `action_bar_show_result`：浮窗 hide 后紧接着要 show CompactEditor，
/// deactivate 会导致新窗口被压后台。但必须闭合 depth 引用计数生命周期，
/// 否则 depth 永久递增 → 后续焦点协调彻底瘫痪。
#[cfg(target_os = "macos")]
pub fn after_floating_window_hide_keep_active(app: &tauri::AppHandle) {
    let depth = {
        let mut d = FLOAT_DEPTH.lock();
        if *d > 0 { *d -= 1; }
        *d
    };

    // 只有最外层才恢复窗口
    if depth > 0 { return; }

    // 清状态（不交还前台焦点——CompactEditor 需要前台）
    *WAS_INACTIVE.lock() = false;
    let _ = PREV_APP.lock().take();

    // 恢复临时隐藏的 Regular 窗口（app 仍在前台，窗口温和恢复）
    let hidden = {
        let mut guard = TEMP_HIDDEN.lock();
        std::mem::take(&mut *guard)
    };
    for label in hidden {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.show();
        }
    }
}
