//! 截图翻译只读译文浮窗——显示流式翻译结果，不获取键盘焦点。
//!
//! 复用 overlay_window 的最简建窗范式 + result_window 的 ready 机制（防 emit 早于
//! React mount 丢事件）。职责单一：只读展示译文 + 复制 + Esc/外击关闭 + 可拖拽。

use std::sync::atomic::{AtomicBool, Ordering};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::ui::window_factory::{build_float_window, FloatWindowSpec};

pub const WINDOW_LABEL: &str = "translate_window";

const WIN_W: f64 = 400.0;
const WIN_H: f64 = 300.0;

/// 前端 React mount 完成 + listener 注册后置 true。emit 早于 mount 时存 PENDING。
static WINDOW_READY: AtomicBool = AtomicBool::new(false);
/// 未 ready 时暂存最新译文（progress 覆盖，done 终态）。
static PENDING_TEXT: Mutex<Option<(String, bool)>> = Mutex::new(None); // (text, is_done)

/// 创建浮窗（启动期调用，visible=false）。
pub fn create_translate_window(app: &AppHandle) {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return;
    }
    let _ = build_float_window(app, FloatWindowSpec {
        label: WINDOW_LABEL,
        url: "translate.html",
        title: "",
        inner_size: (WIN_W, WIN_H),
        visible: false,
        resizable: true,
        position: None,
        focused: Some(false),           // 不抢键盘焦点（同 result_window）
        accept_first_mouse: Some(true), // 非激活窗首次点击可靠（同 result_window）
    });
}

/// 在鼠标附近 show 窗口 + emit reset 清空上次译文。
pub fn show_at_mouse(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let (win_x, win_y) = match crate::action_bar::action_bar_commands::get_mouse_position(app) {
            Some((mx, my)) => (mx - WIN_W / 2.0, my - WIN_H - 20.0), // 鼠标上方居中
            None => {
                // fallback：主屏中心偏上（同 overlay_window 范式）
                app.primary_monitor()
                    .ok()
                    .flatten()
                    .map(|m| {
                        let scale = m.scale_factor();
                        let pos = m.position();
                        let sz = m.size();
                        ((pos.x as f64 / scale + sz.width as f64 / scale / 2.0) - WIN_W / 2.0,
                         (pos.y as f64 / scale + sz.height as f64 / scale / 3.0) - WIN_H / 2.0)
                    })
                    .unwrap_or((400.0, 300.0))
            }
        };
        // 窗口已可见时，macOS 对 always_on_top+transparent 窗口的 set_position 可能被
        // 窗口管理器忽略（不移动）。先 hide 再 set_position 再 show，强制刷新位置。
        // 首次 show（visible=false）时 hide 是 no-op，不影响。
        let already_visible = win.is_visible().unwrap_or(false);
        if already_visible {
            let _ = win.hide();
        }
        let _ = win.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(win_x, win_y),
        ));
        let _ = win.show();
        // 通知前端清空上次译文（listener 已注册，reset 不参与 ready 机制）。
        // 注意：这里**不**重置 WINDOW_READY——listener 在 React mount 时注册一次，
        // hide≠destroy 不重 mount，reset 事件单独负责清空 UI 文本。对齐 result_window
        // （ready 只在 set_*_ready 命令里 store(true)，从不 reset）。
        let _ = app.emit_to(WINDOW_LABEL, "translate-window://reset", ());
    }
}

/// ready-gated emit progress（供 TranslateEmitTarget::Float 调）。
///
/// 「判 ready + 写 pending」收进同一把 PENDING_TEXT 锁，与 set_translate_window_ready 的
/// store(true)+take 互斥——消除「load(false) 后、写 pending 前 ready 已 take 走 None」
/// 导致该文本滞留（译文丢失）的 TOCTOU 竞态。对齐 result_window.rs:256-264。
pub fn emit_float_progress(app: &AppHandle, text: &str) {
    let need_emit = {
        let mut guard = PENDING_TEXT.lock();
        if WINDOW_READY.load(Ordering::SeqCst) {
            true
        } else {
            *guard = Some((text.to_string(), false));
            false
        }
    };
    if need_emit {
        let _ = app.emit_to(WINDOW_LABEL, "translate-window://progress", text);
    }
}

/// ready-gated emit done（供 TranslateEmitTarget::Float 调）。
///
/// 同 emit_float_progress：判 ready + 写 pending 收进同一锁，防 ready 横插导致最终译文滞留。
pub fn emit_float_done(app: &AppHandle, text: &str) {
    let need_emit = {
        let mut guard = PENDING_TEXT.lock();
        if WINDOW_READY.load(Ordering::SeqCst) {
            true
        } else {
            *guard = Some((text.to_string(), true));
            false
        }
    };
    if need_emit {
        let _ = app.emit_to(WINDOW_LABEL, "translate-window://done", text);
    }
}

/// 前端 mount 完成 + listener 注册后调用：flush pending + 标记 ready。
#[tauri::command]
pub fn set_translate_window_ready(app: AppHandle) {
    WINDOW_READY.store(true, Ordering::SeqCst);
    let pending = PENDING_TEXT.lock().take();
    if let Some((text, is_done)) = pending {
        if is_done {
            let _ = app.emit_to(WINDOW_LABEL, "translate-window://done", &text);
        } else {
            let _ = app.emit_to(WINDOW_LABEL, "translate-window://progress", &text);
        }
    }
}
