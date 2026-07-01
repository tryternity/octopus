//! 图片预览命令层：PENDING 暂存 + 开/取/关三个命令。
//!
//! PENDING 模式镜像 compact_editor_commands：open 时「先写 PENDING 再建窗」，
//! 前端 mount 调 get_pending_image 取走。预览窗按需创建（非预建隐藏窗），
//! mount 必然在 create_window 之后，get 必读到；并发再开改用事件推送。

use std::sync::Mutex;
use tauri::{Emitter, Manager};

use crate::image_preview_window::{create_image_preview_window, WINDOW_LABEL};

/// 跨窗口传递的预览载荷。rename_all=camelCase → 前端拿到 { imageId }。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingImage {
    pub image_id: i64,
}

/// 待预览的图片 id。open 时写入，前端 mount/并发再开时 take 或 load 推送。
static PENDING: Mutex<Option<PendingImage>> = Mutex::new(None);

fn store_pending(image_id: i64) {
    *PENDING.lock().unwrap() = Some(PendingImage { image_id });
}

fn take_pending() -> Option<PendingImage> {
    PENDING.lock().unwrap().take()
}

/// 打开图片预览：写 PENDING；已存在则 emit load 推送新 id + 聚焦，否则建窗。
#[tauri::command]
pub fn open_image_preview(image_id: i64, app_handle: tauri::AppHandle) {
    store_pending(image_id);
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        // 并发再开：窗口已 mount，PENDING 已被首次 take，改用事件推送新 { imageId }。
        let _ = window.emit("image-preview://load", PendingImage { image_id });
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        create_image_preview_window(&app_handle);
    }
}

/// 前端 mount 时拉取（take 清空）。
#[tauri::command]
pub fn get_pending_image() -> Option<PendingImage> {
    take_pending()
}

/// 关闭预览窗口（触发 Destroyed → macOS 切 Accessory）。
#[tauri::command]
pub fn close_image_preview(app_handle: tauri::AppHandle) {
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
        store_pending(42);
        let got = take_pending().expect("take 应返回刚写入的载荷");
        assert_eq!(got.image_id, 42);
        assert!(take_pending().is_none(), "第二次 take 应为空");
    }
}
