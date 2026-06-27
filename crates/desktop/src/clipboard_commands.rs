use std::sync::Arc;
use tauri::{Manager, State};
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
pub async fn toggle_clipboard_favorite(id: i64) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::toggle_favorite(conn, id)
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_clipboard_item(id: i64) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::delete_item(conn, id)
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clear_clipboard_history(keep_favorite: bool) -> Result<usize, String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::clear_history(conn, keep_favorite)
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn copy_clipboard_item(
    id: i64,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<(), String> {
    let content = octopus_infra::db::with_db(|conn| {
        let items = octopus_clipboard::store::query_history(conn, &QueryFilter {
            filter: "all".into(),
            search: None,
            page: 1,
            size: 1000,
        })?;
        Ok::<_, anyhow::Error>(items)
    })
    .map_err(|e| e.to_string())?;

    // 找到对应 id 的条目
    if let Some(item) = content.into_iter().find(|i| i.id == id) {
        if item.item_type == octopus_clipboard::ItemType::Text {
            handle.write_text(&item.content).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn clipboard_stats() -> Result<i64, String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::count_all(conn)
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
    // 1. 从 DB 按 id 读条目内容
    let content = octopus_infra::db::with_db(|conn| {
        let items = octopus_clipboard::store::query_history(conn, &octopus_clipboard::QueryFilter {
            filter: "all".into(),
            search: None,
            page: 1,
            size: 1000,
        })?;
        Ok::<_, anyhow::Error>(items)
    })
    .map_err(|e| e.to_string())?;

    let item = content.into_iter().find(|i| i.id == id);
    if item.is_none() {
        return Ok(());
    }
    let item = item.unwrap();

    // 2. hide 剪贴板窗口
    let win = app_handle.get_webview_window("clipboard_window");
    if let Some(w) = &win {
        let _ = w.hide();
    }
    drop(win);

    // 3. 恢复焦点 + 粘贴（同一线程）
    let text = item.content;
    let handle = handle.inner().clone();
    let focus = focus.inner().clone();
    std::thread::spawn(move || {
        // 1. 写剪贴板
        let _ = handle.write_text(&text);
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
        let items = octopus_clipboard::store::query_history(conn, &QueryFilter {
            filter: "all".into(),
            search: None,
            page: 1,
            size: 1000,
        })?;
        Ok::<_, anyhow::Error>(items.into_iter().find(|i| i.id == id))
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;

    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
        .ok_or("图片元数据缺失")?;

    // 2. 读原图 PNG 字节
    let orig_path = octopus_clipboard::image::clipboard_images_dir()
        .join(format!("{}.png", blob_hash));
    let png_bytes = std::fs::read(&orig_path).map_err(|e| e.to_string())?;

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

    // 5. 编码写入
    match ext {
        "png" => octopus_infra::image_util::save_as_png(&png_bytes, &save_path),
        "webp" => octopus_infra::image_util::save_as_webp(&png_bytes, &save_path, q),
        _ => octopus_infra::image_util::save_as_jpeg(&png_bytes, &save_path, q),
    }
    .map_err(|e| e.to_string())?;

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

/// 文件条目：用系统默认应用打开第一个文件
#[tauri::command]
pub async fn open_file_item(id: i64) -> Result<(), String> {
    let item = octopus_infra::db::with_db(|conn| {
        let items = octopus_clipboard::store::query_history(conn, &QueryFilter {
            filter: "all".into(),
            search: None,
            page: 1,
            size: 1000,
        })?;
        Ok::<_, anyhow::Error>(items.into_iter().find(|i| i.id == id))
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
    let path = first.strip_prefix("file://").unwrap_or(first);

    // 用系统默认应用打开
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(path).spawn()
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
        let items = octopus_clipboard::store::query_history(conn, &QueryFilter {
            filter: "all".into(),
            search: None,
            page: 1,
            size: 1000,
        })?;
        Ok::<_, anyhow::Error>(items.into_iter().find(|i| i.id == id))
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
        .ok_or("图片元数据缺失")?;

    let orig_path = octopus_clipboard::image::clipboard_images_dir()
        .join(format!("{}.png", blob_hash));
    let png_bytes = std::fs::read(&orig_path).map_err(|e| e.to_string())?;

    let engine = octopus_ocr::engine::OcrEngine::instance()
        .map_err(|e| e.to_string())?;
    let text = engine.recognize(&png_bytes).map_err(|e| e.to_string())?;

    if text.trim().is_empty() {
        return Err("未识别到文本".into());
    }

    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_search_text(conn, id, &text)
    }).map_err(|e| e.to_string())?;

    handle.write_text(&text).map_err(|e| e.to_string())?;

    open_text_editor_with_content(&text);

    Ok(text)
}

/// 用系统文本编辑器新建无标题文档（不落盘临时文件）。
fn open_text_editor_with_content(text: &str) {
    #[cfg(target_os = "macos")]
    {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            r#"tell application "TextEdit"
    activate
    make new document with properties {{text:"{}"}}
end tell"#,
            escaped
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("notepad").spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg("text://").spawn();
    }
}
