//! 统一内容查看器命令层（多 tab）：PENDING_TABS 暂存 + 开/取/读文本/关。
//!
//! Tab 类型：clipboard（文本/图片）| transcription（只读）。
//! - open_compact_editor_tab(item_id, source)：单开（前端命令）——转调 open_compact_editor_tabs
//! - open_compact_editor_tabs(items)：批量开——一次 push 全部 + 一次 create/emit，
//!   避免连续单开在「窗口刚 build、React 未 mount」中间态丢失第二个 tab
//! - get_pending_compact_tabs()：前端 mount take 全部 pending（Vec）
//! - get_clipboard_item_text(item_id)：读 clipboard_history content
//! - get_transcription_text(id)：读 transcriptions 全文（只读 tab）
//! - get_clipboard_item_type(item_id)：读 item_type（前端据此渲染 textarea 或 ImagePreview）
//! - close_compact_editor：关窗

use parking_lot::Mutex;
use serde::Serialize;
use crate::core::error_util::e2s;
use tauri::{Emitter, Manager};

use crate::commands::compact_editor_window::{create_compact_editor_window, WINDOW_LABEL};

/// 窗口已存在时，向已 mount 的前端推送「打开/切换到某 tab」事件。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTabPayload {
    pub item_id: i64,
    pub source: String,
}

/// 临时 tab 打开参数（不写 DB）。mode=None 为单栏（现有行为），mode="contrast" 为翻译对照。
///
/// **R1 修复（2026-07-17）**：emit 时直接序列化此结构体（替代手写 json!），故加入
/// item_id / source / is_temp 字段以匹配前端 OpenTabPayload 类型——open_temp_compact_editor
/// 调用时这三项固定（item_id=0, source="temp", is_temp=true），由 open_temp_compact_editor
/// 在 emit 前补齐。
#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TempTabPayload {
    /// item_id 固定 0（temp tab 不写 DB）。emit 时补齐。
    #[serde(default)]
    pub item_id: i64,
    /// source 固定 "temp"。emit 时补齐。
    #[serde(default)]
    pub source: String,
    /// is_temp 固定 true。emit 时补齐。
    #[serde(default)]
    pub is_temp: bool,
    /// 单栏文本（mode=None 时用）
    #[serde(default)]
    pub text: String,
    /// "contrast" | None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// 对照原文（mode=contrast 时用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_text: Option<String>,
    /// 对照译文（mode=contrast 时用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated_text: Option<String>,
    /// 翻译 sessionId（mode=contrast 且通过流式翻译路径时用）。
    ///
    /// 前端 open-tab 据此把 sessionId → tabKey 映射写入 translatingSessionsRef，
    /// 后续 `compact-editor://translate-progress|done` 事件按 sessionId 路由到该 tab。
    /// 2026-07-17 修复发现 1（竞态）+ 8（并发错路由）：前端不再依赖单值 ref 的赋值时序。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate_session_id: Option<String>,
}

/// 待打开的 tab（含完整数据）。open 时写入队列，前端 mount take 全部。
/// 合并 itemType + text 到一次返回，消除前端多次串行 IPC。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTabFull {
    pub item_id: i64,
    pub source: String,
    pub item_type: String,
    pub text: String,
    /// 图片原始宽（仅 image 类型），用于 URL 注入消除布局突变
    pub img_width: u32,
    /// 图片原始高
    pub img_height: u32,
    /// 临时文本（不写 DB，保存按钮灰掉）
    #[serde(default)]
    pub is_temp: bool,
    /// 对照模式（mode=contrast）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// 对照原文
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_text: Option<String>,
    /// 对照译文
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated_text: Option<String>,
    /// 翻译 sessionId（流式 contrast tab 携带，前端据此建 translatingSessionsRef 映射）。
    /// 2026-07-17 修复 R1：原先此结构无此字段，store_pending_temp 漏传 →
    /// 新窗口路径下前端拿不到 sessionId → 翻译事件无法路由 → 永久 loading。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate_session_id: Option<String>,
    /// file source tab 的磁盘路径（保存写回用，仅 source="file"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

/// 待开 tab 队列（支持批量双开）。open 时 push，前端 mount take 全部。
static PENDING_TABS: Mutex<Vec<PendingTabFull>> = Mutex::new(Vec::new());

fn push_pending_tab(item_id: i64, source: &str) {
    // 读取 DB 获取 itemType + text + 图片尺寸，一次合并到 pending（前端只需 1 次 IPC）
    let (item_type, text, img_w, img_h) = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, item_id)
    })
    .ok()
    .flatten()
    .map(|item| {
        let (w, h) = item.meta_info
            .as_ref()
            .and_then(|m| m.w.zip(m.h))
            .unwrap_or((0, 0));
        (item.item_type.as_str().to_string(), item.content, w, h)
    })
    .unwrap_or_else(|| ("text".into(), String::new(), 0, 0));

    PENDING_TABS.lock().push(PendingTabFull {
        item_id,
        source: source.to_string(),
        item_type,
        text,
        img_width: img_w,
        img_height: img_h,
        is_temp: false,
        mode: None,
        original_text: None,
        translated_text: None,
        translate_session_id: None, // 普通 tab（DB 查询）不走翻译，无 sessionId
        file_path: None,
    });
}

/// 存储临时 tab（不查 DB，payload 直接传入）。
/// source 参数保留以兼容调用方语义（pending 队列按 source 路由），item_id/is_temp
/// 在此固定（temp tab 不写 DB）。
pub fn store_pending_temp(payload: TempTabPayload, source: &str) {
    PENDING_TABS.lock().push(PendingTabFull {
        item_id: 0,
        source: source.to_string(),
        item_type: "text".into(),
        text: payload.text,
        img_width: 0,
        img_height: 0,
        is_temp: true,
        mode: payload.mode,
        original_text: payload.original_text,
        translated_text: payload.translated_text,
        translate_session_id: payload.translate_session_id,
        file_path: None,
    });
}

/// 存 pending file tab（窗口首次创建时用）。source="file"，不查 DB，text 直接携带。
pub fn store_pending_file(item_id: i64, text: String, file_path: String) {
    PENDING_TABS.lock().push(PendingTabFull {
        item_id,
        source: "file".into(),
        item_type: "text".into(),
        text,
        file_path: Some(file_path),
        img_width: 0,
        img_height: 0,
        is_temp: false,
        mode: None,
        original_text: None,
        translated_text: None,
        translate_session_id: None,
    });
}

/// 打开 CompactEditor 并定位到一个临时 tab（不写 DB）。
/// payload.mode=None 为单栏（现有行为）；payload.mode="contrast" 为翻译对照（左原文右译文）。
/// 窗口已存在 → emit 推送新 temp tab；窗口不存在 → store_pending_temp + 建窗。
///
/// **R1 修复（2026-07-17）**：窗口已存在路径原先用手写 serde_json::json! emit，
/// 漏掉 translateSessionId 字段 → 前端拿不到 sessionId → 翻译事件无法路由 →
/// 永久 loading。现改为 emit 整个 TempTabPayload（serde rename camelCase 已与
/// 前端 OpenTabPayload 兼容），消除手写 JSON 漂移。
pub fn open_temp_compact_editor(app: &tauri::AppHandle, payload: &TempTabPayload) {
    // 补齐 emit 所需的固定字段——调用方只关心 text/mode/originalText/translatedText/
    // translate_session_id。source/is_temp 固定；item_id 仅在调用方未设（=0）时固定为 0，
    // 保留显式设置的值（prompt 文件查看用 md5 hash 作 item_id 实现去重）。
    let mut emit_payload = payload.clone();
    emit_payload.source = "temp".into();
    emit_payload.is_temp = true;

    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        // 直接 emit TempTabPayload——序列化字段名（camelCase）与前端 OpenTabPayload
        // 类型兼容（itemId/source/isTemp/text/mode/originalText/translatedText/translateSessionId）。
        // 不再手写 json!，避免字段漂移（R1 回归根因）。
        let _ = window.emit("compact-editor://open-tab", emit_payload);
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        store_pending_temp(emit_payload, "temp");
        create_compact_editor_window(app, None);
    }
}

fn take_pending_tabs() -> Vec<PendingTabFull> {
    std::mem::take(&mut *PENDING_TABS.lock())
}

/// 批量打开多个 tab（一次调用）。每个 item push 进 PENDING_TABS；窗口不存在则
/// create（URL 注入首个，前端 mount 时 take 全部），窗口存在则逐个 emit open-tab。
///
/// 一次调用避免连续单开的中间态：第一次单开 `build()` 同步注册窗口 label 后，
/// 第二次单开会命中 `get_webview_window=Some` 走 emit 分支，但此时 WebView/React
/// 尚未 mount → emit 被丢 + `push_pending_tab` 覆盖首个 tab → 第二个 tab 永久丢失
/// （截图 OCR 双开图片+文本 tab 即此 bug）。批量调用只走一次 create/emit，无中间态。
pub fn open_compact_editor_tabs(items: &[(i64, Option<&str>)], app_handle: &tauri::AppHandle) {
    if items.is_empty() {
        return;
    }
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        // PENDING_TABS 是否为空 = React 是否已 mount 并 take 清空过的信号。
        // 非空 = 窗口刚 create、React 还没 mount（首次 push 的还在队列）→ 这次也 push，
        //        让 mount 时一并 take；不 emit（未 mount 时 listener 未注册，emit 会丢）。
        // 空 = React 已 mount → emit 即时推送（并聚焦）。
        // 修复：连续两次 open（如用户快速点两个条目）首次走 else 建窗 + push，第二次
        // 命中 window exists 但 React 未 mount，旧实现 emit 会丢第二个 tab。
        let react_mounted = PENDING_TABS.lock().is_empty();
        if react_mounted {
            log::info!("[compact-editor] window exists & mounted → emit {} open-tab(s)", items.len());
            for (id, src) in items {
                let s = src.unwrap_or("clipboard").to_string();
                let _ = window.emit(
                    "compact-editor://open-tab",
                    OpenTabPayload { item_id: *id, source: s },
                );
            }
        } else {
            log::info!("[compact-editor] window exists but not mounted → push {} tab(s) to pending", items.len());
            for (id, src) in items {
                let s = src.unwrap_or("clipboard");
                push_pending_tab(*id, s);
            }
        }
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        // 窗口不存在：先清空残留（上次建窗后 React 未 mount 就关窗 / 建窗失败会留 stale，
        // 否则下次 first() 返回 stale 污染首屏），再 push 全部 + create。
        // 批量只 build 一次，无「build 后第二次 get_webview_window=Some」中间态。
        log::info!("[compact-editor] window absent → create ({} tabs pending)", items.len());
        let _ = take_pending_tabs();
        for (id, src) in items {
            let s = src.unwrap_or("clipboard");
            log::info!("[compact-editor] open_tab item_id={} source={}", id, s);
            push_pending_tab(*id, s);
        }
        let pending_data = PENDING_TABS.lock().first().cloned();
        create_compact_editor_window(app_handle, pending_data.as_ref());
    }
}

/// 打开统一查看器并定位到某 tab（单开，前端命令）——转调批量版单元素。
#[tauri::command]
pub fn open_compact_editor_tab(
    item_id: i64,
    source: Option<String>,
    app_handle: tauri::AppHandle,
) {
    open_compact_editor_tabs(&[(item_id, source.as_deref())], &app_handle);
}

/// 前端 mount 时拉取全部 pending tab（含完整数据，take 清空）。
/// 合并了 itemType + text，前端不再需要额外 IPC。
#[tauri::command]
pub fn get_pending_compact_tabs() -> Vec<PendingTabFull> {
    take_pending_tabs()
}

/// 读取剪贴板条目的文本内容（content）。前端据此新建文本 tab。
#[tauri::command]
pub async fn get_clipboard_item_text(item_id: i64) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, item_id)
    })
    .map_err(e2s)?;
    item.map(|i| i.content).ok_or_else(|| "条目不存在".to_string())
}

/// 读取剪贴板条目的类型（text/image/file）。前端据此决定渲染 textarea 还是 ImagePreview。
#[tauri::command]
pub async fn get_clipboard_item_type(item_id: i64) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, item_id)
    })
    .map_err(e2s)?;
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
    .map_err(e2s)?;
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
    fn pending_tabs_push_multiple_and_take_all() {
        let _ = take_pending_tabs(); // 清空残留
        // push_pending_tab 读 DB（测试环境无 DB，走 fallback "text"/""）
        push_pending_tab(1, "clipboard");
        push_pending_tab(2, "clipboard");
        let got = take_pending_tabs();
        assert_eq!(got.len(), 2, "push 两个应 take 出两个");
        assert_eq!(got[0].item_id, 1);
        assert_eq!(got[1].item_id, 2);
        assert!(take_pending_tabs().is_empty(), "take 后应清空");
    }
}
