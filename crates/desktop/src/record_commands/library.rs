//! 录屏库管理命令子模块（macOS 独占）。
//!
//! 从 record_commands/mod.rs 拆出（Task 1.2）。包含：
//! - ListRecordingsParams + list/get/thumbnail/rename/favorite/delete/open/reveal recordings
//! - RecordStatus + get_record_status（RecordControl 浮窗初始化用）

#![cfg(target_os = "macos")]

use octopus_record::{ListFilter, RecordError, RecordSession, RecordStore, RecordingMeta};
use tauri::{command, State};

use crate::error_util::e2s;
use super::with_db_blocking;

/// list_recordings 的前端参数：单独定义而非直接收 ListFilter，
/// 因为 ListFilter 在 record/store.rs 只 derive Debug/Clone/Default，无 Deserialize。
/// serde 从前端 JSON 反序列化到此 struct，再转 ListFilter。
#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub struct ListRecordingsParams {
    pub limit: u32,
    pub offset: u32,
    pub favorites_only: bool,
}

impl From<ListRecordingsParams> for ListFilter {
    fn from(p: ListRecordingsParams) -> Self {
        ListFilter {
            limit: p.limit,
            offset: p.offset,
            favorites_only: p.favorites_only,
        }
    }
}

#[command]
pub async fn list_recordings(
    filter: Option<ListRecordingsParams>,
) -> Result<Vec<RecordingMeta>, String> {
    let filter: ListFilter = filter.unwrap_or_default().into();
    with_db_blocking(move |conn| RecordStore::new(conn).list(&filter)).await
}

#[command]
pub async fn get_recording(id: i64) -> Result<RecordingMeta, String> {
    with_db_blocking(move |conn| {
        RecordStore::new(conn)
            .get(id)?
            .ok_or(RecordError::NotFound(id))
    })
    .await
}

#[command]
pub async fn get_recording_thumbnail(id: i64) -> Result<Option<Vec<u8>>, String> {
    with_db_blocking(move |conn| RecordStore::new(conn).get_thumbnail(id)).await
}

#[command]
pub async fn rename_recording(id: i64, title: String) -> Result<(), String> {
    with_db_blocking(move |conn| RecordStore::new(conn).rename(id, &title)).await
}

#[command]
pub async fn toggle_recording_favorite(id: i64) -> Result<(), String> {
    with_db_blocking(move |conn| RecordStore::new(conn).toggle_favorite(id)).await
}

#[command]
pub async fn delete_recording(id: i64, permanent: bool) -> Result<(), String> {
    use octopus_infra::paths::resolve_recording_path;

    // permanent=true：物理删 DB 行 + 磁盘文件（含 .srt 字幕文件）
    // permanent=false：仅删 DB 行（磁盘文件保留，下次启动 cleanup 孤儿清理会删）
    if permanent {
        // 先查 meta 拿路径 → 删文件 + 关联 .srt → 删 DB 行（顺序避免删文件后 DB 失败留孤儿）
        let file_path = with_db_blocking(move |conn| {
            let store = RecordStore::new(conn);
            let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
            Ok::<_, RecordError>(meta.file_path)
        })
        .await?;
        let abs = resolve_recording_path(&file_path);
        if abs.exists() {
            std::fs::remove_file(&abs).map_err(e2s)?;
        }
        // 删关联的 .N.srt 字幕文件（与 mp4 同目录同名，所有版本）
        if let Some(dir) = abs.parent() {
            if let Some(stem) = abs.file_stem().and_then(|s| s.to_str()) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    // 匹配 <stem>.N.srt
                    if name.starts_with(&format!("{}.", stem)) && name.ends_with(".srt") {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
            }
        }
    }
    with_db_blocking(move |conn| RecordStore::new(conn).delete_db_row(id)).await
}

/// 用系统默认应用打开录屏文件（QuickTime Player）。
///
/// 与 clipboard_commands::open_file_item 同模式：std::process::Command::new("open")，
/// 不用 opener crate（项目惯例）。
#[command]
pub async fn open_recording_file(id: i64) -> Result<(), String> {
    use octopus_infra::paths::resolve_recording_path;
    let file_path = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
        Ok::<_, RecordError>(meta.file_path)
    })
    .await?;
    let abs = resolve_recording_path(&file_path);
    crate::platform::sys_open::open_with_default(&abs.to_string_lossy())?;
    Ok(())
}

/// 在文件管理器中定位录屏文件（macOS Finder / Windows Explorer / Linux xdg-open）。
///
/// macOS: `open -R` 让 Finder 高亮选中文件；与 search_commands::reveal_path 一致。
/// 不用 NSWorkspace activateFileViewerSelecting（spec §9.2 F13 推迟项）。
#[command]
pub async fn reveal_recording(id: i64) -> Result<(), String> {
    use octopus_infra::paths::resolve_recording_path;
    let file_path = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
        Ok::<_, RecordError>(meta.file_path)
    })
    .await?;
    let abs = resolve_recording_path(&file_path);
    crate::platform::sys_open::reveal_path(&abs)?;
    Ok(())
}

/// 查询当前录制状态 + 已录秒数。
///
/// 用途：RecordControl 浮窗 mount 时初始化——浮窗创建晚于 recording-started 事件，
/// 收不到事件，靠此命令拿当前 state + elapsed_secs 启动计时器。
/// 返回 {state: "idle"/"recording"/"paused"/..., elapsedSecs: u64}。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStatus {
    pub state: String,
    pub elapsed_secs: u64,
}

#[command]
pub async fn get_record_status(state: State<'_, RecordSession>) -> Result<RecordStatus, String> {
    let s = state.state().await;
    let elapsed = state.elapsed_secs().await.unwrap_or(0);
    Ok(RecordStatus {
        state: format!("{:?}", s).to_lowercase(),
        elapsed_secs: elapsed,
    })
}

