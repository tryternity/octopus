//! 菜单项 CRUD + 脚本执行记录 + prompt seed 复原（从 action_bar_commands/mod.rs 提取，Task 1.4）。
//!
//! 命令面板设置页的后端：菜单项增删改查 / 排序 / 全局快捷键、脚本执行记录列表/清理/删除、
//! 润色 prompt 按 seed 文件复原。`derive_need_voice` 由 create/update 在保存时统一调用。

use tauri::AppHandle;
use crate::core::error_util::{e2s, e2s_ctx};

// ── 菜单管理命令（设置页 CRUD）──

#[tauri::command]
pub fn list_action_bar_items() -> Result<Vec<octopus_infra::db::ActionBarItem>, String> {
    octopus_infra::db::list_all_action_bar_items().map_err(e2s)
}

/// 推导 need_voice：agent 类型且 action_data 含 `{{voice}}` → true；否则 false。
/// `{{voice}}` 占位符触发语音录入（用户口述指令），识别结果填入该占位符。
/// 由 create/update_action_bar_item 在保存时统一调用，**前端不再传 need_voice 字段**
/// （2026-07-19 v43 修订——回滚前端 toggle，保留 DB 字段，保存时自动判定）。
fn derive_need_voice(action_type: &str, action_data: &str) -> bool {
    action_type == "agent" && action_data.contains("{{voice}}")
}

#[allow(clippy::too_many_arguments)] // Tauri 命令参数平铺（前端 invoke JSON 传）
#[tauri::command]
pub fn create_action_bar_item(
    parent_id: Option<i64>,
    title: String,
    icon: String,
    action_type: String,
    action_data: String,
    is_async: bool,
    write_output_to_clipboard: bool,
    agent: String,
    accepts: String,
    trigger_keyword: Option<String>,
    is_enabled: Option<bool>,
    app_bundle_ids: Option<String>,
) -> Result<i64, String> {
    // 同级菜单项最多 35 个（Alt+1-9 + a-z 定位符上限）
    let all = octopus_infra::db::list_all_action_bar_items().map_err(e2s)?;
    let sibling_count = all.iter().filter(|i| i.parent_id == parent_id).count();
    if sibling_count >= 35 {
        return Err("同级菜单项已达上限 35 个（Alt+1-9 + a-z 定位）".into());
    }
    let need_voice = derive_need_voice(&action_type, &action_data);
    octopus_infra::db::insert_action_bar_item(&octopus_infra::db::ActionBarItemInput {
        parent_id,
        fields: octopus_infra::db::ActionBarItemFields {
            title: &title,
            icon: &icon,
            action_type: &action_type,
            action_data: &action_data,
            is_async,
            write_output_to_clipboard,
            agent: &agent,
            accepts: &accepts,
            trigger_keyword: trigger_keyword.as_deref().unwrap_or(""),
            is_enabled: is_enabled.unwrap_or(true),
            need_voice,
            app_bundle_ids: app_bundle_ids.as_deref().unwrap_or(""),
        },
    })
        .map_err(e2s)
}

#[allow(clippy::too_many_arguments)] // Tauri 命令参数平铺（前端 invoke JSON 传）
#[tauri::command]
pub fn update_action_bar_item(
    id: i64,
    title: String,
    icon: String,
    action_type: String,
    action_data: String,
    is_enabled: bool,
    is_async: bool,
    write_output_to_clipboard: bool,
    agent: String,
    accepts: String,
    trigger_keyword: Option<String>,
    app_bundle_ids: Option<String>,
) -> Result<(), String> {
    // need_voice 自动从 action_type + action_data 推导（前端不再传）
    let need_voice = derive_need_voice(&action_type, &action_data);
    octopus_infra::db::update_action_bar_item(&octopus_infra::db::ActionBarItemUpdate {
        id,
        fields: octopus_infra::db::ActionBarItemFields {
            title: &title,
            icon: &icon,
            action_type: &action_type,
            action_data: &action_data,
            is_async,
            write_output_to_clipboard,
            agent: &agent,
            accepts: &accepts,
            trigger_keyword: trigger_keyword.as_deref().unwrap_or(""),
            is_enabled,
            need_voice,
            app_bundle_ids: app_bundle_ids.as_deref().unwrap_or(""),
        },
    })
        .map_err(e2s)
}

#[tauri::command]
pub fn delete_action_bar_item(id: i64) -> Result<(), String> {
    octopus_infra::db::delete_action_bar_item(id).map_err(e2s)
}

#[tauri::command]
pub fn move_action_bar_item(id: i64, direction: i32) -> Result<(), String> {
    octopus_infra::db::move_action_bar_item(id, direction).map_err(e2s)
}

/// 设置菜单项的全局快捷键（Quick Execute silent 入口）。空串清除。
/// 保存后触发重新注册全局快捷键。
#[tauri::command]
pub fn set_global_shortcut(id: i64, global_shortcut: String, app: AppHandle) -> Result<(), String> {
    octopus_infra::db::set_global_shortcut(id, &global_shortcut).map_err(e2s)?;
    // 重新注册全局快捷键
    crate::action_bar::action_hotkey::register_action_hotkeys(&app);
    Ok(())
}

// ── 脚本执行记录 ──

#[tauri::command]
pub fn list_script_runs(limit: Option<i64>, item_id: Option<i64>) -> Result<Vec<octopus_infra::db::ScriptRun>, String> {
    octopus_infra::db::list_script_runs(limit, item_id).map_err(e2s)
}

#[tauri::command]
pub fn clear_script_runs(keep_recent: Option<i64>) -> Result<(), String> {
    octopus_infra::db::clear_script_runs(keep_recent).map_err(e2s)
}

/// 按 ID 批量删除执行记录。2026-07-17 新增——执行记录 TAB 复选框删除。
#[tauri::command]
pub fn delete_script_runs(ids: Vec<i64>) -> Result<(), String> {
    octopus_infra::db::delete_script_runs(&ids).map_err(e2s)
}

/// 按 prompt id 复原默认内容：读 seeds/prompts/<name>.md 文件内容并返回字符串。
/// 不直接写 DB——前端把内容塞回 textarea，由用户点「保存」触发 `update_prompt` 才入库。
/// id → name 映射：1 → "faithful"，2 → "user-intent"，3 → "app-casual"。
#[tauri::command]
pub fn restore_prompt_from_seed(prompt_id: i64) -> Result<String, String> {
    let name = match prompt_id {
        1 => "faithful",
        2 => "user-intent",
        3 => "app-casual",
        _ => return Err(format!("prompt id {} 无对应 seed 文件", prompt_id)),
    };
    let path = octopus_infra::seeds::seed_prompt_path(name)
        .ok_or_else(|| format!("seed 文件不存在: {}.md", name))?;
    std::fs::read_to_string(&path).map_err(|e| e2s_ctx("读 seed 文件失败: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_need_voice_agent_with_task_placeholder() {
        assert!(derive_need_voice("agent", "做 PPT：{{voice}}\n文件：{{files}}"));
    }

    #[test]
    fn derive_need_voice_agent_without_task_placeholder() {
        assert!(!derive_need_voice("agent", "整理这些文件：{{files}}"));
    }

    #[test]
    fn derive_need_voice_non_agent_type() {
        // 非 agent 类型——即使含 {{voice}} 也不是语音项
        assert!(!derive_need_voice("script", "#shell\necho {{voice}}"));
        assert!(!derive_need_voice("url", "https://example.com/?q={{voice}}"));
    }
}
