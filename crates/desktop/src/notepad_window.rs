//! 记事本入口：改走 egui 独立进程（本地 TCP IPC），不再建 webview。
//! open_notepad / open_notepad_with_note 调 egui_ipc（连不上则 spawn）。

/// 打开记事本（egui 进程：已运行则 show，未运行则 spawn）。
#[tauri::command]
pub fn open_notepad(_app_handle: tauri::AppHandle) {
    crate::egui_ipc::show();
}

/// 打开记事本并选中指定笔记（OCR 识别结果存笔记后调用）。
#[tauri::command]
pub fn open_notepad_with_note(_app_handle: tauri::AppHandle, note_id: i64) {
    crate::egui_ipc::open_note(note_id);
}
