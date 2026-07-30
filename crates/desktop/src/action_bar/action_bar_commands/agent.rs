//! Agent 适配器 + 任务管理（从 action_bar_commands/mod.rs 提取，Task 1.1）。
//!
//! 命令面板「智能体管理」Tab 的后端：适配器 CRUD + 语音联动触发 + 任务列表/重试/删除。

use tauri::AppHandle;
use crate::core::error_util::e2s;
use crate::action_bar::action_bar_window::hide_action_bar_window;
use super::{PENDING_CONTEXT, derive_cwd, resolve_prompt_reference, finalize_action_bar};

#[tauri::command]
pub fn list_agent_adapters() -> Result<Vec<crate::action_bar::agent_adapter::AgentAdapter>, String> {
    Ok(crate::action_bar::agent_adapter::list_adapters())
}

#[tauri::command]
pub fn create_agent_adapter(
    key: String, display_name: String, detect_binary: String, command_template: String,
) -> Result<i64, String> {
    // DB UNIQUE(key) 约束拦截同名——内置（is_system=1）已 seed 入表，
    // 用户尝试 create 同 key 直接被 UNIQUE 拒绝。
    octopus_infra::db::insert_agent_adapter_record(&key, &display_name, &detect_binary, &command_template)
        .map_err(e2s)
}

#[tauri::command]
pub fn update_agent_adapter(
    id: i64, key: String, display_name: String, detect_binary: String, command_template: String,
) -> Result<(), String> {
    // DB UNIQUE(key) 约束拦截。内置项（is_system=1）的 key 字段仍允许更新
    // （detect_binary / command_template 可能因版本变化需要调整），但不允许删除。
    octopus_infra::db::update_agent_adapter_record(id, &key, &display_name, &detect_binary, &command_template)
        .map_err(e2s)
}

/// 设为默认 agent（全局唯一）。
#[tauri::command]
pub fn set_default_agent(id: i64) -> Result<(), String> {
    octopus_infra::db::set_default_agent(id).map_err(e2s)
}

/// 清除默认 agent（菜单 agent='' 时走 fallback 到第一个可用）。
#[tauri::command]
pub fn clear_default_agent() -> Result<(), String> {
    octopus_infra::db::clear_default_agent().map_err(e2s)
}

#[tauri::command]
pub fn delete_agent_adapter(id: i64) -> Result<(), String> {
    octopus_infra::db::delete_agent_adapter_record(id).map_err(e2s)
}

#[tauri::command]
pub fn refresh_agent_detection() -> Result<Vec<crate::action_bar::agent_adapter::AgentAdapter>, String> {
    Ok(crate::action_bar::agent_adapter::refresh_detection())
}

// ── Agent Voice（语音联动）──

/// trigger_agent_voice 的核心逻辑——Tauri 命令和 quick_execute 共用。
///
/// `hide_action_bar: bool` 控制是否走 hide 浮窗收口：
/// - Tauri 命令路径（用户从 ActionBar 浮窗点击 agent 项）：ActionBar 可见 → 传 `true`，
///   走 `hide_action_bar_window + finalize_action_bar` 统一收口（切回 Accessible + 焦点协调）。
/// - quick_execute 路径（全局快捷键直触发）：ActionBar 本就未显示 → 传 `false`，
///   不调 hide（hide 一个不可见窗口会触发不必要的 activateWithOptions 把源 app 拉到前台，
///   干扰随后 CompactEditor 的 set_focus 夺焦）。
///
/// 关键副作用：`coordinator.start_agent_recording(task_id)` —— 启动语音录入。
/// 不调它用户说话进不来。
pub(crate) fn trigger_agent_voice_core(
    item: &octopus_infra::db::ActionBarItem,
    app: &AppHandle,
    coordinator: &crate::engine::coordinator::Coordinator,
    hide_action_bar: bool,
) -> Result<(), String> {
    let pending = PENDING_CONTEXT.lock();
    let files: Vec<String> = pending.as_ref().map(|c| c.files.clone()).unwrap_or_default();
    let selected_text: String = pending.as_ref().and_then(|c| c.text.clone()).unwrap_or_default();

    let cwd = derive_cwd(&files);
    let context = serde_json::json!({
        "kind": "files",
        "files": files,
        "text": selected_text,
        "cwd": cwd,
        "prompt_template": resolve_prompt_reference(&item.action_data),
    }).to_string();

    let task_id = uuid::Uuid::new_v4().to_string();
    octopus_infra::db::insert_agent_task(&task_id, &item.agent, &context)
        .map_err(e2s)?;

    if hide_action_bar {
        // 隐藏 action bar 浮窗（走统一收口 hide_action_bar_window：含切回 Accessory + 焦点协调，非裸 win.hide()）
        hide_action_bar_window(app);
        finalize_action_bar(app);
    }

    // 触发 agent 录音
    coordinator.start_agent_recording(task_id);
    Ok(())
}

/// agent 项 need_voice=true 时：创建 agent_task → 隐藏浮窗 → 触发音录。
/// Tauri 命令——薄包装，核心逻辑在 trigger_agent_voice_core。
///
/// 2026-07-19 v40 改：判定从「action_data 含 {{voice}}」改为「need_voice 字段」，
/// 避免前端扫描 prompt 字符串的脆弱性。need_voice 由 seed 或用户在设置面板勾选。
#[tauri::command]
pub fn trigger_agent_voice(
    item_id: i64,
    app: AppHandle,
    coordinator: tauri::State<'_, crate::engine::coordinator::Coordinator>,
) -> Result<(), String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(e2s)?
        .ok_or("菜单项不存在")?;
    if !item.need_voice {
        return Err(format!("菜单项「{}」未启用语音输入（need_voice=false）", item.title));
    }
    trigger_agent_voice_core(&item, &app, coordinator.inner(), true)
}

#[tauri::command]
pub fn list_agent_tasks(limit: Option<i64>) -> Result<Vec<octopus_infra::db::AgentTask>, String> {
    octopus_infra::db::list_agent_tasks(limit.unwrap_or(100)).map_err(e2s)
}

#[tauri::command]
pub fn delete_agent_task(id: String) -> Result<(), String> {
    octopus_infra::db::delete_agent_task(&id).map_err(e2s)
}

#[tauri::command]
pub fn retry_agent_task(id: String, app: AppHandle) -> Result<(), String> {
    let task = octopus_infra::db::load_agent_task(&id)
        .map_err(e2s)?
        .ok_or("task 不存在")?;
    if task.status != "failed" && task.status != "done" {
        return Err("仅 failed/done 状态可重试".into());
    }
    if task.transcribed_text.trim().is_empty() {
        return Err("识别结果为空，无法重试".into());
    }
    crate::engine::coordinator::retry_agent_task(&app, &id);
    Ok(())
}
