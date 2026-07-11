//! 热词版本管理后端命令——版本 CRUD + 单词增删 + 导入导出 + 挖掘到版本。
//! 底层：hotword_sets（版本纯文本）+ hotword_hits（全局命中）。

use octopus_infra::db::{self, HotwordSet};

/// 写库后统一 reload corrector 热词索引（enabled 版本并集）。
/// 失败仅告警，不阻断写操作（下次启动会重新装载）。
fn reload_after_write() {
    match db::list_active_hotword_words() {
        Ok(words) => octopus_asr_local::corrector::reload_hotwords(words),
        Err(e) => log::warn!("[hotword] reload 失败: {}", e),
    }
}

// ── 版本 CRUD ──

#[tauri::command]
pub fn list_hotword_sets() -> Result<Vec<HotwordSet>, String> {
    db::list_hotword_sets().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_hotword_set(name: String) -> Result<i64, String> {
    let id = db::insert_hotword_set(&name).map_err(|e| e.to_string())?;
    Ok(id)
}

#[tauri::command]
pub fn rename_hotword_set(id: i64, name: String) -> Result<(), String> {
    db::rename_hotword_set(id, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_hotword_set(id: i64) -> Result<(), String> {
    db::delete_hotword_set(id).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

#[tauri::command]
pub fn toggle_hotword_set(id: i64, enabled: bool) -> Result<(), String> {
    db::toggle_hotword_set(id, enabled).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

// ── 单词增删（系统透明维护 words_text）──

#[tauri::command]
pub fn add_word_to_set(id: i64, word: String) -> Result<bool, String> {
    let added = db::add_word_to_set(id, &word).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(added)
}

#[tauri::command]
pub fn remove_word_from_set(id: i64, word: String) -> Result<(), String> {
    db::remove_word_from_set(id, &word).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(())
}

// ── 全局命中 ──

#[tauri::command]
pub fn list_hotword_hits() -> Result<std::collections::HashMap<String, i64>, String> {
    db::list_hotword_hits().map_err(|e| e.to_string())
}

// ── 挖掘候选（不落库，前端确认后再批量 add_words_to_set）──

/// 挖掘候选词列表（扫历史 + jieba + 词频过滤），不写库。前端展示候选供用户勾选确认。
#[tauri::command]
pub fn list_hotword_candidates() -> Result<Vec<String>, String> {
    octopus_asr_local::miner::collect_candidate_words().map_err(|e| e.to_string())
}

/// 批量追加多词到指定版本（挖掘确认 / 手动批量）。返回实际新增条数。
#[tauri::command]
pub fn add_words_to_set(id: i64, words: Vec<String>) -> Result<usize, String> {
    let added = db::add_words_to_set(id, &words).map_err(|e| e.to_string())?;
    reload_after_write();
    Ok(added)
}

// ── 导入 / 导出（照搬 save_image_dialog 的 spawn_blocking + dialog 范式）──

/// 导入 txt：mode = "new"（新建版本，需 new_name）/ "append"（追加到 target_set_id）
/// / "overwrite"（覆盖 target_set_id 的 words_text）。返回目标版本 id。
#[tauri::command]
pub async fn import_hotwords(
    app_handle: tauri::AppHandle,
    mode: String,
    target_set_id: Option<i64>,
    new_name: Option<String>,
) -> Result<i64, String> {
    tokio::task::spawn_blocking(move || -> Result<i64, String> {
        use tauri_plugin_dialog::DialogExt;
        let path = app_handle
            .dialog()
            .file()
            .add_filter("文本", &["txt"])
            .blocking_pick_file();
        let Some(path) = path else {
            return Err("未选择文件".into());
        };
        let path = path.as_path().ok_or("无效路径")?;
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;

        match mode.as_str() {
            "new" => {
                let name = new_name.unwrap_or_else(|| "导入版本".to_string());
                let id = db::insert_hotword_set(&name).map_err(|e| e.to_string())?;
                db::set_hotword_set_words(id, &content).map_err(|e| e.to_string())?;
                reload_after_write();
                Ok(id)
            }
            "append" => {
                let id = target_set_id.ok_or("append 模式需 target_set_id")?;
                let words: Vec<String> = content.split_whitespace().map(|s| s.to_string()).collect();
                db::add_words_to_set(id, &words).map_err(|e| e.to_string())?;
                reload_after_write();
                Ok(id)
            }
            "overwrite" => {
                let id = target_set_id.ok_or("overwrite 模式需 target_set_id")?;
                db::set_hotword_set_words(id, &content).map_err(|e| e.to_string())?;
                reload_after_write();
                Ok(id)
            }
            other => Err(format!("未知导入模式: {}", other)),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 导出某版本 words_text 到 txt（用户选保存路径）。
#[tauri::command]
pub async fn export_hotwords(app_handle: tauri::AppHandle, set_id: i64) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let set = db::get_hotword_set(set_id).map_err(|e| e.to_string())?;
        use tauri_plugin_dialog::DialogExt;
        let save_path = app_handle
            .dialog()
            .file()
            .add_filter("文本", &["txt"])
            .set_file_name(format!("{}.txt", set.name))
            .blocking_save_file();
        if let Some(path) = save_path {
            let path = path.as_path().ok_or("无效路径")?;
            std::fs::write(path, &set.words_text).map_err(|e| e.to_string())?;
            log::info!("[hotword] 导出版本「{}」到 {}", set.name, path.display());
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}
