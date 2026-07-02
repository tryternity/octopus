//! 精简编辑器命令层：编辑会话状态机 + 开/关命令。
//!
//! macOS 原生窗无前端 mount，open 直接 set_text 塞文本到 NSTextView（见
//! compact_editor_native）。会话状态机(requestId/SAVED)供保存/取消/关窗兜底用(Task 6)。
//! 非 macOS 回退 webview：本命令层仍可调用，但初始文本传递已随 PENDING 一并移除
//! （macOS 试水阶段，非 macOS fallback 暂不维护初始文本）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::Manager;

use crate::compact_editor_window::{create_compact_editor_window, WINDOW_LABEL};

// ── 编辑会话状态机（原生窗无前端 mount，后端直接管 requestId/SAVED）──

/// 当前编辑会话的 requestId（单例窗，同时一会话）。
static CURRENT_REQUEST_ID: Mutex<Option<String>> = Mutex::new(None);
/// 本会话是否已显式发 result/cancel（关窗兜底据此决定是否补 cancel）。
static SAVED: AtomicBool = AtomicBool::new(false);

/// 开启/重置一个编辑会话（设 requestId，标记未保存）。
pub fn set_session(request_id: String) {
    *CURRENT_REQUEST_ID.lock().unwrap() = Some(request_id);
    SAVED.store(false, Ordering::Relaxed);
}

/// 当前会话的 requestId（克隆）。
pub fn current_request_id() -> Option<String> {
    CURRENT_REQUEST_ID.lock().unwrap().clone()
}

/// 标记本会话已显式发 result/cancel（保存/取消按钮触发）。
pub fn mark_saved() {
    SAVED.store(true, Ordering::Relaxed);
}

/// 关窗兜底：若未 saved 且有会话，返回 requestId 让调用方补发 cancel，并清空会话。
/// 已 saved 则返回 None（无需补 cancel）。
pub fn take_unsaved_cancel() -> Option<String> {
    let saved = SAVED.load(Ordering::Relaxed);
    let rid = CURRENT_REQUEST_ID.lock().unwrap().take();
    if !saved {
        rid
    } else {
        None
    }
}

/// 打开精简编辑器：设会话；已存在则换文本 + 聚焦，否则建窗 + 塞初始文本。
/// macOS 原生窗走 set_text(直接写 NSTextView)；非 macOS fallback 仅建/聚焦窗
/// (初始文本传递已移除，fallback 暂不支持)。
#[tauri::command]
pub fn open_compact_editor(
    initial_text: String,
    request_id: String,
    app_handle: tauri::AppHandle,
) {
    set_session(request_id);
    // 取窗：macOS 原生 Window(get_window) / 非 macOS webview(get_webview_window)
    let existing = app_handle.get_window(WINDOW_LABEL).or_else(|| {
        app_handle
            .get_webview_window(WINDOW_LABEL)
            .map(|w| w.as_ref().window().clone())
    });
    if let Some(window) = existing {
        #[cfg(target_os = "macos")]
        crate::compact_editor_native::set_text(&app_handle, &initial_text);
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        create_compact_editor_window(&app_handle);
        #[cfg(target_os = "macos")]
        crate::compact_editor_native::set_text(&app_handle, &initial_text);
    }
}

/// 关闭精简编辑器窗口（触发 Destroyed → macOS 切 Accessory）。
#[tauri::command]
pub fn close_compact_editor(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_window(WINDOW_LABEL) {
        let _ = window.close();
    } else if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_state_set_and_clear() {
        let _ = take_unsaved_cancel(); // 清残留

        // 已 saved 的会话 → take 不补 cancel
        set_session("rid-state-1".into());
        assert_eq!(current_request_id().as_deref(), Some("rid-state-1"));
        mark_saved();
        assert!(
            take_unsaved_cancel().is_none(),
            "已 saved，take 应 None（无需补 cancel）"
        );

        // 未 saved + 关窗 → take 返回 rid 补 cancel
        set_session("rid-state-2".into()); // SAVED 重置 false
        assert_eq!(
            take_unsaved_cancel().as_deref(),
            Some("rid-state-2"),
            "未 saved，take 应返回 rid 补 cancel"
        );
        assert!(take_unsaved_cancel().is_none(), "二次 take 应 None");
    }
}
