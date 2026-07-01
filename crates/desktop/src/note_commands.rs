//! 记事本集成入口：识别结果 → 笔记。
//! 其余 CRUD（list/get/create/update/delete/toggle/export/import/image）已废弃——
//! egui 进程直连 octopus_notepad::store，不走 invoke。仅留这 2 个 Tauri 命令
//! 供 OCR/ASR 识别后调用：写笔记（type='text'）+ IPC 通知 egui 刷新。

use octopus_notepad::{NoteSource, NoteType};

/// 语音结果 → 新建笔记（type='text'，纯文本无 `<p>` 包裹）+ IPC 通知 egui。
///
/// IPC 的 send() 带最多 ~2s spawn 重试，同步调用会阻塞 async 命令线程；
/// 故写库后立即返回 id，IPC 通知 fire-and-forget 到独立线程（两条消息同线程保序）。
#[tauri::command]
pub async fn save_transcription_to_note(
    transcription_id: i64,
    text: String,
    _app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let id = octopus_notepad::store::create_note(
        NoteSource::Asr,
        Some(transcription_id),
        &text,
        NoteType::Text,
    )
    .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        crate::egui_ipc::notes_changed();
        crate::egui_ipc::open_note(id);
    });
    Ok(id)
}

/// OCR 结果 → 新建笔记（type='text'）+ IPC 通知 egui（fire-and-forget）。
#[tauri::command]
pub async fn save_ocr_to_note(text: String, _app_handle: tauri::AppHandle) -> Result<i64, String> {
    let id = octopus_notepad::store::create_note(NoteSource::Ocr, None, &text, NoteType::Text)
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        crate::egui_ipc::notes_changed();
        crate::egui_ipc::open_note(id);
    });
    Ok(id)
}
