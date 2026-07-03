//! 精简编辑器命令层（多 tab）：PENDING_TAB 暂存 item_id + 开/取/读文本/关 四个命令。
//!
//! 多 tab 改造：每个 tab 绑定一个 clipboard item_id，取代旧的 request_id 单文本模型。
//! - open_compact_editor_tab(item_id)：写 PENDING_TAB；窗口已存在则 emit open-tab 推送新
//!   item_id + 聚焦，否则建窗
//! - get_pending_compact_tab()：前端 mount take 首个 item_id（建窗后必然读到）
//! - get_clipboard_item_text(item_id)：读条目 content，前端据此新建/刷新 tab
//! - close_compact_editor：关窗
//!
//! PENDING_TAB 模式参考 result_window：open 时「先写 PENDING 再建窗」。编辑器按需创建
//! （非预建隐藏窗），mount 必在 create_window 之后，get 必读到。

use std::sync::Mutex;
use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::compact_editor_window::{create_compact_editor_window, WINDOW_LABEL};

/// 窗口已存在时，向已 mount 的前端推送「打开/切换到某 tab」事件。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTabPayload {
    pub item_id: i64,
}

/// 待打开的首个 item_id。open 时写入，前端 mount take。
static PENDING_TAB: Mutex<Option<i64>> = Mutex::new(None);

fn store_pending_tab(item_id: i64) {
    *PENDING_TAB.lock().unwrap() = Some(item_id);
}

fn take_pending_tab() -> Option<i64> {
    PENDING_TAB.lock().unwrap().take()
}

/// 打开精简编辑器并定位到 item_id 对应的 tab：
/// 写 PENDING_TAB；窗口已存在则 emit open-tab 推送新 item_id + 聚焦，否则建窗。
#[tauri::command]
pub fn open_compact_editor_tab(item_id: i64, app_handle: tauri::AppHandle) {
    store_pending_tab(item_id);
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        // 并发再开：窗口已 mount，PENDING_TAB 已被首次 take，改用事件推送新 item_id。
        let _ = window.emit("compact-editor://open-tab", OpenTabPayload { item_id });
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        create_compact_editor_window(&app_handle);
    }
}

/// 前端 mount 时拉取首个 item_id（take 清空）。
#[tauri::command]
pub fn get_pending_compact_tab() -> Option<i64> {
    take_pending_tab()
}

/// 读取剪贴板条目的文本内容（content）。前端据此新建 tab 或刷新内容。
#[tauri::command]
pub async fn get_clipboard_item_text(item_id: i64) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, item_id)
    })
    .map_err(|e| e.to_string())?;
    item.map(|i| i.content).ok_or_else(|| "条目不存在".to_string())
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
    fn pending_tab_store_and_take_roundtrip() {
        // 清空可能的残留（全局静态，防并行测试污染）。
        let _ = take_pending_tab();
        store_pending_tab(42);
        assert_eq!(take_pending_tab(), Some(42));
        assert!(take_pending_tab().is_none(), "第二次 take 应为空");
    }
}
