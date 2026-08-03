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
//! | **depth 变化** | `N → N-1`（`if >0` 防下溢） |
//! | **depth>0 时** | **直接 return**——不清状态、不 deactivate（有嵌套浮窗存活，如 action_bar） |
//! | **depth==0 时** | 清 `WAS_INACTIVE`/`PREV_APP` + 恢复 `TEMP_HIDDEN` 窗口 + `deactivate()` |
//! | **调用方** | `clipboard_window` `Focused(false)` 事件回调（窗口事件=主线程） |
//! | **线程要求** | `deactivate()` 需 `MainThreadMarker` |
//!
//! **为什么 depth>0 不清状态**：剪贴板失焦时 action_bar 可能仍在前台。
//! 若无条件清 WAS_INACTIVE/PREV_APP，外层浮窗的焦点协调状态丢失 → 后续快捷键失效。
//! 纯逻辑经 `float_depth_decrement_and_is_zero` 提取，单测覆盖 5 场景。
//!
//! **为什么必须扣减 depth**：剪贴板 toggle 模式下失焦=虚拟关闭，但 toggle 的 else 分支
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
/// 固定 label 的窗口用精确匹配；终端窗口是多实例（`terminal_*` 前缀），用前缀匹配。
const REGULAR_WINDOWS: &[&str] = &[
    "settings_window",
    "compact_editor_window",
];
/// 多实例常规窗口的 label 前缀（终端窗口 `terminal_<n>`）。
const REGULAR_WINDOW_PREFIXES: &[&str] = &[
    "terminal_",
];

/// 判断某 label 是否为常规窗口（精确 + 前缀）。
fn is_regular_window(label: &str) -> bool {
    REGULAR_WINDOWS.contains(&label)
        || REGULAR_WINDOW_PREFIXES.iter().any(|p| label.starts_with(p))
}

/// 浮窗 show 时需临时隐藏的其他 Regular 窗口，
/// 防止 set_focus 激活 app 后这些窗口抢焦点。
///
/// 注意：`clipboard_window` **不在**此列表——它是 always_on-top 浮窗，
/// dock 收缩态下一直 visible（8px 细条 + 鼠标穿透），不抢焦点。
/// 如果列入，其他浮窗（action_bar/compact_editor）的 show→hide 周期会
/// 把 dock 态剪贴板拖进 hide→裸 show 循环，导致 DOCK_EXPANDED 状态不一致
/// + setIgnoresMouseEvents 残留（窗口看得见但点不动/拖不动）。
/// 2026-07-24 修复。
const WINDOWS_TO_HIDE_ON_FLOAT: &[&str] = &[
    "settings_window",
    "compact_editor_window",
];
/// 多实例浮窗隐藏窗口的前缀（终端 `terminal_*`）。
const WINDOWS_TO_HIDE_ON_FLOAT_PREFIXES: &[&str] = &[
    "terminal_",
];

fn should_hide_on_float(label: &str) -> bool {
    WINDOWS_TO_HIDE_ON_FLOAT.contains(&label)
        || WINDOWS_TO_HIDE_ON_FLOAT_PREFIXES
            .iter()
            .any(|p| label.starts_with(p))
}

/// action bar 场景专用：激活 app 后，把可见的 Regular 窗口（终端/编辑器/设置）
/// 压回 z-order 后面（orderBack:），保持它们可见但不浮在最前。
///
/// 背景：`activate_self` 的 `NSRunningApplication::activateWithOptions(ActivateAllWindows)`
/// 会把所有窗口带到前台——终端/编辑器原本不是焦点时会被抬上来（用户不期望）。
/// action bar 是 always_on_top 浮窗（floating level），在 Regular 之上；压回 Regular 后，
/// action bar 仍浮在最前 + 持 key window（makeKeyAndOrderFront）。
///
/// 只压回**可见的** Regular 窗口（已隐藏的不动）；不改变 key 状态（action bar 的
/// makeKeyAndOrderFront 会夺取 key）。dismiss action bar 时焦点回原 app，
/// Regular 窗口 z-order 自然恢复（用户点终端/编辑器即抬前）。
#[cfg(target_os = "macos")]
pub fn order_back_regular_windows(app: &tauri::AppHandle) {
    use objc2_app_kit::NSWindow;
    for (label, w) in app.webview_windows() {
        if !should_hide_on_float(&label) {
            continue;
        }
        // 只压回可见的——已隐藏的不动（保持用户原本状态）
        if !w.is_visible().unwrap_or(false) {
            continue;
        }
        if let Ok(ns_ptr) = w.ns_window() {
            if !ns_ptr.is_null() {
                unsafe {
                    let ns_win = &*(ns_ptr as *const NSWindow);
                    // orderBack: 把窗口送到 app 窗口列表后面（保持可见，不抬到最前）。
                    // 不用 orderOut:（会隐藏）——用户要终端保持可见。
                    ns_win.orderBack(None);
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
pub fn order_back_regular_windows(_app: &tauri::AppHandle) {}

/// 某常规窗口关闭后调用：仅当无其他常规窗口存活时才切回 Accessory。
///
/// 必须在 `WindowEvent::Destroyed`（窗口已从 app 移除）里调用——此时被关窗口的
/// `get_webview_window` 已返回 None，故检查自然只看其余窗口。
///
/// 固定 label 用精确查；多实例（终端 `terminal_*`）需遍历所有 webview 窗口做前缀匹配。
pub fn restore_accessory_if_no_regular_window(app_handle: &tauri::AppHandle) {
    // 遍历所有 webview 窗口，任一常规窗口存活则保持 Regular
    let any_alive = app_handle
        .webview_windows()
        .keys()
        .any(|label| is_regular_window(label));
    if !any_alive {
        let _ = app_handle.set_activation_policy(ActivationPolicy::Accessory);
    }
}

/// 全局热键触发浮窗（clipboard/result/action_bar）前调用：
/// 临时隐藏常规窗口（settings/compact_editor/terminal_*），避免 app 被激活时
/// 把这些窗口带到前台抢焦点。用户手动点 Dock 图标或托盘仍可恢复。
#[allow(dead_code)]
pub fn hide_regular_windows(app_handle: &tauri::AppHandle) {
    for (label, win) in app_handle.webview_windows() {
        if !should_hide_on_float(&label) {
            continue;
        }
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        }
    }
}

/// 浮窗操作完成后调用：恢复之前被隐藏的常规窗口。（保留供未来使用）
#[allow(dead_code)]
pub fn show_regular_windows(app_handle: &tauri::AppHandle) {
    for (label, win) in app_handle.webview_windows() {
        if !should_hide_on_float(&label) {
            continue;
        }
        // 只 show 不 focus——不抢焦点
        let _ = win.show();
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

/// 浮窗 show 前调用：app 非活跃时记录前台 app（焦点交还目标），可选隐藏 Regular 窗口。
/// 配合 `after_floating_window_hide` 形成闭环。
///
/// `hide_regular`：app 后台时是否临时隐藏终端等 Regular 窗口。
/// - `true`（默认场景：clipboard/record/vault）：隐藏 Regular 窗口，防止 app 激活时
///   把它们带到前台。
/// - `false`（action bar 场景）：**不隐藏**——终端本来可见就保持可见，本来不可见就
///   保持不可见（用户偏好：action bar 不该有副作用改变其他窗口可见性）。action bar
///   是 always_on_top 浮窗（floating level），视觉层级在终端之上；show 时配合
///   `makeKeyAndOrderFront` 夺 key window，终端虽 order front 但在 action bar 下层
///   且不持 key，不抢焦点。
#[cfg(target_os = "macos")]
pub fn before_floating_window_show(app: &tauri::AppHandle, hide_regular: bool) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSWorkspace, NSRunningApplication};

    if let Some(mtm) = MainThreadMarker::new() {
        let app_ns = NSApplication::sharedApplication(mtm);
        let depth = float_depth_increment();

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

            // app 后台时临时隐藏其他窗口（action bar 场景跳过——保持终端可见性不变）
            if is_inactive && hide_regular {
                let mut hidden = Vec::new();
                for (label, w) in app.webview_windows() {
                    if !should_hide_on_float(&label) {
                        continue;
                    }
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                        hidden.push(label);
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
    if !float_depth_decrement_and_is_zero() { return; }

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
            // NSApplicationActivateAllWindows = 1 << 0。
            // ActivateIgnoringOtherApps (1 << 1) 在 macOS 14+ 已 deprecated 且"will have no effect"
            // （Apple 官方头文件明确标注），项目内 activate_window_by_pid / activate_self 已统一用 1 << 0。
            let _ = prev_app.activateWithOptions(
                objc2_app_kit::NSApplicationActivationOptions(1 << 0),
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
        // 防御：clipboard_window 不应被 restore（浮窗不是 Regular，
        // dock 收缩态一直 visible；裸 show 破坏 DOCK_EXPANDED 状态）。
        if label == "clipboard_window" {
            continue;
        }
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
    // depth>0（仍有浮窗存活）时直接返回——不清状态、不恢复窗口
    if !float_depth_decrement_and_is_zero() { return; }
    float_clear_state();

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
        // 防御：clipboard_window 不应被 restore（浮窗不是 Regular，
        // dock 收缩态一直 visible；裸 show 破坏 DOCK_EXPANDED 状态）。
        if label == "clipboard_window" {
            continue;
        }
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.show();
        }
    }
}

/// 递减 FLOAT_DEPTH + 恢复隐藏窗口 + 清状态，但**不 deactivate / 不交还前台焦点**。
/// 用于 `action_bar_show_result`：浮窗 hide 后紧接着要 show CompactEditor，
/// deactivate 会导致新窗口被压后台。但必须闭合 depth 引用计数生命周期，
/// 否则 depth 永久递增 → 后续焦点协调彻底瘫痪。
#[cfg(target_os = "macos")]
pub fn after_floating_window_hide_keep_active(app: &tauri::AppHandle) {
    if !float_depth_decrement_and_is_zero() { return; }
    float_clear_state();

    // 恢复临时隐藏的 Regular 窗口（app 仍在前台，窗口温和恢复）
    let hidden = {
        let mut guard = TEMP_HIDDEN.lock();
        std::mem::take(&mut *guard)
    };
    for label in hidden {
        // 防御：clipboard_window 不应被 restore（浮窗不是 Regular，
        // dock 收缩态一直 visible；裸 show 破坏 DOCK_EXPANDED 状态）。
        if label == "clipboard_window" {
            continue;
        }
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.show();
        }
    }
}

// ── 纯逻辑（可单测，不依赖 AppHandle / MainThreadMarker）──

/// FLOAT_DEPTH +1，返回新值。
fn float_depth_increment() -> u32 {
    let mut d = FLOAT_DEPTH.lock();
    *d += 1;
    *d
}

/// FLOAT_DEPTH -1（防下溢），返回递减后是否为 0。
fn float_depth_decrement_and_is_zero() -> bool {
    let mut d = FLOAT_DEPTH.lock();
    if *d > 0 { *d -= 1; }
    *d == 0
}

/// 清理焦点协调状态（depth==0 时调用）。
fn float_clear_state() {
    *WAS_INACTIVE.lock() = false;
    let _ = PREV_APP.lock().take();
}

/// Run And Paste silent 模式：按 PID 激活源应用窗口，确保粘贴发到正确目标。
/// 比 osascript 更可靠——直接用 NSRunningApplication API。
/// 返回 true=激活成功，false=PID 未找到或激活失败。
/// 当前 quick_execute 路径不再粘贴——保留供未来 silent 模式复用。
#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn activate_window_by_pid(pid: i32) -> bool {
    use objc2_app_kit::NSWorkspace;

    let workspace = NSWorkspace::sharedWorkspace();
    let apps = workspace.runningApplications();
    for app in apps.iter() {
        if app.processIdentifier() == pid {
            // NSApplicationActivateAllWindows = 1 << 0
            app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(1 << 0));
            log::info!("[activation] activated pid={}", pid);
            return true;
        }
    }
    log::warn!("[activation] pid={} not found in running applications", pid);
    false
}

/// 激活自己（本进程）到前台——给 settings/compact_editor 等常规窗口用。
///
/// macOS 14+ 的 `NSApplication::activate()` 是协作式激活，Apple 文档明说"不保证成功"。
/// 托盘菜单点击场景尤其脆弱：菜单关闭时 macOS 会尝试恢复菜单弹出前的焦点，
/// 若 activate() 恰好在此时执行会被覆盖（"偶尔不激活"的根因）。
///
/// 双保险策略：
/// 1. `NSApplication::activate()` —— 标准路径，app 已是前台时足够
/// 2. `NSRunningApplication::activateWithOptions(ActivateAllWindows)` —— 走 NSWorkspace
///    路径的跨 app 激活 API，比 NSApplication::activate 更可靠（Apple 推荐用于
///    把指定 app 带到前台）。注意 IgnoreOtherApps（1<<1）在 macOS 14+ 已 deprecated
///    且"will have no effect"，故只用 ActivateAllWindows（1<<0）。
///
/// 必须在主线程调用（NSApplication::sharedApplication 要求）。
#[cfg(target_os = "macos")]
pub fn activate_self() {
    use objc2_app_kit::{NSApplication, NSWorkspace};
    use objc2_foundation::MainThreadMarker;

    let mtm = match MainThreadMarker::new() {
        Some(mtm) => mtm,
        None => {
            log::warn!("[activation] activate_self called off main thread, skipping");
            return;
        }
    };

    // ① NSApplication 标准激活
    let app = NSApplication::sharedApplication(mtm);
    app.activate();

    // ② NSRunningApplication 兜底（用自己 PID 走 NSWorkspace 路径）
    let pid = std::process::id() as i32;
    let workspace = NSWorkspace::sharedWorkspace();
    for running in workspace.runningApplications().iter() {
        if running.processIdentifier() == pid {
            running.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(1 << 0));
            break;
        }
    }
    log::info!("[activation] activate_self pid={}", pid);
}

#[cfg(not(target_os = "macos"))]
pub fn activate_self() {}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
pub fn activate_window_by_pid(_pid: i32) -> bool {
    log::warn!("[activation] activate_window_by_pid not supported on this platform");
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        *FLOAT_DEPTH.lock() = 0;
        *WAS_INACTIVE.lock() = false;
        let _ = PREV_APP.lock().take();
        TEMP_HIDDEN.lock().clear();
    }

    /// 单个测试函数覆盖所有场景（static 全局状态，不能并行）。
    #[test]
    fn float_depth_lifecycle() {
        // ── 基础 increment/decrement ──
        reset();
        assert_eq!(float_depth_increment(), 1);
        assert_eq!(float_depth_increment(), 2);
        assert!(!float_depth_decrement_and_is_zero()); // depth=1
        assert!(float_depth_decrement_and_is_zero());  // depth=0

        // ── 下溢保护 ──
        reset();
        assert!(float_depth_decrement_and_is_zero()); // 0→0 不 panic
        assert!(float_depth_decrement_and_is_zero());

        // ── 单浮窗完整周期 ──
        reset();
        float_depth_increment();
        *WAS_INACTIVE.lock() = true;
        assert!(float_depth_decrement_and_is_zero());
        float_clear_state();
        assert!(!*WAS_INACTIVE.lock());

        // ── 嵌套浮窗：内层关闭不清状态 ──
        reset();
        float_depth_increment(); // action_bar show (depth=1)
        *WAS_INACTIVE.lock() = true;
        float_depth_increment(); // clipboard show (depth=2)
        assert!(!float_depth_decrement_and_is_zero()); // clipboard blur → depth=1
        assert!(*WAS_INACTIVE.lock(), "内层关闭不应清外层状态");
        assert!(float_depth_decrement_and_is_zero());  // action_bar close → depth=0
        float_clear_state();
        assert!(!*WAS_INACTIVE.lock());

        // ── 三层嵌套逐层关闭 ──
        reset();
        float_depth_increment(); // 1
        *WAS_INACTIVE.lock() = true;
        float_depth_increment(); // 2
        float_depth_increment(); // 3
        assert!(!float_depth_decrement_and_is_zero()); // 2
        assert!(*WAS_INACTIVE.lock());
        assert!(!float_depth_decrement_and_is_zero()); // 1
        assert!(*WAS_INACTIVE.lock());
        assert!(float_depth_decrement_and_is_zero());  // 0
        float_clear_state();
        assert!(!*WAS_INACTIVE.lock());
    }

    /// is_regular_window：固定 label 精确匹配 + terminal_ 前缀匹配。
    /// 多实例终端窗口（terminal_<n> + terminal_action_agent）必须被识别为常规窗口，
    /// 否则关窗后激活策略不会正确恢复（Accessory/Regular）。
    #[test]
    fn is_regular_window_matches_fixed_and_prefix() {
        // 固定 label
        assert!(is_regular_window("settings_window"));
        assert!(is_regular_window("compact_editor_window"));
        // terminal_ 前缀（多实例 + agent 单例）
        assert!(is_regular_window("terminal_1"));
        assert!(is_regular_window("terminal_42"));
        assert!(is_regular_window("terminal_action_agent"));
        // 非常规窗口
        assert!(!is_regular_window("clipboard_window"));
        assert!(!is_regular_window("action_bar_window"));
        assert!(!is_regular_window("main"));
        assert!(!is_regular_window(""));
        assert!(!is_regular_window("terminal")); // 无下划线后缀，不匹配 terminal_
    }

    /// should_hide_on_float：浮窗 show 时需隐藏的窗口。
    /// clipboard_window 故意不在列表（always-on-top 浮窗，hide 会破坏 dock 状态）。
    #[test]
    fn should_hide_on_float_excludes_clipboard_includes_terminal() {
        // 应隐藏：settings / compact_editor / terminal_*
        assert!(should_hide_on_float("settings_window"));
        assert!(should_hide_on_float("compact_editor_window"));
        assert!(should_hide_on_float("terminal_1"));
        assert!(should_hide_on_float("terminal_action_agent"));
        // 不应隐藏：clipboard（always-on-top，hide 破坏 dock 状态）
        assert!(!should_hide_on_float("clipboard_window"));
        // 不应隐藏：浮窗本身
        assert!(!should_hide_on_float("action_bar_window"));
        assert!(!should_hide_on_float("overlay_window"));
    }
}
