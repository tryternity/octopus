//! 命令面板 agent 集成（从 coordinator/mod.rs 提取，Task 1.4）。
//!
//! AgentBridge 录音类型（命令面板「听写执行命令」）的分流 + 执行：
//! 录音结束 → `dispatch_by_record_type` 检测 AgentBridge → `execute_agent_task`
//! 渲染 prompt + 选适配器 + Terminal.app 启动。

use crate::engine::transcript::Transcript;
use super::{RecordType, Stage};
use tauri::Emitter;

/// 从 stage 提取 AgentBridge task_id（cancel/discard 清理用）。
pub(crate) fn agent_task_id_in_stage(stage: &Stage) -> Option<String> {
    match stage {
        Stage::Streaming { transcript, .. }
        | Stage::VadSegmented { transcript, .. }
        | Stage::WaitingCompletion { transcript, .. }
        | Stage::StoppingPolish { transcript } => {
            if let RecordType::AgentBridge { task_id } = &transcript.record_type {
                Some(task_id.clone())
            } else { None }
        }
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { transcript, .. } => {
            if let RecordType::AgentBridge { task_id } = &transcript.record_type {
                Some(task_id.clone())
            } else { None }
        }
        _ => None,
    }
}

/// 统一的 record_type 分流 helper——在所有 finalize/cancel/discard 出口调用。
/// 返回 true = 已处理（AgentBridge 路径），调用方应直接 return；
/// 返回 false = Input 路径，调用方继续走现有 paste 逻辑。
pub(crate) fn dispatch_by_record_type(
    transcript: &Transcript,
    text: &str,
    app_handle: &tauri::AppHandle,
) -> bool {
    match &transcript.record_type {
        RecordType::Input => false,
        RecordType::AgentBridge { task_id } => {
            if text.trim().is_empty() {
                let _ = octopus_infra::db::update_agent_task_status(task_id, "failed", "识别结果为空");
            } else {
                execute_agent_task(app_handle, task_id, text);
            }
            true
        }
    }
}

/// agent task 执行器：从 DB 取上下文 + 识别文本 → 渲染命令 → Terminal.app
pub(crate) fn execute_agent_task(app_handle: &tauri::AppHandle, task_id: &str, transcribed_text: &str) {
    // 所有早返回路径统一执行 hide_result + tray Idle
    let cleanup = || {
        crate::ui::result_window::hide_result(app_handle);
        crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
    };

    if let Err(e) = octopus_infra::db::update_agent_task_result(task_id, transcribed_text) {
        log::error!("[agent-task] 更新 task 失败: {}", e);
        cleanup();
        return;
    }

    let task = match octopus_infra::db::load_agent_task(task_id) {
        Ok(Some(t)) => t,
        Ok(None) => { log::warn!("[agent-task] task {} 不存在", task_id); cleanup(); return; }
        Err(e) => { log::error!("[agent-task] 加载 task 失败: {}", e); cleanup(); return; }
    };

    let ctx = parse_agent_context(&task.context);
    let prompt = crate::action_bar::action_bar_commands::render_agent_prompt(&ctx.prompt_template, transcribed_text, &ctx.text, &ctx.files);

    let adapters = crate::action_bar::agent_adapter::list_adapters();
    // 三层 fallback（v42）：菜单指定 → 系统默认 → 第一个可用
    let adapter = {
        // 1. 菜单指定
        if !task.agent_key.is_empty() {
            if let Some(a) = adapters.iter().find(|a| a.key == task.agent_key && a.is_available) {
                a.clone()
            } else {
                log::warn!(
                    "[agent-task] 菜单指定 '{}' 不可用/不存在，fallback",
                    task.agent_key
                );
                // 2. 系统默认
                if let Some(a) = adapters.iter().find(|a| a.is_default && a.is_available) {
                    a.clone()
                } else {
                    // 3. 第一个可用
                    match adapters.iter().find(|a| a.is_available) {
                        Some(a) => a.clone(),
                        None => {
                            let msg = format!(
                                "没有可用的 agent（菜单指定='{}'；默认不可用；列表全部未安装）",
                                task.agent_key
                            );
                            let _ = octopus_infra::db::update_agent_task_status(task_id, "failed", &msg);
                            crate::ui::result_window::show_result(app_handle, &format!("❌ {}", msg), None);
                            crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
                            return;
                        }
                    }
                }
            }
        } else {
            // agent_key 为空——直接走默认 / fallback
            if let Some(a) = adapters.iter().find(|a| a.is_default && a.is_available) {
                a.clone()
            } else if let Some(a) = adapters.iter().find(|a| a.is_available) {
                a.clone()
            } else {
                let msg = "没有可用的 agent（菜单未指定；默认不可用；列表全部未安装）".to_string();
                let _ = octopus_infra::db::update_agent_task_status(task_id, "failed", &msg);
                crate::ui::result_window::show_result(app_handle, &format!("❌ {}", msg), None);
                crate::ui::tray::update_tray_label(app_handle, crate::ui::tray::TrayState::Idle);
                return;
            }
        }
    };

    // Terminal.app 启动投递到后台线程，避免阻塞协调器（osascript 可能数秒）
    let command = crate::action_bar::agent_adapter::render_command(&adapter.command_template, &prompt, &ctx.files, &ctx.cwd);
    let cwd = ctx.cwd.clone();
    let app_clone = app_handle.clone();
    let tid = task_id.to_string();
    std::thread::spawn(move || {
        use crate::action_bar::terminal_launcher::{TerminalAppLauncher, TerminalLauncher};
        match TerminalAppLauncher.spawn(&command, std::path::Path::new(&cwd)) {
            Ok(()) => {
                let _ = octopus_infra::db::update_agent_task_status(&tid, "done", "");
                crate::ui::result_window::hide_result(&app_clone);
            }
            Err(e) => {
                let _ = octopus_infra::db::update_agent_task_status(&tid, "failed", &e);
                crate::ui::result_window::show_result(&app_clone, &format!("❌ Terminal 启动失败: {}", e), None);
            }
        }
        let _ = app_clone.emit("agent-task://updated", ());
        crate::ui::tray::update_tray_label(&app_clone, crate::ui::tray::TrayState::Idle);
    });
}

/// 解析 agent task context JSON（纯函数，可测试）。
pub struct AgentContext {
    pub files: Vec<String>,
    pub text: String,
    pub cwd: String,
    pub prompt_template: String,
}

pub fn parse_agent_context(context_json: &str) -> AgentContext {
    let context: serde_json::Value = serde_json::from_str(context_json).unwrap_or(serde_json::json!({}));
    AgentContext {
        files: context["files"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        text: context["text"].as_str().unwrap_or("").to_string(),
        cwd: context["cwd"].as_str().unwrap_or("/tmp").to_string(),
        prompt_template: context["prompt_template"].as_str().unwrap_or("").to_string(),
    }
}

/// 重试 failed task（用已有 transcribed_text 重新执行）
pub fn retry_agent_task(app_handle: &tauri::AppHandle, task_id: &str) {
    match octopus_infra::db::load_agent_task(task_id) {
        Ok(Some(t)) => execute_agent_task(app_handle, task_id, &t.transcribed_text),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agent_context_full() {
        let json = r#"{"kind":"files","files":["/a.pdf","/b.pdf"],"cwd":"/Users/x","prompt_template":"{{voice}}\n\n{{files}}"}"#;
        let ctx = parse_agent_context(json);
        assert_eq!(ctx.files, vec!["/a.pdf", "/b.pdf"]);
        assert_eq!(ctx.cwd, "/Users/x");
        assert_eq!(ctx.prompt_template, "{{voice}}\n\n{{files}}");
    }

    #[test]
    fn parse_agent_context_empty_json() {
        let ctx = parse_agent_context("{}");
        assert!(ctx.files.is_empty());
        assert_eq!(ctx.cwd, "/tmp");
        assert_eq!(ctx.prompt_template, "");
    }

    #[test]
    fn parse_agent_context_invalid_json() {
        let ctx = parse_agent_context("not json at all");
        assert!(ctx.files.is_empty());
        assert_eq!(ctx.cwd, "/tmp");
    }

    #[test]
    fn parse_agent_context_missing_files_key() {
        let ctx = parse_agent_context(r#"{"cwd":"/home","prompt_template":"hi"}"#);
        assert!(ctx.files.is_empty());
        assert_eq!(ctx.cwd, "/home");
        assert_eq!(ctx.prompt_template, "hi");
    }

    #[test]
    fn parse_agent_context_files_with_non_string_entries() {
        // 混合类型数组——非字符串的应被过滤
        let ctx = parse_agent_context(r#"{"files":["/a.pdf",42,null,"/b.pdf"]}"#);
        assert_eq!(ctx.files, vec!["/a.pdf", "/b.pdf"]);
    }

    #[test]
    fn parse_agent_context_missing_cwd_falls_back_to_tmp() {
        let ctx = parse_agent_context(r#"{"files":["/a.pdf"]}"#);
        assert_eq!(ctx.cwd, "/tmp");
    }

    #[test]
    fn parse_agent_context_empty_files_array() {
        let ctx = parse_agent_context(r#"{"files":[]}"#);
        assert!(ctx.files.is_empty());
    }

    #[test]
    fn parse_agent_context_prompt_with_task_placeholder() {
        let ctx = parse_agent_context(r#"{"prompt_template":"{{voice}}\n\n文件列表：\n{{files}}"}"#);
        assert!(ctx.prompt_template.contains("{{voice}}"));
        assert!(ctx.prompt_template.contains("{{files}}"));
    }
}
