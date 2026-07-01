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
    octopus_infra::db::with_db(|conn| {
        let qf = QueryFilter {
            filter,
            search,
            page: page.max(1),
            size: size.max(1),
        };
        octopus_clipboard::store::query_history(conn, &qf)
    })
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

/// 按条目类型把内容写到系统剪贴板（copy / paste 共用）：
/// - Text: write_text(content)
/// - Image: 从 image_data 读 WebP 原图 → 转 PNG → write_image（还原为真实图片，
///   而非把 blob_hash 当文本写入）
/// - File: 解析 content（JSON 路径数组）→ write_files（还原为真实文件，
///   而非把 JSON 字符串当文本写入）
fn write_item_to_clipboard(handle: &ClipboardHandle, item: &ClipboardItem) -> Result<(), String> {
    match item.item_type {
        octopus_clipboard::ItemType::Text => handle.write_text(&item.content).map_err(|e| e.to_string()),
        octopus_clipboard::ItemType::Image => {
            let blob_hash = item.image_meta.as_ref()
                .map(|m| m.blob_hash.clone())
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
            img.write_to(&mut std::io::Cursor::new(&mut png), ::image::ImageFormat::Png)
                .map_err(|e| format!("编码 PNG 失败: {}", e))?;
            handle.write_image(&png).map_err(|e| e.to_string())
        }
        octopus_clipboard::ItemType::File => {
            let paths: Vec<String> = serde_json::from_str(&item.content)
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
        write_item_to_clipboard(&handle, &item)?;
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
    octopus_infra::db::with_db(|conn| {
        let qf = QueryFilter {
            filter,
            search,
            page: 1,
            size: 1,
        };
        octopus_clipboard::store::count_history(conn, &qf)
    })
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

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
        .ok_or("图片元数据缺失")?;

    // 2. 从 DB 读 WebP 无损 BLOB
    let webp_blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;

    // 3. 目标目录 ~/Downloads/octopus/
    let downloads_dir = dirs::download_dir()
        .unwrap_or_else(|| dirs::home_dir().expect("no home dir"))
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

    // 5. 编码写入（从 WebP BLOB 转码到目标格式）
    match ext {
        "png" => {
            let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
                .map_err(|e| e.to_string())?;
            img.save_with_format(&save_path, ::image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
        }
        "webp" => {
            std::fs::write(&save_path, &webp_blob).map_err(|e| e.to_string())?;
        }
        _ => {
            let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
                .map_err(|e| e.to_string())?;
            let rgb = img.to_rgb8();
            let mut buf = std::io::BufWriter::new(
                std::fs::File::create(&save_path).map_err(|e| e.to_string())?
            );
            let mut encoder = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
            encoder.encode(&rgb, rgb.width(), rgb.height(), ::image::ExtendedColorType::Rgb8)
                .map_err(|e| e.to_string())?;
        }
    }

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
/// 仅 `file://` 开头才解码，避免对含字面 `%XX` 的普通路径误伤（如 `50%20off.txt`）。
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

    // 解析 JSON 路径数组
    let paths: Vec<String> = serde_json::from_str(&item.content)
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

/// 图片条目 OCR：识别文本 → 写 search_text + 写剪贴板 + 新建文档。
#[tauri::command]
pub async fn ocr_image(
    id: i64,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, id)
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
        .ok_or("图片元数据缺失")?;

    let webp_blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;

    let engine = octopus_ocr::engine::OcrEngine::instance()
        .map_err(|e| e.to_string())?;
    let text = engine.recognize(&webp_blob).map_err(|e| e.to_string())?;

    if text.trim().is_empty() {
        return Err("未识别到文本".into());
    }

    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_search_text(conn, id, &text)
    }).map_err(|e| e.to_string())?;

    handle.write_text(&text).map_err(|e| e.to_string())?;

    Ok(text)
}

/// 精简编辑器回写：更新剪贴板条目文本（content + search_text）并同步系统剪贴板。
/// OCR 编辑、剪贴板文本条目编辑两处共用。
#[tauri::command]
pub async fn set_clipboard_item_text(
    item_id: i64,
    text: String,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_content(conn, item_id, &text)
    })
    .map_err(|e| e.to_string())?;

    handle.write_text(&text).map_err(|e| e.to_string())?;
    Ok(())
}

/// 获取图片缩略图 data URL（base64 编码：`data:image/webp;base64,...`）。
///
/// 返回完整 data URL 而非裸 `Vec<u8>`：Tauri IPC 把 `Vec<u8>` 序列化成 JSON 数字数组
/// （4-5x 膨胀），前端还要 `map/join/btoa` 手动转 base64。后端一次编码成 data URL，
/// 前端直接 `<img src={...}>`，省掉膨胀与转换开销（剪贴板窗口滚动时每个图片条目都触发）。
#[tauri::command]
pub async fn get_image_thumb(id: i64) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, id)
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
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
}

/// 取图片全分辨率（image_data.blob）→ data URL（base64 + WebP 前缀）。
///
/// 前端 ImagePreview 用它加载到 <img>/canvas 做标注。镜像 get_image_thumb，
/// 仅取 blob（全分辨率）而非 thumb。返回 data URL 同样为避免 IPC 序列化膨胀。
#[tauri::command]
pub async fn get_image_full(id: i64) -> Result<String, String> {
    let item = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_item_by_id(conn, id)
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
        .ok_or("图片元数据缺失")?;

    let blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "图片数据缺失".to_string())?;

    Ok(format!(
        "data:image/webp;base64,{}",
        general_purpose::STANDARD.encode(&blob)
    ))
}

/// 弹系统保存对话框，把前端合成的标注 PNG（base64）存到用户指定路径。
///
/// 镜像 screenshot_commands::save_screenshot_dialog，去掉截图专属清理
/// （ALL_CAPTURES / close_all_screenshot_windows）。预览窗保持打开。
#[tauri::command]
pub async fn save_image_dialog(
    png_base64: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let png_bytes = general_purpose::STANDARD
        .decode(&png_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;

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
    Ok(())
}

/// 把前端合成的标注 PNG（base64）写入系统剪贴板。
///
/// ClipboardHandle::write_image 内部已 from_bytes + set_image，无需直接碰 RustImageData。
#[tauri::command]
pub async fn copy_image_to_clipboard(
    png_base64: String,
    handle: State<'_, Arc<ClipboardHandle>>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let png_bytes = general_purpose::STANDARD
        .decode(&png_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;
    handle.write_image(&png_bytes).map_err(|e| e.to_string())?;
    // write_image 置 suppress flag，watcher 会跳过自身写入（防回环）。
    // 但图片预览的「复制」期望这条图进入剪贴板历史 → 主动调 watcher 的入库逻辑
    // （与系统复制图片走完全相同的路径：去重 hash + WebP + 缩略图 + image_data BLOB）。
    octopus_clipboard::watcher::handle_clipboard_change(handle.inner().as_ref());
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(())
}
