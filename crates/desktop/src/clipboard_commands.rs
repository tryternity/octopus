use std::sync::Arc;
use tauri::State;
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
