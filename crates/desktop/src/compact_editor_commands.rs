//! 统一内容查看器命令层（多 tab）：PENDING_TAB 暂存 + 开/取/读文本/关。
//!
//! Tab 类型：clipboard（文本/图片）| transcription（只读）。
//! - open_compact_editor_tab(item_id, source)：写 PENDING_TAB；已存在则 emit，否则建窗
//! - get_pending_compact_tab()：前端 mount take 首个 pending
//! - get_clipboard_item_text(item_id)：读 clipboard_history content
//! - get_transcription_text(id)：读 transcriptions 全文（只读 tab）
//! - get_clipboard_item_type(item_id)：读 item_type（前端据此渲染 textarea 或 ImagePreview）
//! - close_compact_editor：关窗

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::compact_editor_window::{create_compact_editor_window, WINDOW_LABEL};

/// 窗口已存在时，向已 mount 的前端推送「打开/切换到某 tab」事件。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTabPayload {
    pub item_id: i64,
    pub source: String,
}

/// 待打开的首个 tab（含完整数据）。open 时写入，前端 mount take。
/// 合并 itemType + text 到一次返回，消除前端 3 次串行 IPC。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTabFull {
    pub item_id: i64,
    pub source: String,
    pub item_type: String,
    pub text: String,
}

static PENDING_TAB: Mutex<Option<PendingTabFull>> = Mutex::new(None);

fn store_pending_tab(item_id: i64, source: &str) {
    // 读取 DB 获取 itemType + text，一次合并到 pending（前端只需 1 次 IPC）
    let (item_type, text) = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, item_id)
    })
    .ok()
    .flatten()
    .map(|item| (item.item_type.as_str().to_string(), item.content))
    .unwrap_or_else(|| ("text".into(), String::new()));

    *PENDING_TAB.lock() = Some(PendingTabFull {
        item_id,
        source: source.to_string(),
        item_type,
        text,
    });
}

fn take_pending_tab() -> Option<PendingTabFull> {
    PENDING_TAB.lock().take()
}

/// 打开统一查看器并定位到某 tab：
/// 写 PENDING_TAB；窗口已存在则 emit open-tab 推送 + 聚焦，否则建窗。
#[tauri::command]
pub fn open_compact_editor_tab(
    item_id: i64,
    source: Option<String>,
    app_handle: tauri::AppHandle,
) {
    let src = source.unwrap_or_else(|| "clipboard".to_string());
    log::info!("[compact-editor] open_tab item_id={} source={}", item_id, src);
    store_pending_tab(item_id, &src);
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        log::info!("[compact-editor] window exists → emit open-tab");
        let _ = window.emit(
            "compact-editor://open-tab",
            OpenTabPayload { item_id, source: src },
        );
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        log::info!("[compact-editor] window absent → create");
        let pending_data = PENDING_TAB.lock().as_ref().cloned();
        create_compact_editor_window(&app_handle, pending_data.as_ref());
    }
}

/// 前端 mount 时拉取首个 pending tab（含完整数据，take 清空）。
/// 合并了 itemType + text，前端不再需要额外 2 次 IPC。
#[tauri::command]
pub fn get_pending_compact_tab() -> Option<PendingTabFull> {
    take_pending_tab()
}

/// 读取剪贴板条目的文本内容（content）。前端据此新建文本 tab。
#[tauri::command]
pub async fn get_clipboard_item_text(item_id: i64) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, item_id)
    })
    .map_err(|e| e.to_string())?;
    item.map(|i| i.content).ok_or_else(|| "条目不存在".to_string())
}

/// 读取剪贴板条目的类型（text/image/file）。前端据此决定渲染 textarea 还是 ImagePreview。
#[tauri::command]
pub async fn get_clipboard_item_type(item_id: i64) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, item_id)
    })
    .map_err(|e| e.to_string())?;
    item.map(|i| i.item_type.as_str().to_string())
        .ok_or_else(|| "条目不存在".to_string())
}

/// 读取语音识别记录的全文（只读 tab）。
/// 转译记录已合并入 clipboard_history（item_type='voice'），从 content 列读全文。
#[tauri::command]
pub async fn get_transcription_text(id: i64) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, id)
    })
    .map_err(|e| e.to_string())?;
    item.map(|i| i.content)
        .ok_or_else(|| "条目不存在".to_string())
}

/// 关闭统一查看器窗口（触发 Destroyed → macOS 切 Accessory）。
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
    fn pending_tab_store_and_take_roundtrip() {
        let _ = take_pending_tab();
        // store_pending_tab 读 DB（测试环境无 DB，走 fallback "text"/""）
        store_pending_tab(42, "clipboard");
        let got = take_pending_tab().expect("take 应返回 pending");
        assert_eq!(got.item_id, 42);
        assert_eq!(got.source, "clipboard");
        assert!(take_pending_tab().is_none(), "第二次 take 应为空");
    }
}
