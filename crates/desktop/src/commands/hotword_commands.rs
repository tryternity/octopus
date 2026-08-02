//! 热词版本管理后端命令——版本 CRUD + 单词增删 + 导入导出 + 挖掘到版本。
//! 底层：hotword_sets（版本纯文本，v46 id 改 UUID 字符串）+ hotword_hits（全局命中）。

use octopus_infra::db::{self, HotwordSet};
use uuid::Uuid;
use crate::core::error_util::e2s;

/// 写库后统一 reload corrector 热词索引（enabled 版本并集）。
/// 失败仅告警，不阻断写操作（下次启动会重新装载）。
fn reload_after_write() {
    match db::list_active_words() {
        Ok(entries) => octopus_asr_local::corrector::reload_hotwords(entries),
        Err(e) => log::warn!("[hotword] reload 失败: {}", e),
    }
}

/// 写操作后回填 set 的 sync_md5——只算元数据指纹（name|enabled）。
///
/// v57（2026-08-01 word 级 merge 后）：set sync_md5 = 纯元数据指纹。词变更不再
/// 改 set 的 sync_md5——每条 word 记录有自己的 sync_md5（DB 层写入时填，见
/// `add_word_to_set_at`），word 级 merge 据此 diff。set 的 sync_md5 只反映 name/enabled
/// 变化（rename / toggle）。
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

/// 列出某版本的活跃词列表（v57：词数据在 hotword_words 表，不再在 set.wordsText）。
#[tauri::command]
pub fn list_words_in_set(set_id: String) -> Result<Vec<String>, String> {
    db::list_words_in_set(&set_id)
        .map(|ws| ws.into_iter().map(|w| w.word).collect())
        .map_err(e2s)
}

/// 批量查各版本的活跃词数（前端列表 Badge 用，避免逐个 list_words_in_set）。
#[tauri::command]
pub fn list_word_counts() -> Result<std::collections::HashMap<String, i64>, String> {
    let sets = db::list_hotword_sets().map_err(e2s)?;
    let mut counts = std::collections::HashMap::new();
    for s in sets {
        let n = db::list_words_in_set(&s.id)
            .map(|ws| ws.len() as i64)
            .unwrap_or(0);
        counts.insert(s.id, n);
    }
    Ok(counts)
}

/// fuzzy 过滤热词列表（汉字 + 拼音首字母 + 匹配度排序）。
///
/// 复用 `octopus_search::matcher::match_score`（与 ActionBar 同款算法：
/// exact > prefix > word-prefix > pinyin > fuzzy，取最高分）。
/// 调用方：前端 HotwordPanel 搜索框（debounce 后 invoke）。
///
/// 返回按 score 降序排列的命中词（未命中的被过滤）。空 query 返回空。
#[tauri::command]
pub fn filter_hotwords_fuzzy(query: String, words: Vec<String>) -> Result<Vec<String>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut scored: Vec<(String, octopus_search::matcher::Score)> = words
        .into_iter()
        .filter_map(|w| {
            octopus_search::matcher::match_score(&query, &w).map(|s| (w, s))
        })
        .collect();
    // score 降序（高分 = 更匹配 = 排前）
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(scored.into_iter().map(|(w, _)| w).collect())
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
    // 删除后必须 export 到 .sync——否则 sync 文件还存旧态（is_deleted=0），
    // 下次 merge 时 pull_set 把它拉活（软删 10 天内不超期，pull_set 不跳过）→ 复活。
    if let Err(e) = octopus_sync::hotword::export_all_hotwords() {
        log::warn!("[hotword] 删除后 export 失败（不阻断，但可能导致 sync 复活）：{}", e);
    }
    reload_after_write();
    Ok(())
}

// ── 回收站 GC（2026-08-02）──

/// 统计回收站 tombstone 数（前端按钮「回收站 (N)」用）。
#[tauri::command]
pub fn count_hotword_tombstones() -> Result<i64, String> {
    db::count_hotword_tombstones().map_err(e2s)
}

/// 清空回收站——硬删所有 set tombstone + 其词 + 所有 word tombstone（不限年龄）。
/// 用户确认后调。清完 export 重建（清 .sync）。
#[tauri::command]
pub fn purge_hotword_tombstones() -> Result<usize, String> {
    let purged = db::purge_all_hotword_tombstones().map_err(e2s)?;
    if purged > 0 {
        // 清 .sync：export 不含超期/已删 tombstone
        if let Err(e) = octopus_sync::hotword::export_all_hotwords() {
            log::warn!("[hotword] 手动清空后 export 重建失败（不阻断）：{}", e);
        }
    }
    Ok(purged)
}

#[tauri::command]
pub fn toggle_hotword_set(id: String, enabled: bool) -> Result<(), String> {
    db::toggle_hotword_set(&id, enabled).map_err(e2s)?;
    refill_sync_md5(&id);
    reload_after_write();
    Ok(())
}

// ── 方言模糊规则（fuzzy_dialect_rules DB 表，2026-08-01）──

/// 列出全部方言规则（含未启用），供前端渲染 toggles。
#[tauri::command]
pub fn list_fuzzy_dialect_rules() -> Result<Vec<octopus_infra::db::FuzzyDialectRule>, String> {
    db::list_fuzzy_dialect_rules().map_err(e2s)
}

/// 设置单条方言规则开关（前端 toggle 用）。写库后 reload corrector 索引（规则变 key 必变）。
#[tauri::command]
pub fn set_fuzzy_dialect_rule(token: String, enabled: bool) -> Result<(), String> {
    db::set_fuzzy_dialect_rule_enabled(&token, enabled).map_err(e2s)?;
    octopus_asr_local::corrector::reload_fuzzy_dialect();
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
        // 按空白分割后过滤纯数字 token（`词 DF值` 格式的 DF 列被丢弃，只留词语）。
        // 支持两种导入格式：纯词语（每行一个或多个）/ 词语+DF（第二列数字自动滤掉）。
        let words: Vec<String> = content
            .split_whitespace()
            .filter(|s| s.parse::<i64>().is_err())
            .map(|s| s.to_string())
            .collect();

        match mode.as_str() {
            "new" => {
                let name = new_name.unwrap_or_else(|| "导入版本".to_string());
                let id = Uuid::new_v4().to_string();
                db::insert_hotword_set(&id, &name).map_err(e2s)?;
                db::set_words_in_set(&id, &words).map_err(e2s)?;
                refill_sync_md5(&id);
                reload_after_write();
                Ok(id)
            }
            "append" => {
                let id = target_set_id.ok_or("append 模式需 target_set_id")?;
                db::add_words_to_set(&id, &words).map_err(e2s)?;
                refill_sync_md5(&id);
                reload_after_write();
                Ok(id)
            }
            "overwrite" => {
                let id = target_set_id.ok_or("overwrite 模式需 target_set_id")?;
                db::set_words_in_set(&id, &words).map_err(e2s)?;
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

/// 导出某版本词列表到 txt（用户选保存路径）。
#[tauri::command]
pub async fn export_hotwords(app_handle: tauri::AppHandle, set_id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let set = db::get_hotword_set(&set_id).map_err(e2s)?;
        let words = db::list_words_in_set(&set_id).map_err(e2s)?;
        let words_text = words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join("\n");
        use tauri_plugin_dialog::DialogExt;
        let save_path = app_handle
            .dialog()
            .file()
            .add_filter("文本", &["txt"])
            .set_file_name(format!("{}.txt", set.name))
            .blocking_save_file();
        if let Some(path) = save_path {
            let path = path.as_path().ok_or("无效路径")?;
            std::fs::write(path, &words_text).map_err(e2s)?;
            log::info!("[hotword] 导出版本「{}」（{} 词）到 {}", set.name, words.len(), path.display());
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

    /// create_hotword_set 后 sync_md5 应被回填（非 NULL）。
    /// v57：sync_md5 = 元数据指纹 + 词指纹（空 set 词指纹为空），不再等于纯元数据 md5。
    #[test]
    fn create_hotword_set_refills_sync_md5() {
        setup_db();
        let id = create_hotword_set("测试版本A".into()).expect("create");
        let h = db::get_hotword_set(&id).expect("get");
        assert!(h.sync_md5.is_some(), "create 后 sync_md5 应被回填");
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

    /// add_word / remove_word 后 set 的 sync_md5 不变（v57 word 级 merge：set md5 只反映
    /// 元数据 name|enabled），但每条 word 记录有自己的 sync_md5（DB 层写入时填）。
    #[test]
    fn word_operations_dont_change_set_md5_but_fill_word_md5() {
        setup_db();
        let id = create_hotword_set("版本".into()).expect("create");
        let md5_empty = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();

        // add_word——set 的 sync_md5 不变（元数据未变），但 word 记录有自己的 sync_md5
        let added = add_word_to_set(id.clone(), "苹果".into()).expect("add");
        assert!(added);
        let md5_one = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();
        assert_eq!(md5_empty, md5_one, "加词后 set 的 sync_md5 不应变（word 有自己的 md5）");
        let w = db::get_hotword_word(&id, "苹果").unwrap().unwrap();
        assert!(w.sync_md5.is_some(), "word 记录应有自己的 sync_md5");

        // 再加一词——set md5 仍不变
        add_word_to_set(id.clone(), "香蕉".into()).expect("add 2");
        let md5_two = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();
        assert_eq!(md5_one, md5_two, "再加词 set 的 sync_md5 仍不变");

        // remove_word（软删）——set md5 仍不变，但 word 的 sync_md5 变成 is_deleted=true 指纹
        remove_word_from_set(id.clone(), "苹果".into()).expect("remove");
        let md5_removed = db::get_hotword_set(&id).unwrap().sync_md5.unwrap();
        assert_eq!(md5_two, md5_removed, "删词后 set 的 sync_md5 不应变");
    }

    /// add_words（批量）后 set 的 sync_md5 不变，但 word 记录有 sync_md5。
    #[test]
    fn add_words_doesnt_change_set_md5_but_fills_word_md5() {
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
        assert_eq!(md5_before, md5_after, "批量加词后 set 的 sync_md5 不应变");
        for word in &["葡萄", "橘子"] {
            let w = db::get_hotword_word(&id, word).unwrap().unwrap();
            assert!(w.sync_md5.is_some(), "批量加的词应有 sync_md5: {}", word);
        }
    }

    /// 回填的 sync_md5 应非 NULL——incremental_export 有 sync_md5 时直接用（不 fallback），
    /// 故只要非 NULL 即可被正确识别为「无变化」（跨设备一致性的基础）。
    /// v57：set sync_md5 = 纯元数据指纹（refill_sync_md5 算 name|enabled）；word 级 merge
    /// 后词变更不再改 set md5（word 有自己的 sync_md5）。
    #[test]
    fn refilled_md5_matches_incremental_export() {
        setup_db();
        let id = create_hotword_set("版本X".into()).expect("create");
        add_word_to_set(id.clone(), "苹果".into()).expect("add");
        toggle_hotword_set(id.clone(), false).expect("disable");

        let h = db::get_hotword_set(&id).unwrap();
        let db_md5 = h.sync_md5.expect("应已回填（非 NULL）");

        // sync_md5 非 NULL → incremental_export 直接用它（不 fallback 到 hotword_set_md5）
        // 验证：再 export 一次（无变化），二次 export 应 0 变更
        use octopus_sync::hotword::incremental_export_hotwords;
        let (_outline, _changed) = incremental_export_hotwords().expect("first export");
        // 第二次（sync_md5 已在 outline）应无变化
        let (_outline2, changed2) = incremental_export_hotwords().expect("export 2");
        assert_eq!(changed2, 0, "sync_md5 已回填，二次 export 应无变化");
        let _ = db_md5;
    }

    /// refill_sync_md5 对不存在的 id 应安全（仅 log::warn 不 panic）。
    #[test]
    fn refill_sync_md5_handles_missing_id_gracefully() {
        setup_db();
        // 不存在的 id——不应 panic
        refill_sync_md5("nonexistent-uuid-xxxx");
        // 函数无返回值，只要不 panic 即通过
    }

    // ── filter_hotwords_fuzzy：复用 matcher::match_score 的 fuzzy 搜索 ──

    #[test]
    fn fuzzy_filters_and_sorts_by_match_score() {
        // 「八爪鱼」(bzy) 前缀命中「八」最高分；「浮窗」(fc) 拼音命中「bz」不行；
        // 「版本」(bf) 不命中「bz」。验证过滤 + 排序。
        let words = vec!["八爪鱼".to_string(), "版本".to_string(), "八哥".to_string()];
        let result = filter_hotwords_fuzzy("八".into(), words.clone()).expect("fuzzy");
        // 「八爪鱼」「八哥」都是前缀命中，排在前面；「版本」不命中被过滤
        assert!(result.contains(&"八爪鱼".to_string()), "应命中八爪鱼");
        assert!(result.contains(&"八哥".to_string()), "应命中八哥");
        assert!(!result.contains(&"版本".to_string()), "版本不应命中「八」");
    }

    #[test]
    fn fuzzy_pinyin_initials_match() {
        // 「by」匹配「八爪鱼」的拼音首字母 bzy（contains）—— 拼音 fuzzy 核心
        let words = vec!["八爪鱼".to_string(), "版本".to_string()];
        let result = filter_hotwords_fuzzy("bzy".into(), words.clone()).expect("fuzzy");
        assert_eq!(result, vec!["八爪鱼".to_string()], "bzy 应命中八爪鱼（拼音首字母）");
    }

    #[test]
    fn fuzzy_exact_ranks_above_prefix() {
        // query 等于某个词 → exact 10000 > prefix；该词应排第一
        let words = vec!["八爪鱼".to_string(), "八".to_string()];
        let result = filter_hotwords_fuzzy("八".into(), words.clone()).expect("fuzzy");
        assert_eq!(result[0], "八", "精确匹配「八」应排第一（exact > prefix）");
    }

    #[test]
    fn fuzzy_empty_query_returns_empty() {
        let result = filter_hotwords_fuzzy("".into(), vec!["八爪鱼".into()]).expect("fuzzy");
        assert!(result.is_empty(), "空 query 应返回空");
    }

    #[test]
    fn fuzzy_no_match_returns_empty() {
        let result = filter_hotwords_fuzzy("zzz".into(), vec!["八爪鱼".into()]).expect("fuzzy");
        assert!(result.is_empty(), "无匹配应返回空");
    }
}
