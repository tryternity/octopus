//! 热词版本管理后端命令——版本 CRUD + 单词增删 + 导入导出 + 挖掘到版本。
//! 底层：hotword_sets（版本纯文本，v46 id 改 UUID 字符串）+ hotword_hits（全局命中）。

use octopus_infra::db::{self, HotwordSet};
use uuid::Uuid;
use crate::error_util::e2s;

/// 写库后统一 reload corrector 热词索引（enabled 版本并集）。
/// 失败仅告警，不阻断写操作（下次启动会重新装载）。
fn reload_after_write() {
    match db::list_active_hotword_words() {
        Ok(words) => octopus_asr_local::corrector::reload_hotwords(words),
        Err(e) => log::warn!("[hotword] reload 失败: {}", e),
    }
}

/// 写操作后回填 sync_md5——读完整 row 算 md5 再 update。
///
/// 为什么读 row 而非用命令参数算：words_text 在 DB 内 normalize（拼音首字母排序 + 去重），
/// 命令层传入的原始词序与 DB 存的不同——读 row 拿到 normalize 后的 words_text 才能算准 md5。
///
/// 失败仅告警，不阻断写操作（sync 时会检测到 NULL 重算）。
fn refill_sync_md5(id: &str) {
    match db::get_hotword_set(id) {
        Ok(h) => {
            let md5 = octopus_sync::hotword::hotword_set_md5(&h);
            if let Err(e) = db::update_hotword_set_sync_md5(id, &md5) {
                log::warn!("[hotword] 回填 sync_md5 失败 {}: {}", id, e);
            }
        }
        Err(e) => log::warn!("[hotword] 读 row 算 md5 失败 {}: {}", id, e),
    }
}

// ── 版本 CRUD ──

#[tauri::command]
pub fn list_hotword_sets() -> Result<Vec<HotwordSet>, String> {
    db::list_hotword_sets().map_err(e2s)
}

/// 新建热词版本。v46：id 由后端生成 UUID（不再 AUTOINCREMENT），返回 String。
#[tauri::command]
pub fn create_hotword_set(name: String) -> Result<String, String> {
    let id = Uuid::new_v4().to_string();
    db::insert_hotword_set(&id, &name).map_err(e2s)?;
    refill_sync_md5(&id);
    Ok(id)
}

#[tauri::command]
pub fn rename_hotword_set(id: String, name: String) -> Result<(), String> {
    db::rename_hotword_set(&id, &name).map_err(e2s)?;
    refill_sync_md5(&id);
    Ok(())
}

#[tauri::command]
pub fn delete_hotword_set(id: String) -> Result<(), String> {
    db::delete_hotword_set(&id).map_err(e2s)?;
    reload_after_write();
    Ok(())
}

#[tauri::command]
pub fn toggle_hotword_set(id: String, enabled: bool) -> Result<(), String> {
    db::toggle_hotword_set(&id, enabled).map_err(e2s)?;
    refill_sync_md5(&id);
    reload_after_write();
    Ok(())
}

// ── 单词增删（系统透明维护 words_text）──

#[tauri::command]
pub fn add_word_to_set(id: String, word: String) -> Result<bool, String> {
    let added = db::add_word_to_set(&id, &word).map_err(e2s)?;
    refill_sync_md5(&id);
    reload_after_write();
    Ok(added)
}

#[tauri::command]
pub fn remove_word_from_set(id: String, word: String) -> Result<(), String> {
    db::remove_word_from_set(&id, &word).map_err(e2s)?;
    refill_sync_md5(&id);
    reload_after_write();
    Ok(())
}

// ── 全局命中 ──

#[tauri::command]
pub fn list_hotword_hits() -> Result<std::collections::HashMap<String, i64>, String> {
    db::list_hotword_hits().map_err(e2s)
}

// ── 挖掘候选（不落库，前端确认后再批量 add_words_to_set）──

/// 挖掘候选词列表（扫历史 + jieba + 词频过滤），不写库。前端展示候选供用户勾选确认。
#[tauri::command]
pub fn list_hotword_candidates() -> Result<Vec<String>, String> {
    octopus_asr_local::miner::collect_candidate_words().map_err(e2s)
}

/// 批量追加多词到指定版本（挖掘确认 / 手动批量）。返回实际新增条数。
#[tauri::command]
pub fn add_words_to_set(id: String, words: Vec<String>) -> Result<usize, String> {
    let added = db::add_words_to_set(&id, &words).map_err(e2s)?;
    refill_sync_md5(&id);
    reload_after_write();
    Ok(added)
}

// ── 导入 / 导出（照搬 save_image_dialog 的 spawn_blocking + dialog 范式）──

/// 导入 txt：mode = "new"（新建版本，需 new_name）/ "append"（追加到 target_set_id）
/// / "overwrite"（覆盖 target_set_id 的 words_text）。返回目标版本 id（v46: String UUID）。
#[tauri::command]
pub async fn import_hotwords(
    app_handle: tauri::AppHandle,
    mode: String,
    target_set_id: Option<String>,
    new_name: Option<String>,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || -> Result<String, String> {
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
        let content = std::fs::read_to_string(path).map_err(e2s)?;

        match mode.as_str() {
            "new" => {
                let name = new_name.unwrap_or_else(|| "导入版本".to_string());
                let id = Uuid::new_v4().to_string();
                db::insert_hotword_set(&id, &name).map_err(e2s)?;
                db::set_hotword_set_words(&id, &content).map_err(e2s)?;
                refill_sync_md5(&id);
                reload_after_write();
                Ok(id)
            }
            "append" => {
                let id = target_set_id.ok_or("append 模式需 target_set_id")?;
                let words: Vec<String> = content.split_whitespace().map(|s| s.to_string()).collect();
                db::add_words_to_set(&id, &words).map_err(e2s)?;
                refill_sync_md5(&id);
                reload_after_write();
                Ok(id)
            }
            "overwrite" => {
                let id = target_set_id.ok_or("overwrite 模式需 target_set_id")?;
                db::set_hotword_set_words(&id, &content).map_err(e2s)?;
                refill_sync_md5(&id);
                reload_after_write();
                Ok(id)
            }
            other => Err(format!("未知导入模式: {}", other)),
        }
    })
    .await
    .map_err(e2s)?
}

/// 导出某版本 words_text 到 txt（用户选保存路径）。
#[tauri::command]
pub async fn export_hotwords(app_handle: tauri::AppHandle, set_id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let set = db::get_hotword_set(&set_id).map_err(e2s)?;
        use tauri_plugin_dialog::DialogExt;
        let save_path = app_handle
            .dialog()
            .file()
            .add_filter("文本", &["txt"])
            .set_file_name(format!("{}.txt", set.name))
            .blocking_save_file();
        if let Some(path) = save_path {
            let path = path.as_path().ok_or("无效路径")?;
            std::fs::write(path, &set.words_text).map_err(e2s)?;
            log::info!("[hotword] 导出版本「{}」到 {}", set.name, path.display());
        }
        Ok(())
    })
    .await
    .map_err(e2s)?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 干净 DB（set_test_db 内部跑 INIT_SQL 建 v46 schema + 默认「通用」seed）。
    fn setup_db() {
        octopus_infra::db::set_test_db(
            rusqlite::Connection::open_in_memory().expect("in-memory DB"),
        );
    }

    /// create_hotword_set 后 sync_md5 应被回填（非 NULL），且值与 hotword_set_md5 一致。
    #[test]
    fn create_hotword_set_refills_sync_md5() {
        setup_db();
        let id = create_hotword_set("测试版本A".into()).expect("create");
        let h = db::get_hotword_set(&id).expect("get");
        assert!(h.sync_md5.is_some(), "create 后 sync_md5 应被回填");
        // 回填值应等于当前内容的 md5
        let expected = octopus_sync::hotword::hotword_set_md5(&h);
        assert_eq!(h.sync_md5.as_deref(), Some(expected.as_str()));
    }

    /// rename 后 sync_md5 应更新（name 变 → md5 变）。
    #[test]
    fn rename_hotword_set_updates_sync_md5() {
        setup_db();
        let id = create_hotword_set("原名".into()).expect("create");
        let md5_before = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();

        rename_hotword_set(id.clone(), "新名".into()).expect("rename");
        let md5_after = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();

        assert_ne!(md5_before, md5_after, "rename 后 md5 应变化");
    }

    /// toggle enabled 后 sync_md5 应更新（enabled 变 → md5 变）。
    #[test]
    fn toggle_hotword_set_updates_sync_md5() {
        setup_db();
        let id = create_hotword_set("版本".into()).expect("create");
        let md5_before = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();

        toggle_hotword_set(id.clone(), false).expect("toggle off");
        let md5_after = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();

        assert_ne!(md5_before, md5_after, "toggle enabled 后 md5 应变化");
        assert!(!db::get_hotword_set(&id).unwrap().enabled);
    }

    /// add_word / remove_word 后 sync_md5 应更新（words_text 变 → md5 变）。
    #[test]
    fn word_operations_update_sync_md5() {
        setup_db();
        let id = create_hotword_set("版本".into()).expect("create");
        let md5_empty = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();

        // add_word
        let added = add_word_to_set(id.clone(), "苹果".into()).expect("add");
        assert!(added);
        let md5_one = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();
        assert_ne!(md5_empty, md5_one, "加词后 md5 应变化");

        // 再加一词
        add_word_to_set(id.clone(), "香蕉".into()).expect("add 2");
        let md5_two = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();
        assert_ne!(md5_one, md5_two, "再加词 md5 应变化");

        // remove_word
        remove_word_from_set(id.clone(), "苹果".into()).expect("remove");
        let md5_removed = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();
        assert_ne!(md5_two, md5_removed, "删词后 md5 应变化");
    }

    /// add_words（批量）后 sync_md5 应更新。
    #[test]
    fn add_words_updates_sync_md5() {
        setup_db();
        let id = create_hotword_set("版本".into()).expect("create");
        let md5_before = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();

        let n = add_words_to_set(
            id.clone(),
            vec!["葡萄".into(), "橘子".into()],
        )
        .expect("add_words");
        assert_eq!(n, 2);
        let md5_after = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();
        assert_ne!(md5_before, md5_after, "批量加词后 md5 应变化");
    }

    /// sync_md5 应等于算 md5 用的标准输入（name|enabled|words_text）。
    /// 验证回填的值能被 incremental_export 正确识别为「无变化」（跨设备一致性的基础）。
    #[test]
    fn refilled_md5_matches_incremental_export() {
        setup_db();
        let id = create_hotword_set("版本X".into()).expect("create");
        add_word_to_set(id.clone(), "苹果".into()).expect("add");
        toggle_hotword_set(id.clone(), false).expect("disable");

        let h = db::get_hotword_set(&id).unwrap();
        // 先算 recomputed（借用 h），再取 db_md5（移动 sync_md5）
        let recomputed = octopus_sync::hotword::hotword_set_md5(&h);
        let db_md5 = h.sync_md5.expect("应已回填");

        assert_eq!(
            db_md5, recomputed,
            "DB 中回填的 sync_md5 应与重新计算的值一致"
        );
    }

    /// refill_sync_md5 对不存在的 id 应安全（仅 log::warn 不 panic）。
    #[test]
    fn refill_sync_md5_handles_missing_id_gracefully() {
        setup_db();
        // 不存在的 id——不应 panic
        refill_sync_md5("nonexistent-uuid-xxxx");
        // 函数无返回值，只要不 panic 即通过
    }
}
