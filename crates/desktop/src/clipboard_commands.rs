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
            size: 1,
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

    // 2. 写剪贴板（设 suppress flag 防 watcher 重复记录）
    if item.item_type == octopus_clipboard::ItemType::Text {
        handle.write_text(&item.content).map_err(|e| e.to_string())?;
    } else {
        return Ok(()); // 非 text 暂不支持自动粘贴
    }

    // 3. hide 剪贴板窗口
    let win = app_handle.get_webview_window("clipboard_window");
    if let Some(w) = &win {
        log::info!("paste_clipboard_item: hiding clipboard window");
        let _ = w.hide();
    }
    drop(win);

    // 4. 用 paste::paste（Clipboard 方式）——复用 ASR 粘贴的完整逻辑
    //    hide 后 macOS 自动还焦点 → paste 写剪贴板 + enigo Cmd+V
    let handle = handle.inner().clone();
    tokio::task::spawn_blocking(move || {
        // 等焦点回到上一个应用
        std::thread::sleep(std::time::Duration::from_millis(300));
        // 复用 paste.rs 的 paste_via_clipboard（写剪贴板 + Cmd+V + 不恢复原剪贴板）
        let config = crate::config::AppConfig {
            write_to_clipboard: true,
            paste_method: "clipboard".into(),
            ..Default::default()
        };
        // paste::paste 内部会 write_text（设 suppress）+ Cmd+V
        // 但我们已经在前面写过了剪贴板，这里直接调 paste_via_clipboard 会再写一次
        // 更简单：直接模拟 Cmd+V（剪贴板已有内容）
        log::info!("paste_clipboard_item: enigo Cmd+V via paste module");
        use enigo::{Direction, Enigo, Key, Keyboard, Settings};
        match Enigo::new(&Settings::default()) {
            Ok(mut enigo) => {
                let mod_key = Key::Meta;
                let v_key = Key::Other(9);
                let _ = enigo.key(mod_key, Direction::Press);
                let _ = enigo.key(v_key, Direction::Click);
                let _ = enigo.key(mod_key, Direction::Release);
                log::info!("paste_clipboard_item: Cmd+V done");
            }
            Err(e) => log::warn!("paste_clipboard_item: enigo failed: {}", e),
        }
        let _ = handle; // 保持引用，确保 suppress flag 不被提前清理
    });

    Ok(())
}
