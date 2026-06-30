//! 精简编辑器命令层：PENDING 暂存 + 开/取/关三个命令。
//!
//! PENDING 模式参考 result_window：open 时「先写 PENDING 再建窗」，前端 mount 调
//! get_pending_compact_edit 取走。编辑器是按需创建（非预建隐藏窗），故无需 ready 握手——
//! mount 必然在 create_window 之后，get 必读到。

use std::sync::Mutex;
use tauri::{Emitter, Manager};

use crate::compact_editor_window::{create_compact_editor_window, WINDOW_LABEL};

/// 跨窗口传递的编辑载荷。rename_all=camelCase：事件 payload 与命令返回都给前端 {text, requestId}。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactEditPayload {
    pub text: String,
    pub request_id: String,
}

/// 待载入的初始文本。open 时写入，前端 mount/并发再开时 take 或 load 推送。
static PENDING: Mutex<Option<CompactEditPayload>> = Mutex::new(None);

fn store_pending(text: String, request_id: String) {
    *PENDING.lock().unwrap() = Some(CompactEditPayload { text, request_id });
}

fn take_pending() -> Option<CompactEditPayload> {
    PENDING.lock().unwrap().take()
}

/// 打开精简编辑器：写 PENDING；已存在则 emit load 推送新文本 + 聚焦，否则建窗。
#[tauri::command]
pub fn open_compact_editor(
    initial_text: String,
    request_id: String,
    app_handle: tauri::AppHandle,
) {
    store_pending(initial_text.clone(), request_id.clone());
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        // 并发再开：窗口已 mount，PENDING 已被首次 take，改用事件推送新 {text, requestId}。
        let _ = window.emit(
            "compact-editor://load",
            CompactEditPayload {
                text: initial_text,
                request_id,
            },
        );
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        create_compact_editor_window(&app_handle);
    }
}

/// 前端 mount 时拉取初始文本（take 清空）。
#[tauri::command]
pub fn get_pending_compact_edit() -> Option<CompactEditPayload> {
    take_pending()
}

/// 关闭精简编辑器窗口（触发 Destroyed → macOS 切 Accessory）。
#[tauri::command]
pub fn close_compact_editor(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_store_and_take_roundtrip() {
        // 清空可能的残留（全局静态，防并行测试污染）。
        let _ = take_pending();
        store_pending("你好".into(), "rid-1".into());
        let got = take_pending().expect("take 应返回刚写入的载荷");
        assert_eq!(got.text, "你好");
        assert_eq!(got.request_id, "rid-1");
        assert!(take_pending().is_none(), "第二次 take 应为空");
    }
}
