//! 统一内容查看器命令层（多 tab）：PENDING_TAB 暂存 + 开/取/读文本/关。
//!
//! Tab 类型：clipboard（文本/图片）| transcription（只读）。
//! - open_compact_editor_tab(item_id, source)：写 PENDING_TAB；已存在则 emit，否则建窗
//! - get_pending_compact_tab()：前端 mount take 首个 pending
//! - get_clipboard_item_text(item_id)：读 clipboard_history content
//! - get_transcription_text(id)：读 transcriptions 全文（只读 tab）
//! - get_clipboard_item_type(item_id)：读 item_type（前端据此渲染 textarea 或 ImagePreview）
//! - close_compact_editor：关窗

use std::sync::Mutex;
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

/// 待打开的首个 tab。open 时写入，前端 mount take。
#[derive(Clone)]
struct PendingTab {
    item_id: i64,
    source: String,
}

static PENDING_TAB: Mutex<Option<PendingTab>> = Mutex::new(None);

fn store_pending_tab(item_id: i64, source: &str) {
    *PENDING_TAB.lock().unwrap() = Some(PendingTab {
        item_id,
        source: source.to_string(),
    });
}

fn take_pending_tab() -> Option<PendingTab> {
    PENDING_TAB.lock().unwrap().take()
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
        create_compact_editor_window(&app_handle);
    }
}

/// 前端 mount 时拉取首个 pending tab（take 清空）。
#[tauri::command]
pub fn get_pending_compact_tab() -> Option<OpenTabPayload> {
    take_pending_tab().map(|p| OpenTabPayload { item_id: p.item_id, source: p.source })
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
#[tauri::command]
pub async fn get_transcription_text(id: i64) -> Result<String, String> {
    let text = octopus_infra::db::with_db(|conn| {
        conn.query_row(
            "SELECT text FROM transcriptions WHERE id = ?1",
            [&id],
            |r| r.get::<_, String>(0),
        )
        .map_err(|e| anyhow::anyhow!(e))
    })
    .map_err(|e| e.to_string())?;
    Ok(text)
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
        store_pending_tab(42, "clipboard");
        let got = take_pending_tab().expect("take 应返回 pending");
        assert_eq!(got.item_id, 42);
        assert_eq!(got.source, "clipboard");
        assert!(take_pending_tab().is_none(), "第二次 take 应为空");
    }
}
