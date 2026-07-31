// src/result_window.rs

use log::debug;
use std::sync::atomic::{AtomicBool, Ordering};
use parking_lot::Mutex;
use tauri::{Emitter, Manager};

// 窗口物理固定 720×480：setSize/setFrame 在 transparent + decorations(false) 悬浮窗上被
// NSWindow 拒绝（min/max 全放宽到 [100,4000]、720×480 完全在区间内仍 setSize 无效），
// 故放弃运行时改尺寸，改用「CSS 伪装 + 点击穿透」——精简态只渲染顶部 720×116 小条
// （与窗口同宽），下方透明区由轮询线程 setIgnoreCursorEvents 穿透到后方应用；
// 长篇态容器撑满 720×480。
const RESULT_WIDTH: f64 = 720.0;
const RESULT_HEIGHT: f64 = 480.0;
const WINDOW_LABEL: &str = "result_window";

// 精简态小条与窗口同宽（720px），工具栏按钮从左边缘开始排列。
// 轮询线程据此判光标是否在小条内（内→可交互，外→穿透）。
const BAR_W: f64 = 720.0;
const BAR_H: f64 = 116.0;
const BAR_OFFSET_X: f64 = (RESULT_WIDTH - BAR_W) / 2.0;

/// instant 模式底部指示卡高度（穿透 poller 用，Task 3）。
const INSTANT_BAR_H: f64 = 80.0;

static WINDOW_READY: AtomicBool = AtomicBool::new(false);
static PENDING_TEXT: Mutex<Option<String>> = Mutex::new(None);
static SESSION_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
// 精简态=true（顶部小条可点 + 下方透明区穿透）；长篇态=false（整窗 720×480 可交互）。
static RESULT_CLICK_THROUGH: AtomicBool = AtomicBool::new(true);

// ── 窗口管理 ──

/// 创建结果展示窗口（默认隐藏）。
pub fn create_result_window(app: &tauri::AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }

    match crate::ui::window_factory::build_float_window(app, crate::ui::window_factory::FloatWindowSpec {
        label: WINDOW_LABEL,
        url: "result.html",
        title: "Result",
        // 物理尺寸固定 720×480（CSS 伪装方案）：精简/长篇双模式由前端 CSS 切容器尺寸，
        // 不再运行时 setSize（transparent 无边框窗上被 NSWindow 拒绝）。resizable(true) 保留。
        inner_size: (RESULT_WIDTH, RESULT_HEIGHT),
        visible: false,
        resizable: true,
        position: None,
        // macOS：非激活悬浮窗（focused(false)）默认吞掉首次点击——仅用于激活窗口、
        // 不派发给 webview，导致工具栏按钮（✏️ 进入编辑等）首次点击无响应。accept_first_mouse
        // 让首次点击也正常派发，按钮点击可靠（双击进入已弃用，改用 edit_shortcut，见 spec §3.1）。
        focused: Some(false),
        accept_first_mouse: Some(true),
    }) {
        Ok(window) => {
            // 恢复上次位置（不可见时 fallback 到顶部居中）
            crate::ui::window_position::restore_window_position(&window, WINDOW_LABEL, |w| {
                if let Ok(Some(m)) = w.primary_monitor() {
                    let x = (m.size().width as f64 / m.scale_factor() - RESULT_WIDTH) / 2.0;
                    let _ = w.set_position(tauri::Position::Logical(
                        tauri::LogicalPosition::new(x, 80.0),
                    ));
                }
            });

            // 移动结束后保存位置（按屏存：每屏独立记位置，show 时取鼠标所在屏的坐标）
            let win_clone = window.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::Moved(_) = event {
                    crate::ui::window_position::save_current_position_per_display(&win_clone, WINDOW_LABEL);
                }
            });

            // 启动点击穿透轮询（窗口生命周期内常驻；仅 macOS 真实生效）
            start_click_through_poller(app.clone());

            debug!("Result window created");
        }
        Err(e) => debug!("Failed to create result window: {}", e),
    }
}

/// 前端页面就绪命令：初始化 ready 状态，并冲刷可能积压的初始文本
#[tauri::command]
pub fn result_window_ready(app_handle: tauri::AppHandle) {
    WINDOW_READY.store(true, Ordering::Relaxed);
    let pending = PENDING_TEXT.lock().take();
    if let Some(text) = pending {
        show_result(&app_handle, &text);
    }
}

/// 切换 Result 窗口的点击穿透模式（CSS 伪装方案：窗口物理固定 720×480）。
/// - expanded=true（长篇）：整窗可交互，关闭穿透。
/// - expanded=false（精简）：仅顶部 720×116 小条可点（与窗口同宽），下方透明区穿透到后方应用。
/// 精简态的穿透由 start_click_through_poller 按光标位置实时切换。
#[tauri::command]
pub fn set_result_click_through(app: tauri::AppHandle, expanded: bool) {
    // 需穿透 = 精简态（!expanded）
    RESULT_CLICK_THROUGH.store(!expanded, Ordering::Relaxed);
    if expanded {
        // 切到长篇：整窗可交互，立即停止穿透
        if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
            set_result_ignores_mouse(&win, false);
        }
    }
    // 切到精简：交由轮询线程按光标位置决定（下一 tick 生效）
}

/// 启动点击穿透轮询线程（窗口生命周期内常驻）。
///
/// 为什么必须 Rust 轮询、不能用前端 setIgnoreCursorEvents+mousemove：一旦
/// setIgnoreCursorEvents(true)，窗口完全不收鼠标事件（macOS NSWindow 连 tracking area
/// 都禁），前端 mousemove 不再触发 → 无法检测光标重新进入小条 → 重入失效。故读全局鼠标
/// 位置（不依赖窗口收事件），~33ms 轮询判光标是否在小条矩形内，据此切换穿透。
///
/// 跨平台实现：`WebviewWindow::cursor_position()` / `outer_position()` / `scale_factor()`
/// 均为 Tauri 跨平台 API（tao 在 Windows 用 GetCursorPos、Linux X11 用 XQueryPointer、
/// macOS 用 NSEvent mouseLocation）。平台差异封装在 [`set_result_ignores_mouse`]。
///
/// 已知限制：Linux **Wayland** 出于安全策略禁止后台查询全局光标位置，tao 在 Wayland 下
/// 恒返回 (0,0) —— 轮询会判定光标恒在小条外（除非小条恰好在屏幕原点），整窗被设为穿透、
/// 小条内按钮无法点击。这是 Wayland 协议层限制（非焦点窗口不可读全局输入），目前无解，
/// 用户可改用 XWayland 运行以恢复 X11 行为。
///
/// **双频率状态机**（2026-07-17 性能优化）：
/// - **慢检测模式（200ms tick）**：窗口隐藏 / 长篇态（整窗可交互）时使用，仅检查
///   `is_visible` + `RESULT_CLICK_THROUGH` 是否需要升级到高频。原实现 33ms tick 即使
///   窗口隐藏也每 tick 跑 4 次 IPC（get_webview_window + is_visible +
///   cursor_position + outer_position），导致闲置时 ~7% CPU + libpas scavenger
///   持续 spin（每 IPC 涉及 NSWindow 引用 / autoreleasepool / 序列化临时分配）。
/// - **高频跟踪模式（33ms tick）**：仅当窗口可见 + 精简态（click-through 开启）
///   进入，按光标位置实时切换 setIgnoresMouseEvents。鼠标移出小条立即降级回穿透，
///   用户体验无变化（33ms = 30 FPS，超过人手移动感知上限）。
/// - 进入高频模式前重置 interval（`set_at_now`），避免 burst tick 补累积欠债。
pub fn start_click_through_poller(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 慢检测间隔：仅查"是否需要进入高频模式"，200ms 足够（用户展开精简态后 200ms 内
        // 开始响应不影响感知）。窗口隐藏 / 长篇态时 idle 开销 ≈ 0（IPC 仅 is_visible）。
        let mut slow_poll = tokio::time::interval(std::time::Duration::from_millis(200));
        // 高频跟踪间隔：精简态可见时管理穿透切换，33ms = 30 FPS。
        let mut fast_poll = tokio::time::interval(std::time::Duration::from_millis(33));
        let mut cur_ignore = false; // 当前是否正在穿透（ignore mouse events）
        let mut in_fast_mode = false;

        loop {
            if !in_fast_mode {
                // ── 慢检测模式 ──
                slow_poll.tick().await;
                let Some(win) = app.get_webview_window(WINDOW_LABEL) else { continue };
                let visible = win.is_visible().unwrap_or(false);
                let need_through = RESULT_CLICK_THROUGH.load(Ordering::Relaxed);
                if !visible || !need_through {
                    // 不需要穿透：清理残留状态，留在慢模式
                    if cur_ignore {
                        set_result_ignores_mouse(&win, false);
                        cur_ignore = false;
                    }
                    continue;
                }
                // 升级到高频模式：重置 fast_poll 避免补累积 tick
                fast_poll.reset();
                in_fast_mode = true;
            }

            // ── 高频跟踪模式 ──
            fast_poll.tick().await;
            let Some(win) = app.get_webview_window(WINDOW_LABEL) else {
                in_fast_mode = false;
                continue;
            };
            let visible = win.is_visible().unwrap_or(false);
            let need_through = RESULT_CLICK_THROUGH.load(Ordering::Relaxed);
            if !visible || !need_through {
                // 降级条件触发：清理穿透状态 + 回慢模式
                if cur_ignore {
                    set_result_ignores_mouse(&win, false);
                    cur_ignore = false;
                }
                in_fast_mode = false;
                continue;
            }
            // 精简态 + 可见：读全局鼠标位置，判是否在可交互 BAR 矩形内
            // （toggle=顶部小条 / instant=底部指示卡，见下方按模式分支）
            // 统一用物理坐标：cursor_position() 和 outer_position() 都是 PhysicalPosition，
            // 避免多屏不同缩放率下逻辑/物理混合换算错误
            let (mx, my) = match win.cursor_position() {
                Ok(p) => (p.x, p.y),
                Err(_) => continue,
            };
            let (wx, wy) = match win.outer_position() {
                Ok(p) => (p.x as f64, p.y as f64),
                Err(_) => continue,
            };
            // 小条屏幕矩形（物理坐标——BAR 常量是逻辑像素，乘 scale_factor 转物理）
            let sf = win.scale_factor().unwrap_or(1.0);
            // 按模式决定可交互区（顶部 toggle 小条 / 底部 instant 指示卡）
            let (bar_off_x, bar_off_y, bar_h) = if crate::engine::coordinator::INSTANT_MODE
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                // instant：底部指示卡，水平居中（指示卡 400 宽，但可交互区放宽到窗口宽 720 便于点击）
                (BAR_OFFSET_X, RESULT_HEIGHT - INSTANT_BAR_H, INSTANT_BAR_H)
            } else {
                // toggle 精简态：顶部小条
                (BAR_OFFSET_X, 0.0, BAR_H)
            };
            let bx0 = wx + bar_off_x * sf;
            let by0 = wy + bar_off_y * sf;
            let bar_w = BAR_W * sf;
            let bar_h_phys = bar_h * sf;
            let in_bar = mx >= bx0
                && mx <= bx0 + bar_w
                && my >= by0
                && my <= by0 + bar_h_phys;
            let want = !in_bar; // 小条外 → 穿透
            if want != cur_ignore {
                set_result_ignores_mouse(&win, want);
                cur_ignore = want;
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn set_result_ignores_mouse(win: &tauri::WebviewWindow, ignore: bool) {
    // 直调 NSWindow setIgnoresMouseEvents（比 Tauri set_ignore_cursor_events 封装更可靠，
    // 复用 screenshot_commands::set_window_ignores_mouse_events 的做法）。需 run_on_main_thread。
    let win_clone = win.clone();
    let _ = win.run_on_main_thread(move || {
        if let Ok(ptr) = win_clone.ns_window() {
            if !ptr.is_null() {
                let ns_win = unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                ns_win.setIgnoresMouseEvents(ignore);
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn set_result_ignores_mouse(win: &tauri::WebviewWindow, ignore: bool) {
    let _ = win.set_ignore_cursor_events(ignore);
}

/// 显示结果窗口并展示识别文本。
pub fn show_result(app: &tauri::AppHandle, text: &str) {
    let _ = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    // 「判 ready + 写 pending」收进同一把 PENDING_TEXT 锁，与 result_window_ready 的
    // store(true)+take 互斥——消除「load(false) 后、写 pending 前 ready 已 take 走 None」
    // 导致该文本滞留（应用启动首帧文本丢失 / 不弹窗）的 TOCTOU 竞态。
    let need_emit = {
        let mut guard = PENDING_TEXT.lock();
        if WINDOW_READY.load(Ordering::Relaxed) {
            true
        } else {
            *guard = Some(text.to_string());
            false
        }
    };
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        // 多屏跟随：仅在窗口**首次显示**（从不可见到可见）时定位到鼠标所在显示器。
        // 同一会话的后续 show（listening→润色中→最终文本）保持位置不动——避免录音期间
        // 鼠标移到副屏，结束时窗口跳走（spec 2026-07-31 e2e 修复）。
        let was_visible = window.is_visible().unwrap_or(false);
        if !was_visible {
            reposition_to_mouse_monitor(&window);
            // toggle 模式 emit record-mode（仅首次显示时，避免重复 emit）
            let _ = app.emit_to(WINDOW_LABEL, "record-mode", "toggle");
        }
        // 物理窗口无条件 show：冷启动首启 webview 可能尚未 ready（走 pending 分支），此前
        // 不 show、要等 ready 冲刷 → 用户按键后"要说话才出现"。提前 show 让窗口立即可见
        // （macOS 可见窗口的 webview 优先首绘，亦加速 ready）；文本仍等 ready 后由
        // show-result 渲染（#container 默认 opacity:0，提前 show 不产生空窗闪烁）。
        let _ = window.show();
        if need_emit {
            // emit_to 定向——show-result 含完整文本，无需广播到其他窗口
            let _ = app.emit_to(WINDOW_LABEL, "show-result", text);
        }
    }
}

/// instant 模式（PTT/hands-free）show 窗口：emit instant-state + 底部定位 + record-mode。
///
/// 与 [`show_result`] 的区别：
/// - 位置用 [`position_bottom_center`]（窗口贴鼠标所在屏底），而非 [`reposition_to_mouse_monitor`]（顶部居中）。
/// - emit `record-mode: "instant"`（仅在首次显示时），让前端切到 instant UI（底部指示卡）。
/// - emit `instant-state {state, text}` 而非 `show-result`：前端据此渲染录音中/转写中等态。
pub fn show_instant(app: &tauri::AppHandle, state: &str, text: &str) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let was_visible = window.is_visible().unwrap_or(false);
        if !was_visible {
            position_bottom_center(&window);
            let _ = app.emit_to(WINDOW_LABEL, "record-mode", "instant");
        }
        let _ = window.show();
        let _ = app.emit_to(
            WINDOW_LABEL,
            "instant-state",
            serde_json::json!({ "state": state, "text": text }),
        );
    }
}

/// show 前把窗口定位到鼠标所在显示器（spec 2026-07-31 单键三模式 result_window 多屏跟随）。
///
/// 每屏独立存位置（`window_pos.{label}@{display_id}`）：
/// - 鼠标所在屏有保存坐标 → set_position 到该坐标。
/// - 无保存坐标 → 该屏顶部居中（用屏 bounds 算）+ 存下来（下次拖拽会覆盖）。
/// - 无鼠标 / 找不到屏（非 macOS）→ no-op，沿用当前位置。
fn reposition_to_mouse_monitor(window: &tauri::WebviewWindow) {
    use crate::ui::window_position::{
        find_monitor_at_mouse, get_mouse_location, load_window_position_for_display,
        save_window_position_for_display,
    };
    let mouse = get_mouse_location();
    let Some((display_id, ox, oy, w, _h)) = find_monitor_at_mouse(mouse) else {
        // 无鼠标 / 找不到屏：沿用当前位置（兼容非 macOS / 无 CGDisplay 权限）
        return;
    };
    if let Some((x, y)) = load_window_position_for_display(WINDOW_LABEL, display_id) {
        // 该屏有保存坐标 → 用它
        let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
        debug!("[result_window] reposition to saved {}@{}: {},{}", WINDOW_LABEL, display_id, x, y);
    } else {
        // 无保存 → 该屏顶部居中（与 create 时 primary fallback 一致）+ 存
        let x = ox + (w - RESULT_WIDTH) / 2.0;
        let y = oy + 80.0;
        let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
        save_window_position_for_display(WINDOW_LABEL, display_id, x, y);
        debug!("[result_window] reposition to default top-center on display {}: {},{}", display_id, x, y);
    }
}

/// instant 模式定位：窗口底部贴鼠标所在屏底（指示卡在 720×480 透明区底部居中）。
///
/// 与 [`reposition_to_mouse_monitor`] 的区别：toggle 模式（show_result）用顶部居中、
/// 按屏记忆用户拖拽位置；instant 模式（PTT/hands-free）用底部贴底，指示卡始终贴屏底可见。
/// 不按屏存（instant 是临时态，下次进 instant 重新定位）。
///
/// 从 `instant_overlay.rs` 的 `position_bottom_center` 搬入，改用 result_window 的常量
/// （`RESULT_WIDTH`=720 / `RESULT_HEIGHT`=480）。
fn position_bottom_center(win: &tauri::WebviewWindow) {
    let app = win.app_handle();
    let mouse = crate::ui::window_position::get_mouse_location();
    // 窗口底边贴屏底：指示卡（底部）在 720×480 透明区底部居中，留 8px 边距
    const INSTANT_BOTTOM_MARGIN: f64 = 8.0;
    if let Some((_did, ox, oy, w, h)) = crate::ui::window_position::find_monitor_at_mouse(mouse) {
        let x = ox + (w - RESULT_WIDTH) / 2.0;
        // 窗口底边贴屏底：窗口 y = 屏底 - 窗口高(480) - margin
        let y = oy + h - RESULT_HEIGHT - INSTANT_BOTTOM_MARGIN;
        let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
        return;
    }
    // fallback：primary monitor（物理坐标除 scale）
    if let Ok(Some(m)) = app.primary_monitor() {
        let scale = m.scale_factor();
        let pos = m.position();
        let size = m.size();
        let x = (size.width as f64 / scale - RESULT_WIDTH) / 2.0;
        let y = pos.y as f64 / scale
            + (size.height as f64 / scale - RESULT_HEIGHT - INSTANT_BOTTOM_MARGIN);
        let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
    }
}

/// 更新结果窗口文本（流式更新时使用）。
/// insertion=true 表示中间插入态（光标在中间），前端立即渲染、跳过 diverted 300ms 延迟。
/// caret = 光标在扁平文本里的 char 偏移（insertion=true 时前端据此定位闪烁光标，使其跟在最后插入的文字后
/// 右移）；insertion=false 时前端忽略 caret（传 0 即可，光标回末尾）。
/// payload 对象 `{ text, insertion, caret }`，前端 handler 在 Task 7 同步改为读对象。
///
/// **emit_to 定向**（2026-07-17 性能优化）：流式识别期间每 tick 触发，原先 `window.emit`
/// 走全局广播到所有 webview（Emitter::emit 默认实现），每个窗口都反序列化 payload。
/// 改用 emit_to 只发给 result_window，避免无关节点解析大文本。
pub fn update_result(app: &tauri::AppHandle, text: &str, insertion: bool, caret: usize) {
    // 同 show_result：判 ready + 写 pending 进同一锁，消除与 result_window_ready 的竞态。
    let need_emit = {
        let mut guard = PENDING_TEXT.lock();
        if WINDOW_READY.load(Ordering::Relaxed) {
            true
        } else {
            *guard = Some(text.to_string());
            false
        }
    };
    if need_emit {
        if app.get_webview_window(WINDOW_LABEL).is_some() {
            let _ = app.emit_to(
                WINDOW_LABEL,
                "update-result",
                serde_json::json!({ "text": text, "insertion": insertion, "caret": caret }),
            );
        }
    }
}

/// 清空结果窗口内容并隐藏（粘贴完成后调用）。
pub fn clear_result(app: &tauri::AppHandle) {
    *PENDING_TEXT.lock() = None;
    if WINDOW_READY.load(Ordering::Relaxed) {
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = app.emit_to(WINDOW_LABEL, "clear-result", ());
            let window_clone = window.clone();
            let current_session = SESSION_COUNTER.load(Ordering::Relaxed);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                if SESSION_COUNTER.load(Ordering::Relaxed) == current_session {
                    let _ = window_clone.hide();
                }
            });
        }
    }
}

/// 全局编辑快捷键被按下：唤起结果窗（show + set_focus）并通知前端 toggle 编辑态。
///
/// 复用前端 toggleEdit：未编辑则进入编辑（enterEdit 内部已对空文本 return，
/// 全局编辑快捷键被按下：show 结果窗 + set_focus（唤起窗口到前台）。
/// CM6 改造后不再 emit toggle 事件——始终可编辑，窗口聚焦后用户直接输入即可。
pub fn trigger_global_edit(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 注册全局编辑快捷键。与 shortcut::register_shortcut 的区别：handler 调用
/// trigger_global_edit（而非 coordinator.toggle）。set_config 热重载时复用此函数。
pub fn register_edit_global_shortcut(
    app: &tauri::AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_ah, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                trigger_global_edit(&app_handle);
            }
        })
        .map_err(|e| format!("Failed to register shortcut '{}': {}", shortcut_str, e))?;
    debug!("Registered global edit shortcut: {}", shortcut_str);
    Ok(())
}

/// 全局立即润色快捷键被按下：show 结果窗（**不 set_focus**，润色不需窗口聚焦接收键盘）
/// 并通知前端触发 polish_now。前端 polishNow 内部判空（无结果静默）+ polishLoading
/// 门控（幂等）。与 trigger_global_edit 的区别仅在此处不 set_focus。
pub fn trigger_global_polish(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.emit("global-polish-trigger", ());
    }
}

/// 注册全局立即润色快捷键。与 register_edit_global_shortcut 的区别：handler 调
/// trigger_global_polish。set_config 热重载时复用此函数。
pub fn register_polish_global_shortcut(
    app: &tauri::AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_ah, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                trigger_global_polish(&app_handle);
            }
        })
        .map_err(|e| format!("Failed to register shortcut '{}': {}", shortcut_str, e))?;
    debug!("Registered global polish shortcut: {}", shortcut_str);
    Ok(())
}

/// 隐藏结果窗口（不清空内容，不归档）。
pub fn hide_result(app: &tauri::AppHandle) {
    if WINDOW_READY.load(Ordering::Relaxed) {
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            let _ = window.emit("hide-result", ());
            let window_clone = window.clone();
            let current_session = SESSION_COUNTER.load(Ordering::Relaxed);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                if SESSION_COUNTER.load(Ordering::Relaxed) == current_session {
                    let _ = window_clone.hide();
                }
            });
        }
    }
}


