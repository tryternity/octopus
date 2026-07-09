//! 热词管理后端命令——CRUD + 挖掘 + 纠错索引 reload。

use octopus_infra::db::{self, Hotword};

/// 写库后统一 reload corrector 热词索引（active 词表）。
/// 失败仅告警，不阻断写操作（下次启动会重新装载）。
fn reload_after_write() {
    match db::list_active_hotword_words() {
        Ok(words) => octopus_asr_local::corrector::reload_hotwords(words),
        Err(e) => log::warn!("[hotword] reload 失败: {}", e),
    }
}

#[tauri::command]
pub fn list_hotwords(status: String) -> Result<Vec<Hotword>, String> {
    db::list_hotwords(&status).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_hotword(word: String) -> Result<i64, String> {
    let id = db::insert_hotword(&word, "manual", "active").map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(id)
}

#[tauri::command]
pub fn confirm_pending_hotword(id: i64) -> Result<(), String> {
    db::confirm_pending_hotword(id).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

#[tauri::command]
pub fn delete_hotword(id: i64) -> Result<(), String> {
    db::delete_hotword(id).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

/// 触发挖掘：扫历史 ASR 文本挖低频高频专名 → 写 pending（见 miner，Task 7 实现）。
#[tauri::command]
pub fn mine_hotword_candidates() -> Result<usize, String> {
    let n = octopus_asr_local::miner::mine_pending_candidates().map_err(|e| e.to_string())?;
    Ok(n)
}
