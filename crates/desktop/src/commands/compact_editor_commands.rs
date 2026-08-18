//! 统一内容查看器命令层（多 tab）：PENDING_TABS 暂存 + 开/取/读文本/关。
//!
//! Tab 类型：clipboard（文本/图片）| transcription（只读）| file（磁盘文件）。
//! - open_compact_editor_tab(item_id, source)：单开（前端命令）——转调 open_compact_editor_tabs
//! - open_compact_editor_tabs(items)：批量开——组装完整 tab 后转调 open_tabs_batched
//!   （一次 push 全部 + 一次 create/emit，避免连续单开在「窗口刚 build、React 未 mount」
//!   中间态丢失第二个 tab）
//! - open_files_in_editor(paths)：打开磁盘文件（图片入库开图片 tab、文本开 file tab，
//!   spec 2026-08-18-compact-editor-open-files）——经 open_tabs_batched 批量送出
//! - get_pending_compact_tabs()：前端 mount take 全部 pending（Vec）
//! - get_clipboard_item_text(item_id)：读 clipboard_history content
//! - get_transcription_text(id)：读 transcriptions 全文（只读 tab）
//! - get_clipboard_item_type(item_id)：读 item_type（前端据此渲染 textarea 或 ImagePreview）
//! - close_compact_editor：关窗

use parking_lot::Mutex;
use serde::Serialize;
use crate::core::error_util::e2s;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::compact_editor_window::{create_compact_editor_window, WINDOW_LABEL};

/// 临时 tab 打开参数（不写 DB）。mode=None 为单栏（现有行为），mode="contrast" 为翻译对照。
///
/// **R1 修复（2026-07-17）**：emit 时直接序列化此结构体（替代手写 json!），故加入
/// item_id / source / is_temp 字段以匹配前端 OpenTabPayload 类型——open_temp_compact_editor
/// 调用时这三项固定（item_id=0, source="temp", is_temp=true），由 open_temp_compact_editor
/// 在 emit 前补齐。
#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TempTabPayload {
    /// item_id 固定 "0"（temp tab 不写 DB）。emit 时补齐。
    #[serde(default)]
    pub item_id: String,
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
    pub item_id: String,
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

/// DB 组装完整 pending tab（读 itemType + text + 图片尺寸，一次合并——前端只需 1 次 IPC）。
/// 2026-08-18 从 push_pending_tab 抽出（open_compact_editor_tabs 组装后转调
/// open_tabs_batched 用，spec 2026-08-18-compact-editor-open-files §3.2）。
/// 查不到条目时降级 text/空串（保持原 push_pending_tab 行为）。
fn build_pending_tab(item_id: &str, source: &str) -> PendingTabFull {
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

    PendingTabFull {
        item_id: item_id.to_string(),
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
    }
}

/// 存储临时 tab（不查 DB，payload 直接传入）。
/// source 参数保留以兼容调用方语义（pending 队列按 source 路由），item_id/is_temp
/// 在此固定（temp tab 不写 DB）。
pub fn store_pending_temp(payload: TempTabPayload, source: &str) {
    PENDING_TABS.lock().push(PendingTabFull {
        item_id: "0".to_string(),
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
pub fn store_pending_file(item_id: String, text: String, file_path: String) {
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

/// 打开磁盘文件到 CompactEditor file tab（source="file"，编辑后保存写回磁盘）。
/// 窗口已存在 → emit open-tab；不存在 → store_pending_file + 建窗。
/// itemId = md5(路径) 前 16 hex → u64（固定 id，前端 file:&lt;id&gt; 去重——同文件重复打开聚焦同一 tab）。
/// 2026-08-18 从 prompt_files::open_file_in_editor 抽取共用（转 Markdown 输出文件复用，spec §5.2 修订）。
pub fn open_disk_file_in_compact_editor(app: &tauri::AppHandle, path: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取文件失败: {}", e))?;
    let hash = octopus_sync::store::md5_hex(path.as_bytes());
    // 第十九轮 P2-1：itemId 必须 string——前端 tab.itemId.slice(-5) 对 number 会
    // TypeError → React 子树崩溃 → CompactEditor 白屏。
    // 终审 Finding 1：u64 解析（i64 在 md5 首位 8-f 时溢出 → unwrap_or(0) → "file:0"
    // 碰撞）。与 collect_open_tabs 的同型表达式必须一字不差——跨路径去重依赖两处一致。
    let item_id = u64::from_str_radix(&hash[..16], 16).unwrap_or(0);
    let payload = serde_json::json!({
        "itemId": item_id.to_string(),
        "source": "file",
        "text": text,
        "filePath": path,
    });
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("compact-editor://open-tab", payload);
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        store_pending_file(item_id.to_string(), text, path.to_string());
        create_compact_editor_window(app, None);
    }
    Ok(())
}

fn take_pending_tabs() -> Vec<PendingTabFull> {
    std::mem::take(&mut *PENDING_TABS.lock())
}

// ── 打开已存在文件（spec 2026-08-18-compact-editor-open-files）──

/// 图片扩展名封闭清单（spec §1）；其余一律尝试 UTF-8 文本读。
/// 注：svg 是文本（可编辑），归文本路径。
fn is_image_ext(ext: &str) -> bool {
    matches!(
        ext.trim_start_matches('.').to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif",
    )
}

/// 文件图片入库（镜像 watcher.rs:165-211 的 ingest 组合）：
/// 读 bytes → 解码 → hash_rgba 去重（find_by_content_hash 命中则 touch 已有行）
/// → insert_image_data + insert_clipboard_item(type=image)。返回 (historyId, w, h)。
fn ingest_image_file(path: &std::path::Path) -> Result<(String, i64, i64), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败: {}", e))?;
    let dyn_img = ::image::load_from_memory(&bytes)
        .map_err(|_| "图片解码失败".to_string())?;
    // 超大图守卫（终审 Finding 2，镜像 watcher.rs 超过 40MB 跳过）：估算 RGBA 尺寸
    // 超限直接拒绝，在 to_rgba8 前拦截——否则 48MP 照片瞬时分配 ~500MB RGBA。
    let estimated_size = (dyn_img.width() as usize) * (dyn_img.height() as usize) * 4;
    if estimated_size > 40 * 1024 * 1024 {
        return Err("图片过大（上限约 40MB 解码后）".to_string());
    }
    let rgba_img = dyn_img.to_rgba8();
    let (w, h) = (rgba_img.width(), rgba_img.height());
    let rgba = rgba_img.to_vec();
    let hash = octopus_clipboard::image::hash_rgba(&rgba);

    // 历史级去重（watcher 同款）：同图 touch 已有行直接复用 id，不重复入库
    let existing = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::find_by_content_hash(conn, &hash)
    })
    .map_err(|e| format!("DB 查询失败: {}", e))?;
    if let Some(id) = existing {
        let _ = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::touch_created_at(conn, &id)
        });
        return Ok((id, w as i64, h as i64));
    }

    let dyn_img = ::image::DynamicImage::ImageRgba8(
        ::image::RgbaImage::from_raw(w, h, rgba).ok_or("RgbaImage::from_raw failed")?,
    );
    let encoded = octopus_clipboard::image::encode_image(&dyn_img)
        .map_err(|e| format!("编码失败: {}", e))?;
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::insert_image_data(
            conn, &hash, &encoded.image_blob, &encoded.thumb_blob, w as i64, h as i64,
        )
    })
    .map_err(|e| format!("图片存储失败: {}", e))?;

    let id = uuid::Uuid::new_v4().to_string();
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::insert_clipboard_item(
            conn,
            &octopus_clipboard::store::NewClipboardItem {
                id: id.clone(),
                item_type: octopus_clipboard::model::ItemType::Image,
                content: String::new(),
                ref_data: Some(hash.clone()),
                meta_info: Some(octopus_clipboard::model::MetaInfo {
                    w: Some(w),
                    h: Some(h),
                    size: Some(format!("{}KB", encoded.image_blob.len() / 1024)),
                    ..Default::default()
                }),
                created_at: octopus_clipboard::store::iso_now(),
                has_thumbnail: Some(1),
                is_rich: false,
            },
        )
    })
    .map_err(|e| format!("历史写入失败: {}", e))?;
    Ok((id, w as i64, h as i64))
}

/// 分流 + 组装 tab（纯核心，无 AppHandle 便于单测，spec §3.3）：
/// 图片 → 入库图片 tab（source="clipboard"，前端 loadAndAddTab 识别）；
/// 其余 → UTF-8 文本读 → file tab（md5 路径 itemId，与 file tab 去重规则一致）。
/// 失败逐个进 errors（`<文件名>（<原因>）`），不中断其他文件。
pub(crate) fn collect_open_tabs(paths: Vec<String>) -> (Vec<PendingTabFull>, Vec<String>) {
    let mut tabs = Vec::new();
    let mut errors = Vec::new();
    for p in paths {
        let path = std::path::PathBuf::from(&p);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.clone());
        if path.is_dir() {
            errors.push(format!("{}（暂不支持文件夹）", name));
            continue;
        }
        if !path.exists() {
            errors.push(format!("{}（文件不存在）", name));
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        if is_image_ext(&ext) {
            match ingest_image_file(&path) {
                Ok((id, w, h)) => tabs.push(PendingTabFull {
                    item_id: id,
                    source: "clipboard".into(),
                    item_type: "image".into(),
                    text: String::new(),
                    img_width: w as u32,
                    img_height: h as u32,
                    is_temp: false,
                    mode: None,
                    original_text: None,
                    translated_text: None,
                    translate_session_id: None,
                    file_path: None,
                }),
                Err(e) => errors.push(format!("{}（{}）", name, e)),
            }
        } else {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let hash = octopus_sync::store::md5_hex(p.as_bytes());
                    // 终审 Finding 1：u64 解析（i64 溢出场景同 open_disk_file_in_compact_editor
                    // 注释）——两处表达式一字不差，跨路径去重依赖一致。
                    let item_id = u64::from_str_radix(&hash[..16], 16).unwrap_or(0).to_string();
                    tabs.push(PendingTabFull {
                        item_id,
                        source: "file".into(),
                        item_type: "text".into(),
                        text,
                        img_width: 0,
                        img_height: 0,
                        is_temp: false,
                        mode: None,
                        original_text: None,
                        translated_text: None,
                        translate_session_id: None,
                        file_path: Some(p),
                    });
                }
                Err(_) => errors.push(format!("{}（非 UTF-8 文本或读取失败）", name)),
            }
        }
    }
    (tabs, errors)
}

/// 打开磁盘文件结果（camelCase，spec §3.3）。成功的 tab 经事件/pending 送出。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFilesResult {
    pub errors: Vec<String>,
}

/// 打开已存在的文件（spec 2026-08-18-compact-editor-open-files）：
/// 图片入库开图片 tab、文本开 file tab；失败逐个收集返回，命令本身不 Err。
#[tauri::command]
pub async fn open_files_in_editor(
    paths: Vec<String>,
    app: AppHandle,
) -> Result<OpenFilesResult, String> {
    // 图片解码 + 多文件 IO——spawn_blocking 防卡 runtime
    let (tabs, errors) = tokio::task::spawn_blocking(move || collect_open_tabs(paths))
        .await
        .map_err(|e| format!("打开任务异常: {}", e))?;
    // create_compact_editor_window 含 set_dock_icon 需主线程（同 markdown 分支模式）
    let ah = app.clone();
    let _ = app.run_on_main_thread(move || {
        open_tabs_batched(tabs, &ah);
    });
    Ok(OpenFilesResult { errors })
}

/// 批量开 tab（完整 payload 直传，不查 DB）。2026-08-18 从 open_compact_editor_tabs
/// 泛化（open-files 复用，spec §3.2）：
/// - 窗口存在且 React 已 mount（PENDING_TABS 空）→ 逐个 emit + show/focus
/// - 窗口存在未 mount → 全部 push pending（emit 会丢——listener 未注册）
/// - 窗口不存在 → push pending + 一次建窗（批量一次，避免连续单开的中间态丢 tab）
pub(crate) fn open_tabs_batched(tabs: Vec<PendingTabFull>, app: &tauri::AppHandle) {
    if tabs.is_empty() {
        return;
    }
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let react_mounted = PENDING_TABS.lock().is_empty();
        if react_mounted {
            for tab in tabs {
                let _ = window.emit("compact-editor://open-tab", tab);
            }
            let _ = window.show();
            let _ = window.set_focus();
        } else {
            PENDING_TABS.lock().extend(tabs);
        }
    } else {
        // 防幽灵 tab：上次建窗失败/React 未 mount 即关窗会留 stale pending
        //（close_compact_editor 不清队列）——建窗前先清，只交付本次 tabs。
        let _ = take_pending_tabs();
        // 首 tab 元数据注入建窗 URL（零 IPC 首屏，docs/architecture.md CompactEditor 段）。
        // 终审 Finding 3：恢复 Task 2 泛化时丢失的 first-tab 注入（原 open_compact_editor_tabs
        // 的 pending_data.as_ref() 首参）——否则 URL 注入与前端 readInitialTabFromUrl 成死代码。
        let first = tabs.first().cloned();
        PENDING_TABS.lock().extend(tabs);
        create_compact_editor_window(app, first.as_ref());
    }
}

/// 批量打开多个 tab（一次调用）。逐 item 经 build_pending_tab 查 DB 组装完整
/// PendingTabFull，再转调 open_tabs_batched（emit-or-pending + 一次建窗）。
///
/// 一次调用避免连续单开的中间态：第一次单开 `build()` 同步注册窗口 label 后，
/// 第二次单开会命中 `get_webview_window=Some` 走 emit 分支，但此时 WebView/React
/// 尚未 mount → emit 被丢 + pending 覆盖首个 tab → 第二个 tab 永久丢失
/// （截图 OCR 双开图片+文本 tab 即此 bug）。批量调用只走一次 create/emit，无中间态。
pub fn open_compact_editor_tabs(items: &[(String, Option<&str>)], app_handle: &tauri::AppHandle) {
    if items.is_empty() {
        return;
    }
    let tabs: Vec<PendingTabFull> = items
        .iter()
        .map(|(id, src)| {
            let s = src.unwrap_or("clipboard");
            log::info!("[compact-editor] open_tab item_id={} source={} ({} tab(s) batched)", id, s, items.len());
            build_pending_tab(id, s)
        })
        .collect();
    open_tabs_batched(tabs, app_handle);
}

/// 打开统一查看器并定位到某 tab（单开，前端命令）——转调批量版单元素。
#[tauri::command]
pub fn open_compact_editor_tab(
    item_id: String,
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
pub async fn get_clipboard_item_text(item_id: String) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, &item_id)
    })
    .map_err(e2s)?;
    item.map(|i| i.content).ok_or_else(|| "条目不存在".to_string())
}

/// 读取剪贴板条目的类型（text/image/file）。前端据此决定渲染 textarea 还是 ImagePreview。
#[tauri::command]
pub async fn get_clipboard_item_type(item_id: String) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, &item_id)
    })
    .map_err(e2s)?;
    item.map(|i| i.item_type.as_str().to_string())
        .ok_or_else(|| "条目不存在".to_string())
}

/// 读取语音识别记录的全文（只读 tab）。
/// 转译记录已合并入 clipboard_history（item_type='voice'），从 content 列读全文。
#[tauri::command]
pub async fn get_transcription_text(id: String) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, &id)
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
        // build_pending_tab 读 DB（测试环境查不到条目，走 fallback "text"/""）
        PENDING_TABS.lock().push(build_pending_tab("test-1", "clipboard"));
        PENDING_TABS.lock().push(build_pending_tab("test-2", "clipboard"));
        let got = take_pending_tabs();
        assert_eq!(got.len(), 2, "push 两个应 take 出两个");
        assert_eq!(got[0].item_id, "test-1");
        assert_eq!(got[1].item_id, "test-2");
        assert!(take_pending_tabs().is_empty(), "take 后应清空");
    }

    // ── open_files_in_editor（spec 2026-08-18-compact-editor-open-files）──

    // 图片入库触达 with_db——init_test_db 切 in-memory，防绑开发库（AGENTS.md 测试隔离）
    static OPEN_FILES_DB_SETUP: std::sync::Once = std::sync::Once::new();
    fn setup_open_files_test_db() {
        OPEN_FILES_DB_SETUP.call_once(|| {
            octopus_infra::db::init_test_db();
        });
    }

    /// 1×1 红色 PNG（经典 70 字节 base64）。
    const TINY_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn tmp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("octopus-open-files-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn test_is_image_ext_matrix() {
        for ext in ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif"] {
            assert!(is_image_ext(ext), "ext={}", ext);
            assert!(is_image_ext(&ext.to_uppercase()), "大小写不敏感：{}", ext);
        }
        assert!(is_image_ext(".PNG"), "前导点容忍");
        for ext in ["md", "txt", "pdf", "docx", "", "svg"] {
            assert!(!is_image_ext(ext), "ext={} 应非图片", ext);
        }
    }

    #[test]
    fn test_collect_open_tabs_text_file() {
        let p = tmp_path("note.md");
        std::fs::write(&p, b"# hello").unwrap();
        let (tabs, errors) = collect_open_tabs(vec![p.to_string_lossy().to_string()]);
        assert!(errors.is_empty(), "errors={:?}", errors);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].source, "file");
        assert_eq!(tabs[0].text, "# hello");
        assert_eq!(tabs[0].file_path.as_deref(), Some(p.to_string_lossy().as_ref()));
        // itemId = md5(路径) 前 16 hex → u64（与 open_disk_file_in_compact_editor 同规则）。
        // 终审 Finding 1：期望值必须用 u64 算——旧测试用同型 i64 表达式自证清白，
        // 对 i64 溢出（md5 首位 8-f）致 "0" 碰撞的 bug 全盲。
        let hash = octopus_sync::store::md5_hex(p.to_string_lossy().as_bytes());
        let expect = u64::from_str_radix(&hash[..16], 16).unwrap_or(0).to_string();
        assert_eq!(tabs[0].item_id, expect);
    }

    /// md5 溢出探针（终审 Finding 1 回归测试）：固定路径（不掺 pid，md5 可复现）
    /// `/tmp/octopus-md5-overflow-probe1.md` 的 md5 = `ca911dde477bd18993ea92d817ca59e2`
    /// ——首位 'c' ≥ '8'，前 16 hex 值 0xca911dde477bd189 > i64::MAX
    /// (0x7fffffffffffffff)，旧实现 i64::from_str_radix 解析失败 → unwrap_or(0)
    /// → itemId "0"（~50% 路径碰撞在 tab key "file:0" 上）。同批次重复打开共享
    /// 同一 itemId（前端 file:<id> 去重聚焦，覆盖 accumulated-minor #4）。
    #[test]
    fn test_collect_open_tabs_md5_no_i64_overflow() {
        let p = std::path::PathBuf::from("/tmp/octopus-md5-overflow-probe1.md");
        std::fs::write(&p, b"probe").unwrap();
        let hash = octopus_sync::store::md5_hex(p.to_string_lossy().as_bytes());
        assert_eq!(hash, "ca911dde477bd18993ea92d817ca59e2", "探针 md5 漂移（路径串变了？）");
        assert!(hash.as_bytes()[0] >= b'8', "探针 md5 首位须 ≥ '8' 才覆盖溢出分支");
        let expect = u64::from_str_radix(&hash[..16], 16).unwrap().to_string();
        assert_ne!(expect, "0");

        let (tabs, errors) = collect_open_tabs(vec![
            p.to_string_lossy().to_string(),
            p.to_string_lossy().to_string(),
        ]);
        assert!(errors.is_empty(), "errors={:?}", errors);
        assert_eq!(tabs.len(), 2, "同批次重复打开应产出两个 tab");
        assert_ne!(tabs[0].item_id, "0", "i64 溢出场景 itemId 不应塌缩为 \"0\"");
        assert_eq!(tabs[0].item_id, expect, "itemId 应等于 u64 解析期望值");
        assert_eq!(tabs[0].item_id, tabs[1].item_id, "同一路径重复打开 → 同一 itemId（file:<id> 去重）");
    }

    /// 超大图守卫（终审 Finding 2）：4000×3000 估算 RGBA ~45.8MiB > 40MiB 上限
    /// （watcher.rs 同款阈值）——应在 to_rgba8 前拒绝，进 errors 不中断批次。
    #[test]
    fn test_collect_open_tabs_oversized_image_rejected() {
        setup_open_files_test_db();
        let p = tmp_path("huge.png");
        let img = ::image::RgbaImage::from_pixel(4000, 3000, ::image::Rgba([200u8, 30, 30, 255]));
        ::image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::BufWriter::new(std::fs::File::create(&p).unwrap()),
                ::image::ImageFormat::Png,
            )
            .unwrap();
        let (tabs, errors) = collect_open_tabs(vec![p.to_string_lossy().to_string()]);
        assert!(tabs.is_empty(), "超大图不应开 tab");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("huge.png"), "err={}", errors[0]);
        assert!(errors[0].contains("图片过大"), "err={}", errors[0]);
    }

    #[test]
    fn test_collect_open_tabs_non_utf8_rejected() {
        let p = tmp_path("bad.bin");
        std::fs::write(&p, [0xFFu8, 0xFE, 0x00, 0x01]).unwrap();
        let (tabs, errors) = collect_open_tabs(vec![p.to_string_lossy().to_string()]);
        assert!(tabs.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("非 UTF-8"), "err={}", errors[0]);
        assert!(errors[0].contains("bad.bin"));
    }

    #[test]
    fn test_collect_open_tabs_dir_rejected() {
        let dir = tmp_path("adir");
        std::fs::create_dir_all(&dir).unwrap();
        let (tabs, errors) = collect_open_tabs(vec![dir.to_string_lossy().to_string()]);
        assert!(tabs.is_empty());
        assert!(errors[0].contains("暂不支持文件夹"));
    }

    #[test]
    fn test_collect_open_tabs_image_ingests() {
        setup_open_files_test_db();
        let p = tmp_path("tiny.png");
        std::fs::write(&p, base64_decode(TINY_PNG_B64)).unwrap();
        let (tabs, errors) = collect_open_tabs(vec![p.to_string_lossy().to_string()]);
        assert!(errors.is_empty(), "errors={:?}", errors);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].source, "clipboard");
        assert_eq!(tabs[0].item_type, "image");
        assert_eq!(tabs[0].img_width, 1);
        assert_eq!(tabs[0].img_height, 1);
        assert!(!tabs[0].item_id.is_empty());
    }

    #[test]
    fn test_collect_open_tabs_mixed_partial_success() {
        setup_open_files_test_db();
        let ok = tmp_path("ok.md");
        std::fs::write(&ok, b"fine").unwrap();
        let bad = tmp_path("no.txt");
        std::fs::write(&bad, [0xFFu8, 0xFE]).unwrap();
        let img = tmp_path("i.png");
        std::fs::write(&img, base64_decode(TINY_PNG_B64)).unwrap();
        let (tabs, errors) = collect_open_tabs(vec![
            ok.to_string_lossy().to_string(),
            bad.to_string_lossy().to_string(),
            img.to_string_lossy().to_string(),
        ]);
        assert_eq!(tabs.len(), 2, "文本+图片成功");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("no.txt"));
    }

    /// 最小 base64 解码（测试专用，避免引依赖）。
    fn base64_decode(s: &str) -> Vec<u8> {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut buf = 0u32;
        let mut bits = 0u32;
        for c in s.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()) {
            let v = TABLE.iter().position(|t| *t == c).expect("非法 base64") as u32;
            buf = (buf << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
        out
    }
}
