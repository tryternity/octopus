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

/// 前端展示用——在 Hotword 基础上附加拼音首字母串（供搜索/排序）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotwordView {
    id: i64,
    word: String,
    status: String,
    source: String,
    hit_count: i64,
    created_at: String,
    /// 拼音首字母串（大写），前端拼音首字母搜索/排序用
    initials: String,
}

impl From<Hotword> for HotwordView {
    fn from(h: Hotword) -> Self {
        HotwordView {
            initials: octopus_asr_local::hotword::pinyin_initials(&h.word),
            id: h.id,
            word: h.word,
            status: h.status,
            source: h.source,
            hit_count: h.hit_count,
            created_at: h.created_at,
        }
    }
}

#[tauri::command]
pub fn list_hotwords(status: String) -> Result<Vec<HotwordView>, String> {
    db::list_hotwords(&status)
        .map_err(|e| e.to_string())
        .map(|list| list.into_iter().map(HotwordView::from).collect())
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
