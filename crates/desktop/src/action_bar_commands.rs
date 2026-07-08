//! AI 命令面板后端命令——模拟 Cmd+C / LLM 调用 / 模拟 Cmd+V / 打开 URL。

use std::sync::Mutex;
use tauri::{AppHandle, Manager};

use crate::action_bar_window::{hide_action_bar_window, show_action_bar_window};
use crate::focus_tracker::FocusTracker;

/// 暂存选中文本 + 上下文（trigger 时写入，前端 mount 时 take）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    pub text: String,
}

static PENDING_CONTEXT: Mutex<Option<ActionBarContext>> = Mutex::new(None);

/// 热键触发：模拟 Cmd+C → 读剪贴板 → 获取鼠标位置 → 显示浮窗。
#[tauri::command]
pub fn trigger_action_bar(app: AppHandle) {
    // 1. 记录触发前的剪贴板内容
    let clipboard_before = read_clipboard_text(&app);

    // 2. 模拟 Cmd+C
    let focus = FocusTracker::new();
    focus.simulate_copy();

    // 3. 等待 200ms 让系统完成复制
    std::thread::sleep(std::time::Duration::from_millis(200));

    // 4. 读剪贴板
    let clipboard_after = read_clipboard_text(&app);
    let text = match (&clipboard_before, &clipboard_after) {
        (Some(before), Some(after)) if before != after => after.clone(),
        (None, Some(after)) => after.clone(),
        _ => {
            log::warn!("[action-bar] Cmd+C didn't change clipboard — no selection?");
            return;
        }
    };

    if text.trim().is_empty() {
        log::warn!("[action-bar] Selected text is empty");
        return;
    }

    // 5. 暂存上下文
    *PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext { text });

    // 6. 获取鼠标位置
    let (mx, my) = get_mouse_position();
    let win_y = (my - 60.0).max(0.0);

    // 7. 显示浮窗
    show_action_bar_window(&app, mx, win_y);
}

/// 前端 mount 时拉取上下文。
#[tauri::command]
pub fn action_bar_get_context() -> Option<ActionBarContext> {
    PENDING_CONTEXT.lock().unwrap().take()
}

/// 执行 AI 动作（润色/摘要/解释/翻译）。
#[tauri::command]
pub async fn run_ai_action(action: String, text: String) -> Result<String, String> {
    let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    let llm_config = crate::config::llm_config(&config)
        .ok_or("润色模型未配置，请在设置中配置 LLM")?;

    let prompt = match action.as_str() {
        "polish" => "请对以下文本进行润色，使其更加流畅、专业。保持原意不变。只输出润色结果。",
        "summarize" => "请用简洁的中文总结以下内容的要点，不超过 3 句话。只输出总结。",
        "explain" => "请用简洁的中文解释以下内容的含义。只输出解释。",
        "translate" => {
            let has_cjk = text.chars().any(|c| {
                matches!(c as u32, 0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
            });
            if has_cjk {
                "Please translate the following text into English. Only output the translation."
            } else {
                "请将以下文本翻译成中文。只输出翻译结果。"
            }
        }
        _ => return Err(format!("未知动作: {}", action)),
    };

    // 临时切换 system prompt
    let old_prompt = octopus_llm::system_prompt();
    octopus_llm::set_system_prompt(prompt);

    let result = octopus_llm::polish(None, &text, &llm_config)
        .map_err(|e| e.to_string())?;

    // 恢复原 system prompt
    octopus_llm::set_system_prompt(&old_prompt);

    Ok(result)
}

/// 写结果到剪贴板 + 恢复焦点 + 模拟 Cmd+V + 隐藏浮窗。
#[tauri::command]
pub fn action_bar_paste_result(result: String, app: AppHandle) {
    hide_action_bar_window(&app);
    write_clipboard_text(&app, &result);

    let focus = FocusTracker::new();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        focus.restore_focus();
        std::thread::sleep(std::time::Duration::from_millis(100));
        focus.simulate_paste();
    });
}

/// 用系统浏览器打开 URL + 隐藏浮窗。
#[tauri::command]
pub fn action_bar_open_url(url: String, app: AppHandle) {
    hide_action_bar_window(&app);
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd").args(["/c", "start", "", &url]).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
}

// ── 辅助函数 ──

fn read_clipboard_text(app: &AppHandle) -> Option<String> {
    let handle = app.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>();
    handle.read_text().ok()
}

fn write_clipboard_text(app: &AppHandle, text: &str) {
    let handle = app.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>();
    let _ = handle.write_text(text);
}

#[cfg(target_os = "macos")]
fn get_mouse_position() -> (f64, f64) {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let source = match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        Ok(s) => Some(s),
        Err(_) => None,
    };
    let event = match &source {
        Some(s) => CGEvent::new(s.clone()).ok(),
        None => None,
    };
    if let Some(event) = event {
        let point = event.location();
        return (point.x / 2.0, point.y / 2.0);
    }
    (100.0, 100.0)
}

#[cfg(not(target_os = "macos"))]
fn get_mouse_position() -> (f64, f64) {
    (100.0, 100.0)
}
