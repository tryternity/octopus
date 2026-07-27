use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use base64::{Engine, engine::general_purpose};
use octopus_clipboard::{ClipboardHandle, ClipboardItem, QueryFilter};

#[tauri::command]
pub async fn query_clipboard_history(
    filter: String,
    search: Option<String>,
    page: u32,
    size: u32,
) -> Result<Vec<ClipboardItem>, String> {
    // spawn_blocking：with_db 持全局 ReentrantMutex，watcher 写大图（WebP 编码 ~50-200ms）
    // 期间会阻塞读查询。包 spawn_blocking 避免卡住 Tokio worker 影响其他 IPC。
    tokio::task::spawn_blocking(move || {
        octopus_infra::db::with_db(|conn| {
            let qf = QueryFilter {
                filter,
                search,
                page: page.max(1),
                size: size.max(1),
            };
            octopus_clipboard::store::query_history(conn, &qf)
        })
    })
    .await
    .map_err(|e| format!("join error: {}", e))?
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn toggle_clipboard_favorite(
    id: i64,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::toggle_favorite(conn, id)
    })
    .map_err(|e| e.to_string())?;
    // 广播给浮窗 + 设置页同步刷新（否则两端列表状态不一致）
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(())
}

#[tauri::command]
pub async fn delete_clipboard_item(
    id: i64,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::delete_item(conn, id)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(())
}

#[tauri::command]
pub async fn delete_clipboard_items(
    ids: Vec<i64>,
    app_handle: tauri::AppHandle,
) -> Result<usize, String> {
    let n = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::delete_items(conn, &ids)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(n)
}

#[tauri::command]
pub async fn clear_clipboard_history(
    keep_favorite: bool,
    app_handle: tauri::AppHandle,
) -> Result<usize, String> {
    let n = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::clear_history(conn, keep_favorite)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(n)
}

/// 按当前 tab 类别（filter）批量清理非收藏条目。镜像 clear_clipboard_history，
/// 多一个 filter 参数走 clear_history_by_filter；emit clipboard://changed 触前端自动 refresh。
#[tauri::command]
pub async fn clear_clipboard_history_by_filter(
    filter: String,
    keep_favorite: bool,
    app_handle: tauri::AppHandle,
) -> Result<usize, String> {
    let n = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::clear_history_by_filter(conn, &filter, keep_favorite)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(n)
}

// ── 回收站操作（软删/还原/永久删/清空）── 2026-07-22 v47

#[tauri::command]
pub async fn restore_clipboard_item(
    id: i64,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::restore_item(conn, id)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(())
}

#[tauri::command]
pub async fn restore_clipboard_items(
    ids: Vec<i64>,
    app_handle: tauri::AppHandle,
) -> Result<usize, String> {
    let n = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::restore_items(conn, &ids)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(n)
}

#[tauri::command]
pub async fn permanent_delete_clipboard_item(
    id: i64,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::permanent_delete_item(conn, id)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(())
}

#[tauri::command]
pub async fn permanent_delete_clipboard_items(
    ids: Vec<i64>,
    app_handle: tauri::AppHandle,
) -> Result<usize, String> {
    let n = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::permanent_delete_items(conn, &ids)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(n)
}

#[tauri::command]
pub async fn empty_clipboard_trash(
    app_handle: tauri::AppHandle,
) -> Result<usize, String> {
    let n = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::empty_trash(conn)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(n)
}

/// 按条目类型把内容写到系统剪贴板（copy / paste 共用）：
/// - Text/Voice/Ocr: write_text(content)
/// - Image: ref_data 存 blob_hash → 从 image_data 读 WebP 原图 → 转 PNG → write_image
/// - File: ref_data 存 JSON 路径数组 → write_files
fn write_item_to_clipboard(handle: &ClipboardHandle, item: &ClipboardItem) -> Result<(), String> {
    match item.item_type {
        octopus_clipboard::ItemType::Text
        | octopus_clipboard::ItemType::Voice
        | octopus_clipboard::ItemType::Ocr => handle.write_text(&item.content).map_err(|e| e.to_string()),
        octopus_clipboard::ItemType::Image => {
            let blob_hash = item.ref_data.clone()
                .ok_or("图片元数据缺失")?;
            let webp_blob = octopus_infra::db::with_db(|conn| {
                octopus_clipboard::store::get_image_blob(conn, &blob_hash)
            })
            .map_err(|e| e.to_string())?
            .ok_or("图片数据不存在")?;
            // image_data 存 WebP 无损 BLOB；write_image 契约是 PNG，转码一次
            let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
                .map_err(|e| format!("解码 WebP 失败: {}", e))?;
            let mut png = Vec::new();
            let mut cursor = std::io::Cursor::new(&mut png);
            let png_encoder = ::image::codecs::png::PngEncoder::new_with_quality(
                &mut cursor,
                ::image::codecs::png::CompressionType::Fast,
                ::image::codecs::png::FilterType::Up,
            );
            img.write_with_encoder(png_encoder)
                .map_err(|e| format!("编码 PNG 失败: {}", e))?;
            handle.write_image(&png).map_err(|e| e.to_string())
        }
        octopus_clipboard::ItemType::File => {
            let paths_json = item.ref_data.as_ref()
                .ok_or("文件路径缺失")?;
            let paths: Vec<String> = serde_json::from_str(paths_json)
                .map_err(|e| format!("解析文件路径失败: {}", e))?;
            handle.write_files(paths).map_err(|e| e.to_string())
        }
    }
}

#[tauri::command]
pub async fn copy_clipboard_item(
    id: i64,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<(), String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, id)
    })
    .map_err(|e| e.to_string())?;

    if let Some(item) = item {
        let handle = handle.inner().clone();
        // DB 读 + WebP 解码 + PNG 编码 + 剪贴板写入全是 CPU 密集操作，
        // 移入 spawn_blocking 避免阻塞 Tauri UI 线程
        tokio::task::spawn_blocking(move || {
            write_item_to_clipboard(&handle, &item)
        }).await.map_err(|e| e.to_string())??;
    }
    Ok(())
}

/// 统计符合当前 filter（类型筛选）+ search（搜索框）的条目数。
/// 与 query_clipboard_history 同条件，保证底部「共 N 条」随筛选/搜索变化，
/// 而非恒为全表总数。前端两处（浮窗 useClipboardHistory / 设置页 ClipboardPanel）均传参。
#[tauri::command]
pub async fn clipboard_stats(
    filter: String,
    search: Option<String>,
) -> Result<i64, String> {
    // spawn_blocking：与 query_clipboard_history 同模式，避免 watcher 长写阻塞读。
    tokio::task::spawn_blocking(move || {
        octopus_infra::db::with_db(|conn| {
            let qf = QueryFilter {
                filter,
                search,
                page: 1,
                size: 1,
            };
            octopus_clipboard::store::count_history(conn, &qf)
        })
    })
    .await
    .map_err(|e| format!("join error: {}", e))?
    .map_err(|e| e.to_string())
}

/// 双击条目：写剪贴板 → hide 窗口 → 恢复焦点 → 模拟粘贴
#[tauri::command]
pub async fn paste_clipboard_item(
    id: i64,
    app_handle: tauri::AppHandle,
    handle: State<'_, Arc<ClipboardHandle>>,
    focus: State<'_, Arc<crate::focus_tracker::FocusTracker>>,
) -> Result<(), String> {
    // 1. 从 DB 按 id 读条目内容（O(1) rowid 查找，不再整页反序列化）
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, id)
    })
    .map_err(|e| e.to_string())?;

    let item = match item {
        Some(item) => item,
        None => return Ok(()),
    };

    // 2. hide 剪贴板窗口
    let win = app_handle.get_webview_window("clipboard_window");
    if let Some(w) = &win {
        let _ = w.hide();
    }
    drop(win);

    // 3. 恢复焦点 + 粘贴（同一线程）
    let handle = handle.inner().clone();
    let focus = focus.inner().clone();
    std::thread::spawn(move || {
        // 1. 按类型写剪贴板（文本/图片/文件，还原真实内容而非 hash/JSON）
        let _ = write_item_to_clipboard(&handle, &item);
        // 2. hide 后 macOS 自动还焦点（已确认 sublime_text 获得焦点）
        // 3. 等焦点稳定
        std::thread::sleep(std::time::Duration::from_millis(300));
        // 4. osascript 发 Cmd+V 给当前前台应用（不经过 enigo）
        focus.restore_focus();
        focus.simulate_paste();
    });

    Ok(())
}

/// 图片条目：直接保存到 ~/Downloads/octopus/，不弹系统对话框。
/// format: "jpeg" | "webp" | "png"，quality: 1-100（jpeg/webp 生效）
/// open_folder: true 时保存后用系统文件管理器定位到该文件
/// 返回写入的绝对路径，同时写入剪贴板。
#[tauri::command]
pub async fn save_image_item(
    id: i64,
    format: String,
    quality: Option<u8>,
    open_folder: Option<bool>,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<String, String> {
    let fmt = format.to_lowercase();
    let q = quality.unwrap_or(85).clamp(1, 100);
    let open_after = open_folder.unwrap_or(false);

    // 1. 从 DB 读条目
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, id)
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;

    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.ref_data.clone()
        .ok_or("图片元数据缺失")?;

    // 2. 从 DB 读 WebP 无损 BLOB
    let webp_blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;

    // 3. 目标目录 ~/Downloads/octopus/
    let downloads_dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or("无法确定下载目录")?
        .join("octopus");
    std::fs::create_dir_all(&downloads_dir).map_err(|e| e.to_string())?;

    // 4. 确定扩展名 + 文件名（带去重）
    let ext = match fmt.as_str() {
        "png" => "png",
        "webp" => "webp",
        _ => "jpg",
    };
    let base_name = &blob_hash[..8.min(blob_hash.len())];
    let save_path = unique_path(&downloads_dir, base_name, ext);

    // 5. 编码写入——CPU/IO 密集（WebP 解码 + PNG/JPEG 编码 + 文件写入）移入 spawn_blocking
    let save_path_clone = save_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        match ext {
            "png" => {
                let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
                    .map_err(|e| e.to_string())?;
                img.save_with_format(&save_path_clone, ::image::ImageFormat::Png)
                    .map_err(|e| e.to_string())?;
            }
            "webp" => {
                std::fs::write(&save_path_clone, &webp_blob).map_err(|e| e.to_string())?;
            }
            _ => {
                let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
                    .map_err(|e| e.to_string())?;
                let rgb = img.to_rgb8();
                let mut buf = std::io::BufWriter::new(
                    std::fs::File::create(&save_path_clone).map_err(|e| e.to_string())?
                );
                let mut encoder = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
                encoder.encode(&rgb, rgb.width(), rgb.height(), ::image::ExtendedColorType::Rgb8)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("save_image_item 任务异常: {}", e))??;

    // 6. 写文件路径到剪贴板
    let abs_path = save_path.to_string_lossy().to_string();
    handle.write_text(&abs_path).map_err(|e| e.to_string())?;

    // 7. 可选：用系统文件管理器定位到该文件
    if open_after {
        reveal_in_file_manager(&save_path);
    }

    Ok(abs_path)
}

/// 用系统文件管理器打开并高亮指定文件。
fn reveal_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .args(["-R", &path.to_string_lossy()])
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.to_string_lossy()))
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn();
    }
}

/// 在 dir 下找不冲突的文件名：base.ext → base-1.ext → base-2.ext …
fn unique_path(dir: &std::path::Path, base: &str, ext: &str) -> std::path::PathBuf {
    let first = dir.join(format!("{}.{}", base, ext));
    if !first.exists() {
        return first;
    }
    for i in 1..1000 {
        let candidate = dir.join(format!("{}-{}.{}", base, i, ext));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{}.{}", base, ext))
}

/// 解析剪贴板文件路径为本地路径。
/// - Linux X11/Wayland：text/uri-list 存 `file://` URI + 百分号编码 → strip 前缀 + 解码
/// - macOS（clipboard-rs 用 NSURL.path）/ Windows（FileList）：已解码的普通路径，无 `file://` 前缀
///   仅 `file://` 开头才解码，避免对含字面 `%XX` 的普通路径误伤（如 `50%20off.txt`）。
fn decode_file_uri(raw: &str) -> String {
    use percent_encoding::percent_decode_str;
    if let Some(rest) = raw.strip_prefix("file://") {
        percent_decode_str(rest).decode_utf8_lossy().into_owned()
    } else {
        raw.to_string()
    }
}

/// 文件条目：用系统默认应用打开第一个文件
#[tauri::command]
pub async fn open_file_item(id: i64) -> Result<(), String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, id)
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::File {
        return Err("非文件条目".into());
    }

    // 解析 JSON 路径数组。file 条目入库时 content 为空、路径存 ref_data
    // （watcher.rs insert_clipboard_item + store.rs File 分支强制 content=空），
    // 与 write_item_to_clipboard File 分支一致读 ref_data。旧代码误读 content
    // 导致 serde_json::from_str("") 全平台失败、「打开文件」按钮失效。
    let paths_json = item.ref_data.as_ref().ok_or("文件路径缺失")?;
    let paths: Vec<String> = serde_json::from_str(paths_json)
        .map_err(|e| format!("解析路径失败: {}", e))?;

    let first = paths.first().ok_or("无文件路径")?;
    let path = decode_file_uri(first);

    // 用系统默认应用打开
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        // explorer <file> 只在资源管理器里「定位并选中」文件，不会用默认程序打开
        // （与 macOS `open` / Linux `xdg-open` 不一致）；改用 cmd /c start 调起默认
        // 关联程序。"" 是 start 的窗口标题占位，不可省——否则含空格/特殊字符的路径
        // 会被 start 误当作标题。注：此 Windows 分支未经本机编译/运行验证。
        std::process::Command::new("cmd")
            .args(["/C", "start", "", path.as_str()])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 当前 OCR 引擎/模型元数据：engine 固定 paddle（ocr_rs 基于 PaddleOCR）；
/// model 从 OCR 域激活模型（resolve_active_engine("ocr")）取，默认 PP-OCRv6-small。
/// insert_ocr_clipboard_item 与 ocr_screenshot 两处复用，保证 ocr 条目 meta 一致。
/// 返回 (engine, model) 元组，匹配 insert_ocr_item(text, engine, model) 签名。
pub(crate) fn current_ocr_meta() -> (String, String) {
    // Task 2 后：OCR 激活模型从 ACTIVE_ENGINES 缓存取（无激活 fallback 默认）。
    let model_name = octopus_asr_local::config::resolve_active_engine("ocr")
        .map(|r| r.name)
        .unwrap_or_else(|_| octopus_ocr::model::DEFAULT_OCR_MODEL.to_string());
    ("paddle".to_string(), model_name)
}

/// OCR 统一入库：识别文本 → 新建 source=ocr 剪贴板条目 → 广播刷新 → 返回新 id。
/// 三处 OCR 入口（截图/图片预览/剪贴板图片条目）识别出文本后统一走此命令入库，
/// 再由前端 openCompactEditorTab(id) 打开绑定 tab 编辑。
#[tauri::command]
pub async fn insert_ocr_clipboard_item(
    text: String,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    // current_ocr_meta 在 with_db 外取：其内部 load_config_key → with_db，闭包内调虽已
    // 不再死锁（db.rs 已换 ReentrantMutex，同线程重入安全），但仍会让 DB 锁跨 load_config_key
    // 嵌套持有；外取既保持锁短持、又避免重入链路，故保留此习惯。
    let (ocr_engine, ocr_model) = current_ocr_meta();
    log::info!("[insert-ocr] before insert text_len={}", text.len());
    let id = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::insert_ocr_item(conn, &text, &ocr_engine, &ocr_model)
    })
    .map_err(|e| {
        log::error!("[insert-ocr] insert FAILED: {:?}", e);
        e.to_string()
    })?;
    log::info!("[insert-ocr] after insert id={}", id);
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(id)
}

/// 图片条目 OCR：识别文本并返回（纯识别，不入库不写剪贴板）。
/// 入库由前端统一调 insert_ocr_clipboard_item 完成（三入口一致），再 openCompactEditorTab 编辑。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrTextBlock {
    pub text: String,
    pub x: f64, pub y: f64, pub w: f64, pub h: f64,
    pub score: f64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub text: String,
    pub blocks: Vec<OcrTextBlock>,
}

#[tauri::command]
pub async fn ocr_image(id: i64) -> Result<OcrResult, String> {
    // 全局 OCR 互斥：已有 OCR 在跑则立即拒绝，避免多任务并发进入推理。
    let _ocr_lock = octopus_ocr::engine::OcrLockGuard::try_acquire()
        .ok_or_else(|| "前一个 OCR 还未完成，请稍后".to_string())?;
    log::info!(
        "[ocr-image] start id={} thread={:?}",
        id,
        std::thread::current().id()
    );
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, id)
    })
    .map_err(|e| e.to_string())?;
    log::info!("[ocr-image] got item");

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.ref_data.clone()
        .ok_or("图片元数据缺失")?;

    let webp_blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;
    log::info!("[ocr-image] got blob {} bytes", webp_blob.len());

    log::info!("[ocr-image] before OcrEngine::instance()");
    let engine = octopus_ocr::engine::OcrEngine::instance()
        .map_err(|e| e.to_string())?;
    log::info!("[ocr-image] after OcrEngine::instance()");

    log::info!("[ocr-image] before recognize()");
    // CPU 密集推理移入 spawn_blocking——避免阻塞 Tokio worker 线程
    // （与 ocr_screenshot 同模式）
    let (text, blocks) = {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            engine.recognize_with_blocks(&webp_blob)
        })
        .await
        .map_err(|e| format!("OCR 任务异常: {}", e))?
        .map_err(|e| e.to_string())?
    };
    log::info!("[ocr-image] after recognize() text_len={} blocks={}", text.len(), blocks.len());

    if text.trim().is_empty() {
        return Err("未识别到文本".into());
    }

    let blocks = blocks.into_iter().map(|b| OcrTextBlock {
        text: b.text, x: b.x, y: b.y, w: b.w, h: b.h, score: b.score,
    }).collect();
    Ok(OcrResult { text, blocks })
}

/// 图片条目二维码识别：按 image_id 读 DB 图片 blob（WebP）→ 解码 → qrcode::scan →
/// 非空则 join("\n") 写剪贴板。与 ocr_image 同模式读 blob，但不走 OCR 推理、不入库。
/// 返回识别到的二维码内容列表（可能空）。
#[tauri::command]
pub async fn scan_qrcode_image(
    image_id: i64,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<Vec<String>, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, image_id)
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.ref_data.clone()
        .ok_or("图片元数据缺失")?;

    let webp_blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;

    // WebP 解码 + rqrr 识别 + 写剪贴板：CPU 密集，移入 spawn_blocking 隔离 Tokio worker
    // （与 ocr_image / write_item_to_clipboard Image 分支同范式）。
    let handle_clone = handle.inner().clone();
    let codes = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
            .map_err(|e| format!("解码 WebP 失败: {}", e))?;
        let codes = octopus_ocr::qrcode::scan(&img).map_err(|e| e.to_string())?;
        if !codes.is_empty() {
            let joined = codes.join("\n");
            handle_clone.write_text(&joined).map_err(|e| e.to_string())?;
        }
        Ok(codes)
    })
    .await
    .map_err(|e| format!("scan_qrcode_image 任务异常: {}", e))??;
    Ok(codes)
}

/// 精简编辑器回写：更新剪贴板条目文本（content）并同步系统剪贴板。
/// OCR 编辑、剪贴板文本条目编辑两处共用。
#[tauri::command]
pub async fn set_clipboard_item_text(
    item_id: i64,
    text: String,
    handle: State<'_, Arc<ClipboardHandle>>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_content(conn, item_id, &text)
    })
    .map_err(|e| e.to_string())?;

    handle.write_text(&text).map_err(|e| e.to_string())?;
    // 编辑器是独立窗口，剪贴板列表窗口需靠此事件感知条目变化并重新拉取。
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(())
}

/// 精简编辑器「图文编辑」入口保存：插入一条新文本剪贴板条目（content=text），
/// 同步系统剪贴板（write_text 自带 suppress 不重复入库）并广播变更。返回新 id
/// ——前端据此把 temp tab 升级为正式 clipboard tab（key/itemId/isTemp 同步）。
/// 与 set_clipboard_item_text 对称（后者是 update 既有条目，此为 insert 新条目）。
#[tauri::command]
pub async fn insert_clipboard_text_item(
    text: String,
    handle: State<'_, Arc<ClipboardHandle>>,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let new_id = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
            id: octopus_clipboard::store::chrono_millis(),
            item_type: octopus_clipboard::ItemType::Text,
            content: text.clone(),
            ref_data: None,
            meta_info: Some(octopus_clipboard::MetaInfo {
                char_count: Some(text.chars().count()),
                ..Default::default()
            }),
            created_at: octopus_clipboard::store::iso_now(),
            has_thumbnail: None,
            is_rich: false,
        })
    })
    .map_err(|e| e.to_string())?;

    handle.write_text(&text).map_err(|e| e.to_string())?;
    // 编辑器是独立窗口，剪贴板列表窗口需靠此事件感知条目变化并重新拉取。
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(new_id)
}

/// 获取图片缩略图 data URL（base64 编码：`data:image/webp;base64,...`）。
///
/// 返回完整 data URL 而非裸 `Vec<u8>`：Tauri IPC 把 `Vec<u8>` 序列化成 JSON 数字数组
/// （4-5x 膨胀），前端还要 `map/join/btoa` 手动转 base64。后端一次编码成 data URL，
/// 前端直接 `<img src={...}>`，省掉膨胀与转换开销（剪贴板窗口滚动时每个图片条目都触发）。
#[tauri::command]
pub async fn get_image_thumb(id: i64) -> Result<String, String> {
    // spawn_blocking：两次 with_db 调用（item + thumb blob），可能在 watcher 写大图时阻塞。
    tokio::task::spawn_blocking(move || -> Result<String, String> {
        let item = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::get_item_by_id(conn, id)
        })
        .map_err(|e| e.to_string())?;

        let item = item.ok_or("条目不存在")?;
        if item.item_type != octopus_clipboard::ItemType::Image {
            return Err("非图片条目".into());
        }

        let blob_hash = item.ref_data.clone()
            .ok_or("图片元数据缺失")?;

        let thumb_blob = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::get_image_thumb(conn, &blob_hash)
        })
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "缩略图不存在".to_string())?;

        Ok(format!(
            "data:image/webp;base64,{}",
            general_purpose::STANDARD.encode(&thumb_blob)
        ))
    })
    .await
    .map_err(|e| format!("join error: {}", e))?
}

/// 取图片全分辨率（image_data.blob）→ data URL（base64 + WebP 前缀）。
///
/// 前端 ImagePreview 用它加载到 <img>/canvas 做标注。镜像 get_image_thumb，
/// 仅取 blob（全分辨率）而非 thumb。返回 data URL 同样为避免 IPC 序列化膨胀。
#[tauri::command]
pub async fn get_image_full(id: i64) -> Result<tauri::ipc::Response, String> {
    // spawn_blocking：两次 with_db（item + blob，4MB WebP 读取），watcher 写时可能阻塞。
    // ipc::Response 不便从 spawn_blocking 闭包返回（非简单 Send），只把 DB 读包进去。
    let blob: Vec<u8> = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let item = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::get_item_by_id(conn, id)
        })
        .map_err(|e| e.to_string())?;

        let item = item.ok_or("条目不存在")?;
        if item.item_type != octopus_clipboard::ItemType::Image {
            return Err("非图片条目".into());
        }

        let blob_hash = item.ref_data.clone()
            .ok_or("图片元数据缺失")?;

        let blob = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::get_image_blob(conn, &blob_hash)
        })
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "图片数据缺失".to_string())?;

        Ok(blob)
    })
    .await
    .map_err(|e| format!("join error: {}", e))??;

    // 返回原始 WebP 字节（Raw body），前端用 URL.createObjectURL 加载
    Ok(tauri::ipc::Response::new(blob))
}

/// 弹系统保存对话框，把前端合成的标注 PNG（base64）存到用户指定路径。
///
/// 把前端合成的标注 PNG（Raw body 二进制）写入系统剪贴板。
/// 前端调用：invoke("copy_image_to_clipboard", uint8array)
#[tauri::command]
pub async fn copy_image_to_clipboard(
    request: tauri::ipc::Request<'_>,
    handle: State<'_, Arc<ClipboardHandle>>,
    _app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(png_bytes) = request.body() else {
        return Err("expected raw binary body".into());
    };
    let png_bytes = png_bytes.clone();
    // write_image（PNG 解码 + set_image）是 CPU 密集，移入 spawn_blocking 避免阻塞 IPC。
    // 入库改走 clipboard_queue（与 watcher 同路径，后台 worker 异步处理 + emit）。
    let handle_clone = handle.inner().clone();
    tokio::task::spawn_blocking(move || {
        handle_clone.write_image(&png_bytes).map_err(|e| e.to_string())?;
        // write_image 触发 NSPasteboard 变化 → watcher 会收到通知 → enqueue。
        // 但有时通知有延迟，这里主动 enqueue 一次确保 worker 处理。
        crate::clipboard_queue::enqueue();
        Ok::<(), String>(())
    }).await.map_err(|e| e.to_string())??;
    Ok(())
}

/// 预览窗保存图片：前端传 Raw body 二进制 PNG。
/// 前端调用：invoke("save_image_dialog", uint8array)
#[tauri::command]
pub async fn save_image_dialog(
    request: tauri::ipc::Request<'_>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(png_bytes) = request.body() else {
        return Err("expected raw binary body".into());
    };

    // blocking_save_file（弹原生对话框，等用户选路径可达数秒）+ fs::write 均为同步阻塞，
    // 全部移入 spawn_blocking 避免卡住 Tokio worker 线程（与上方 copy_image_to_clipboard 同模式）。
    // 不用 plugin 的 save_file() 回调式 API：回调内 fs::write 同样阻塞且错误无法返回前端；
    // spawn_blocking 既能隔离阻塞、又能把对话框取消/写入错误正常回传给调用方。
    let png_bytes = png_bytes.clone();
    tokio::task::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let save_path = app_handle
            .dialog()
            .file()
            .add_filter("PNG 图片", &["png"])
            .set_file_name("image.png")
            .blocking_save_file();
        if let Some(path) = save_path {
            let path = path.as_path().ok_or("无效路径")?;
            std::fs::write(path, &png_bytes).map_err(|e| e.to_string())?;
            log::info!("Image preview saved to {}", path.display());
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())?
}
