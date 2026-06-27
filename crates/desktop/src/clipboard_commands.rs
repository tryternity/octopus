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

/// 图片条目：保存原图为文件 + 写文件绝对路径到剪贴板。
/// format: "png" | "webp" | "jpeg"（决定对话框 filter 和编码方式）
/// quality: 1-100，仅 webp/jpeg 生效，png 忽略
#[tauri::command]
pub async fn save_image_item(
    id: i64,
    format: String,
    quality: Option<u8>,
    app_handle: tauri::AppHandle,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<String, String> {
    let fmt = format.to_lowercase();
    let q = quality.unwrap_or(90).clamp(1, 100);

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

    // 3. 弹保存对话框（仅显示用户选择的格式）
    let ext = match fmt.as_str() {
        "png" => "png",
        "jpeg" | "jpg" => "jpg",
        _ => "webp",
    };
    let filter_label = match ext {
        "png" => "PNG 图片",
        "jpg" => "JPEG 图片",
        _ => "WebP 图片",
    };
    let default_name = format!("{}.{}", &blob_hash[..8.min(blob_hash.len())], ext);
    use tauri_plugin_dialog::DialogExt;
    let save_path = app_handle.dialog()
        .file()
        .add_filter(filter_label, &[ext])
        .set_file_name(&default_name)
        .blocking_save_file();

    let save_path = save_path.ok_or("用户取消")?;
    let save_path = save_path.as_path().ok_or("无效路径")?;

    // 4. 按格式+质量保存
    match ext {
        "png" => octopus_infra::image_util::save_as_png(&png_bytes, save_path),
        "jpg" => octopus_infra::image_util::save_as_jpeg(&png_bytes, save_path, q),
        _ => octopus_infra::image_util::save_as_webp(&png_bytes, save_path, q),
    }
    .map_err(|e| e.to_string())?;

    // 5. 写文件绝对路径到剪贴板
    let abs_path = save_path.to_string_lossy().to_string();
    handle.write_text(&abs_path).map_err(|e| e.to_string())?;

    Ok(abs_path)
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
