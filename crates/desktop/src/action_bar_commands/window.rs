//! 浮窗显示/收口 + 重入 guard（从 action_bar_commands/mod.rs 提取，Task 1.3）。
//!
//! 热键触发 `trigger_action_bar`（detect → 路由 → show）；
//! 显示辅助（mouse/centered/碰撞检测）；
//! 统一收口 `finalize_action_bar`（重置 TRIGGER_IN_PROGRESS）+ 超时保护；
//! 上下文暂存读写（PENDING_CONTEXT）。

use std::sync::atomic::Ordering;
use tauri::AppHandle;
use crate::action_bar_window::{hide_action_bar_window, show_action_bar_window};
// 父模块的共享状态 + 共享类型 + 共享 helper（context.rs 提取前的函数仍挂在 mod.rs）
use super::{
    PENDING_CONTEXT, TRIGGER_IN_PROGRESS, TRIGGER_TIMESTAMP,
    ActionBarContext, detect_selection, log_app_context,
};

#[tauri::command]
pub fn trigger_action_bar(app: AppHandle) {
    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("[action-bar] 仅 macOS 支持此功能");
        let _ = app;
        return;
    }

    let app_clone = app.clone();
    std::thread::spawn(move || {
        if TRIGGER_IN_PROGRESS.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            log::info!("[action-bar] trigger already in progress, skipping");
            return;
        }
        TRIGGER_TIMESTAMP.store(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64,
            Ordering::SeqCst,
        );

        // ── 检测：一次性拿到全部信息，changeCount 只在这里出现 ──
        let sel = detect_selection(&app_clone);

        // ── 路由：仅依赖 Selection，不再碰检测细节 ──
        // 注意：show_action_bar_* 投递 show 闭包到主线程后立即返回（不 await）。
        // finalize 在 match 后统一调用——guard 防的是「detect+gather 期间重入」，
        // show 投递后 guard 可清（toggle 兜底 + reset_trigger_guard_if_stale 兜底仍在）。
        match &sel {
            crate::action_bar_commands::Selection::None => {
                *PENDING_CONTEXT.lock() = None;
                show_action_bar_centered(&app_clone);
            }
            crate::action_bar_commands::Selection::Text { text, mouse } => {
                // 先同步采集上下文再 show——gather 会调用前台 app（Sublime 的 `subl --command` /
                // Browser 的 osascript），这些调用激活前台 app、在 show 之后抢走 ActionBar 焦点。
                // 对照实验铁证：无选中（不 gather）→ 正常获焦；有选中（gather）→ 失焦。
                // 移到 show 之前，让随后的 show + set_focus 统一夺回焦点（最后 set_focus 者持有）。
                // 附带收益：show 前前台确定是源 app，frontmost_app() 读到源 app 上下文更准确
                // （原异步方案在 ActionBar 获焦后 frontmost 可能变成 octopus 自己）。
                // 代价：热键到弹出增加 gather 耗时（Sublime ~50-150ms，AX 上限 500ms）。
                let mut ctx = ActionBarContext::text(text.clone());
                match crate::app_context::gather_context(text) {
                    Ok(extra) => {
                        log_app_context(text, &extra);
                        ctx.source = Some(extra.source);
                        ctx.surrounding = extra.surrounding;
                    }
                    Err(e) => log::warn!("[action-bar] context gather 失败（降级到仅 text）: {}", e),
                }
                *PENDING_CONTEXT.lock() = Some(ctx);
                show_action_bar_at_mouse_with_pos(&app_clone, *mouse);
            }
            crate::action_bar_commands::Selection::File { files, .. } => {
                *PENDING_CONTEXT.lock() = Some(ActionBarContext::files(files.clone()));
                show_action_bar_at_mouse_with_pos(&app_clone, sel.mouse());
            }
            crate::action_bar_commands::Selection::Folder { folders, .. } => {
                *PENDING_CONTEXT.lock() = Some(ActionBarContext::files(folders.clone()));
                show_action_bar_at_mouse_with_pos(&app_clone, sel.mouse());
            }
        }
        // 统一收口：所有分支（None/Text/File/Folder）投递 show 后清 guard。
        // 修复 M1——原仅 None 分支调 finalize，Text/File/Folder 漏调，
        // 导致 guard 在用户不操作时最多卡 10s（靠 reset_trigger_guard_if_stale 兜底）。
        finalize_action_bar(&app_clone);
    });
}

/// 在指定鼠标坐标附近显示浮窗（含碰撞检测）。
fn show_action_bar_at_mouse_with_pos(app: &AppHandle, mouse: (f64, f64)) {
    let (mx, my) = mouse;
    // 不截断——副屏在主屏左/上方时坐标可为负值
    let mut win_x = mx - 240.0;
    let win_y = my - 42.0;

    // 碰撞检测：防止浮窗溢出显示器边缘
    const WIN_W: f64 = 480.0;
    if let Some(monitor) = app.available_monitors().ok().and_then(|monitors| {
        monitors.into_iter().find(|m| {
            let scale = m.scale_factor();
            let mon_left = m.position().x as f64 / scale;
            let mon_top = m.position().y as f64 / scale;
            let mon_right = (m.position().x as f64 + m.size().width as f64) / scale;
            let mon_bottom = (m.position().y as f64 + m.size().height as f64) / scale;
            mx >= mon_left && mx < mon_right && my >= mon_top && my < mon_bottom
        })
    }) {
        let scale = monitor.scale_factor();
        let mon_x = monitor.position().x as f64 / scale;
        let mon_w = monitor.size().width as f64 / scale;
        let mon_right = mon_x + mon_w;
        if win_x + WIN_W > mon_right { win_x = mon_right - WIN_W; }
        if win_x < mon_x { win_x = mon_x; }
    }

    log::info!("[action-bar] mouse=({},{}) → win_pos=({},{})", mx, my, win_x, win_y);

    let app_for_show = app.clone();
    let _ = app.run_on_main_thread(move || {
        show_action_bar_window(&app_for_show, win_x, win_y);
    });
}

/// 主屏逻辑坐标矩形 (x, y, w, h)——共享给 primary_monitor_center 与
/// show_action_bar_centered，避免两处重复「primary_monitor + scale 换算」四行
/// （P3-2 DRY）。fallback (0,0,1440,900) 与原 show_action_bar_centered 一致。
fn primary_monitor_logical_rect(app: &AppHandle) -> (f64, f64, f64, f64) {
    match app.primary_monitor().ok().flatten() {
        Some(m) => {
            let scale = m.scale_factor();
            (
                m.position().x as f64 / scale,
                m.position().y as f64 / scale,
                m.size().width as f64 / scale,
                m.size().height as f64 / scale,
            )
        }
        None => (0.0, 0.0, 1440.0, 900.0),
    }
}

/// 主屏占位 mouse 坐标——给 detect_selection 在 mouse 采集失败时用。
///
/// **P3-1 注释精度修正（2026-07-17）**：占位坐标 = (水平中心, 垂直 1/5 位置)，
/// 经下游 `show_action_bar_at_mouse_with_pos` 的 `my - 42` 后，浮窗最终位置比
/// `show_action_bar_centered` 严格 1/5 高 42px（42 是 my-42 的"浮窗在鼠标上方"偏移）。
/// 视觉差异可忽略（900px 屏占 4.7%），仍在上 1/5 区域内，非严格对齐但功能等价。
/// 水平方向经 `mx - 240` 后与 centered 完全一致。
pub(crate) fn primary_monitor_center(app: &AppHandle) -> (f64, f64) {
    let (mon_x, mon_y, mon_w, mon_h) = primary_monitor_logical_rect(app);
    (mon_x + mon_w / 2.0, mon_y + mon_h / 5.0)
}

/// 无选中时在主屏幕居中显示浮窗——水平居中，垂直位于屏幕上 1/5 位置（类似 Alfred/Wox）。
fn show_action_bar_centered(app: &AppHandle) {
    const WIN_W: f64 = 480.0;

    // 强制用主显示器（共享 primary_monitor_logical_rect，P3-2 DRY）
    let (mon_x, mon_y, mon_w, mon_h) = primary_monitor_logical_rect(app);

    let win_x = mon_x + (mon_w - WIN_W) / 2.0;
    // 上 1/5 位置
    let win_y = mon_y + mon_h / 5.0;

    log::info!("[action-bar] centered: monitor=({},{},{},{}) → win_pos=({},{})", mon_x, mon_y, mon_w, mon_h, win_x, win_y);

    let app_for_show = app.clone();
    let _ = app.run_on_main_thread(move || {
        show_action_bar_window(&app_for_show, win_x, win_y);
    });
}

/// action bar 所有出口的统一收口：重置重入 guard。
pub(crate) fn finalize_action_bar(_app: &AppHandle) {
    TRIGGER_IN_PROGRESS.store(false, Ordering::SeqCst);
}

/// 对外暴露的 finalize——供 quick_execute 在 ActionBar 可见时手动收口（发现 3 修复）。
/// quick_execute 走 keep_active hide 路径，绕开了 hide_action_bar_window 内部的 finalize，
/// 必须显式调一次，否则 TRIGGER_IN_PROGRESS 残留卡死后续 trigger。
pub(crate) fn finalize_action_bar_pub(app: &AppHandle) {
    finalize_action_bar(app);
}

/// 写入 PENDING_CONTEXT——供 quick_execute 在执行前刷新上下文（发现 2 修复）。
/// trigger_action_bar 内部各分支自己写，quick_execute 需要单独入口防止读到上次残留。
pub(crate) fn set_pending_context(ctx: ActionBarContext) {
    *PENDING_CONTEXT.lock() = Some(ctx);
}

/// 重置 guard——用于 toggle 隐藏时和超时保护。
pub fn reset_trigger_guard() {
    TRIGGER_IN_PROGRESS.store(false, Ordering::SeqCst);
}

/// 超时保护——如果上次触发超过 timeout_secs 秒仍未 finalize，强制重置。
/// 防 webview 崩溃后 guard 永久卡死。
pub fn reset_trigger_guard_if_stale(timeout_secs: u64) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let ts = TRIGGER_TIMESTAMP.load(Ordering::SeqCst);
    if ts > 0 && now - ts > timeout_secs as i64 {
        log::warn!("[action-bar] trigger guard stale ({}s), force reset", now - ts);
        TRIGGER_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// 前端 mount 时拉取上下文。
#[tauri::command]
pub fn action_bar_get_context() -> Option<ActionBarContext> {
    // 非消耗读取（clone）——防止 mount + show 竞态导致第二次拿到 None
    let ctx = PENDING_CONTEXT.lock().clone();
    log::info!(
        "[action-bar][get_context] {}",
        ctx.as_ref()
            .map(|c| format!("Some(text_len={})", c.text.as_deref().map(|t| t.len()).unwrap_or(0)))
            .unwrap_or_else(|| "None".to_string())
    );
    ctx
}

/// emit show 事件时快照 PENDING_CONTEXT——供前端从事件 payload 直接读 context，
/// 消除 invoke(get_context) 异步延迟导致的首屏竞态（窗口已 show 但 ctx Promise
/// 还在 pending，首屏用陈旧 context state 渲染）。
pub fn snapshot_pending_context() -> Option<ActionBarContext> {
    PENDING_CONTEXT.lock().clone()
}

/// 前端隐藏浮窗时调用。reason 用于诊断 dismiss 触发来源（focus-lost / click-outside / 操作后）。
#[tauri::command]
pub fn action_bar_dismiss(app: AppHandle, reason: Option<String>) {
    log::info!("[action-bar][dismiss] reason={:?}", reason);
    hide_action_bar_window(&app);
    finalize_action_bar(&app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    /// 序列化所有修改 TRIGGER_* 全局静态量的测试，防并行竞态。
    static TRIGGER_TEST_LOCK: Mutex<()> = Mutex::new(());

    // ── TRIGGER_IN_PROGRESS guard 超时保护 ──

    #[test]
    fn test_reset_trigger_guard_clears_flag() {
        let _guard = TRIGGER_TEST_LOCK.lock();
        TRIGGER_IN_PROGRESS.store(true, Ordering::SeqCst);
        reset_trigger_guard();
        assert!(!TRIGGER_IN_PROGRESS.load(Ordering::SeqCst), "reset_trigger_guard 应清除 guard");
    }

    #[test]
    fn test_reset_trigger_guard_if_stale_resets_when_stale() {
        let _guard = TRIGGER_TEST_LOCK.lock();
        TRIGGER_IN_PROGRESS.store(true, Ordering::SeqCst);
        // 设一个 60 秒前的时间戳
        let old = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64 - 60;
        TRIGGER_TIMESTAMP.store(old, Ordering::SeqCst);
        reset_trigger_guard_if_stale(30);
        assert!(!TRIGGER_IN_PROGRESS.load(Ordering::SeqCst), "超过 30s 应被强制重置");
    }

    #[test]
    fn test_reset_trigger_guard_if_stale_keeps_recent() {
        let _guard = TRIGGER_TEST_LOCK.lock();
        TRIGGER_IN_PROGRESS.store(true, Ordering::SeqCst);
        // 设一个 5 秒前的时间戳
        let recent = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64 - 5;
        TRIGGER_TIMESTAMP.store(recent, Ordering::SeqCst);
        reset_trigger_guard_if_stale(30);
        assert!(TRIGGER_IN_PROGRESS.load(Ordering::SeqCst), "5s < 30s 不应被重置");
    }

    #[test]
    fn test_reset_trigger_guard_if_stale_ignores_zero_timestamp() {
        let _guard = TRIGGER_TEST_LOCK.lock();
        TRIGGER_IN_PROGRESS.store(true, Ordering::SeqCst);
        TRIGGER_TIMESTAMP.store(0, Ordering::SeqCst);
        reset_trigger_guard_if_stale(30);
        assert!(TRIGGER_IN_PROGRESS.load(Ordering::SeqCst), "timestamp=0 不应被重置");
    }
}
