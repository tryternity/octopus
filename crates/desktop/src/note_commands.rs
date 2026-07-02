//! 记事本 Tauri 命令层：薄封装转调 octopus-notepad，写操作成功后 emit("notepad://changed")。
//! 图片 BLOB 桥接：notepad 不依赖 clipboard，图片获取/入库由本层桥接。

use base64::{engine::general_purpose, Engine};
use tauri::Emitter;

use octopus_notepad::{Note, NoteFilter, NoteSource, NoteType};

// ── 基础 CRUD ──

#[tauri::command]
pub async fn list_notes(
    source: Option<String>,
    favorite: Option<bool>,
    pinned: Option<bool>,
    search: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Note>, String> {
    let filter = NoteFilter {
        source: source.as_deref().map(NoteSource::from_str),
        favorite: favorite.unwrap_or(false),
        pinned: pinned.unwrap_or(false),
        search,
        limit: limit.unwrap_or(50),
        offset: offset.unwrap_or(0),
    };
    octopus_infra::db::with_db(|conn| octopus_notepad::store::list_notes_at(conn, &filter))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn count_notes(
    source: Option<String>,
    favorite: Option<bool>,
    pinned: Option<bool>,
    search: Option<String>,
) -> Result<i64, String> {
    let filter = NoteFilter {
        source: source.as_deref().map(NoteSource::from_str),
        favorite: favorite.unwrap_or(false),
        pinned: pinned.unwrap_or(false),
        search,
        limit: 1,
        offset: 0,
    };
    octopus_infra::db::with_db(|conn| octopus_notepad::store::count_notes_at(conn, &filter))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_note(id: i64) -> Result<Option<Note>, String> {
    octopus_infra::db::with_db(|conn| octopus_notepad::store::get_note_at(conn, id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_note(
    source: String,
    source_ref_id: Option<i64>,
    body: String,
    note_type: String,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let id = octopus_infra::db::with_db(|conn| {
        octopus_notepad::store::create_note_at(
            conn,
            NoteSource::from_str(&source),
            source_ref_id,
            &body,
            NoteType::from_str(&note_type),
        )
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(id)
}

#[tauri::command]
pub async fn update_note(
    id: i64,
    title: String,
    body: String,
    note_type: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_notepad::store::update_note_at(conn, id, &title, &body, NoteType::from_str(&note_type))
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(())
}

#[tauri::command]
pub async fn delete_notes(ids: Vec<i64>, app_handle: tauri::AppHandle) -> Result<usize, String> {
    let n =
        octopus_infra::db::with_db(|conn| octopus_notepad::store::delete_notes_at(conn, &ids))
            .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(n)
}

#[tauri::command]
pub async fn toggle_note_pinned(id: i64, app_handle: tauri::AppHandle) -> Result<(), String> {
    octopus_notepad::store::toggle_pinned(id).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(())
}

#[tauri::command]
pub async fn toggle_note_favorite(id: i64, app_handle: tauri::AppHandle) -> Result<(), String> {
    octopus_notepad::store::toggle_favorite(id).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(())
}

// ── 导入/导出 ──

#[tauri::command]
pub async fn export_note(stem: String, ext: String, content: String) -> Result<String, String> {
    let path = octopus_notepad::export::write_export(&stem, &ext, &content)
        .map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn import_note_from_file(path: String) -> Result<String, String> {
    octopus_notepad::export::read_import(std::path::Path::new(&path)).map_err(|e| e.to_string())
}

// ── 图片桥接（notepad 不依赖 clipboard）──

/// 取笔记内嵌图片：hash → image_data BLOB → data:image/webp;base64,...
#[tauri::command]
pub async fn get_note_image(hash: String) -> Result<String, String> {
    let blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;
    Ok(format!(
        "data:image/webp;base64,{}",
        general_purpose::STANDARD.encode(&blob)
    ))
}

/// 编辑器插入图片：选中图片文件 → 编码 WebP + 缩略图 + sha256(PNG) 入库 → 返回 hash。
#[tauri::command]
pub async fn insert_note_image(path: String) -> Result<String, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("读取图片失败: {}", e))?;
    let img = ::image::load_from_memory(&bytes).map_err(|e| format!("解码图片失败: {}", e))?;
    // image_data.hash 约定 = sha256(PNG bytes)（见 db.sql image_data 注释 + clipboard encode_and_hash）。
    let encoded = octopus_clipboard::image::encode_to_webp(&img).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (_png_bytes, hash) = octopus_clipboard::image::encode_and_hash(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    )
    .map_err(|e| e.to_string())?;
    let width = img.width() as i64;
    let height = img.height() as i64;
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::insert_image_data(
            conn,
            &hash,
            &encoded.webp_blob,
            &encoded.thumb_blob,
            width,
            height,
        )
    })
    .map_err(|e| e.to_string())?;
    Ok(hash)
}

// ── 集成入口：识别结果 → 笔记 ──

/// 语音结果 → 新建笔记：text 原文直存（type=text，不再 <p> 包裹）。
/// transcription_id 作 source_ref_id 溯源（best-effort）。
#[tauri::command]
pub async fn save_transcription_to_note(
    transcription_id: i64,
    text: String,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let id = octopus_notepad::store::create_note(NoteSource::Asr, Some(transcription_id), &text, NoteType::Text)
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(id)
}

/// OCR 结果 → 新建笔记：text 原文直存（type=text，不再 <p> 包裹）。
#[tauri::command]
pub async fn save_ocr_to_note(text: String, app_handle: tauri::AppHandle) -> Result<i64, String> {
    let id = octopus_notepad::store::create_note(NoteSource::Ocr, None, &text, NoteType::Text)
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(id)
}
