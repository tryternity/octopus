//! AI 命令面板后端命令——模拟 Cmd+C / LLM 调用 / 模拟 Cmd+V / 打开 URL。

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::action_bar_window::{hide_action_bar_window, show_action_bar_window};
use crate::focus_tracker::FocusTracker;

/// 暂存选中文本 + 上下文（trigger 时写入，前端 mount 时 take）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    pub text: String,
}

static PENDING_CONTEXT: Mutex<Option<ActionBarContext>> = Mutex::new(None);
/// 记录被隐藏的常规窗口 label（paste/hide 后恢复）
static HIDDEN_REGULAR_WINDOWS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// 记录触发前的剪贴板内容（操作完成后恢复——模拟 Cmd+C 的文本不进历史）
static CLIPBOARD_BACKUP: Mutex<Option<String>> = Mutex::new(None);

/// 热键触发：模拟 Cmd+C → 读剪贴板 → 获取鼠标位置 → 显示浮窗。
#[tauri::command]
pub fn trigger_action_bar(app: AppHandle) {
    let app_clone = app.clone();
    std::thread::spawn(move || {
        // 0. 记录并隐藏常规窗口
        let hidden: Vec<String> = ["settings_window", "compact_editor_window"]
            .iter()
            .filter_map(|label| {
                if let Some(win) = app_clone.get_webview_window(label) {
                    if win.is_visible().unwrap_or(false) {
                        let _ = win.hide();
                        return Some(label.to_string());
                    }
                }
                None
            })
            .collect();
        *HIDDEN_REGULAR_WINDOWS.lock().unwrap() = hidden.clone();
        log::info!("[action-bar] hidden regular windows: {:?}", hidden);

        // 1. 记录触发前的剪贴板内容（用于完成后恢复）
        let clipboard_before = read_clipboard_text(&app_clone);
        *CLIPBOARD_BACKUP.lock().unwrap() = clipboard_before.clone();

        // 2. 模拟 Cmd+C（不暂停 watcher——暂停会影响 clipboard-rs 的读取。
        //   watcher 的 on_clipboard_change 会记录，但我们在 paste_result 完成后
        //   恢复原始剪贴板内容，多余的条目可以通过删除最近一条来清理）
        let focus = FocusTracker::new();
        focus.simulate_copy();

        // 3. 等待 200ms 让系统完成复制
        std::thread::sleep(std::time::Duration::from_millis(200));

        // 4. 读剪贴板
        let clipboard_after = read_clipboard_text(&app_clone);
        let text = match (&clipboard_before, &clipboard_after) {
            (Some(before), Some(after)) if before != after => after.clone(),
            (None, Some(after)) => after.clone(),
            _ => {
                log::warn!("[action-bar] Cmd+C didn't change clipboard — no selection?");
                restore_hidden_windows(&app_clone);
                return;
            }
        };

        if text.trim().is_empty() {
            log::warn!("[action-bar] Selected text is empty");
            restore_hidden_windows(&app_clone);
            return;
        }

        log::info!("[action-bar] got text len={}", text.len());

        // 5. 暂存上下文
        *PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext { text });

        // 6. 获取鼠标位置 + 显示浮窗（主线程）
        let (mx, my) = get_mouse_position();
        let win_y = (my - 60.0).max(0.0);
        log::info!("[action-bar] show at {},{}", mx, win_y);

        let app_for_show = app_clone.clone();
        let _ = tauri::async_runtime::spawn(async move {
            show_action_bar_window(&app_for_show, mx, win_y);
        });
    });
}

/// 恢复被隐藏的常规窗口（从全局 HIDDEN_REGULAR_WINDOWS 读取并清空）
fn restore_hidden_windows(app: &AppHandle) {
    let labels = HIDDEN_REGULAR_WINDOWS.lock().unwrap().drain(..).collect::<Vec<_>>();
    for label in &labels {
        if let Some(win) = app.get_webview_window(label) {
            let _ = win.show();
            log::info!("[action-bar] restored window: {}", label);
        }
    }
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
    let llm_config = crate::config::llm_config_ignore_mode(&config)
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

/// 前端隐藏浮窗时调用——恢复被隐藏的常规窗口。
#[tauri::command]
pub fn action_bar_dismiss(app: AppHandle) {
    hide_action_bar_window(&app);
    restore_hidden_windows(&app);
}

/// AI 结果通过临时 tab 打开 CompactEditor 展示（不写 DB）。
#[tauri::command]
pub fn action_bar_show_result(result: String, original_text: String, action: String, app: AppHandle) {
    hide_action_bar_window(&app);

    let label = match action.as_str() {
        "translate" => "翻译",
        "polish" => "润色",
        "summarize" => "摘要",
        "explain" => "解释",
        _ => "AI",
    };
    let display_text = format!("【{}】\n{}", label, result);

    // 结果写入系统剪贴板（方便用户手动粘贴）
    write_clipboard_text(&app, &result);

    // 恢复剪贴板原始内容 + 恢复常规窗口
    let app_clone = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let backup = CLIPBOARD_BACKUP.lock().unwrap().take();
        if let Some(original) = backup {
            write_clipboard_text(&app_clone, &original);
        }
        restore_hidden_windows(&app_clone);
    });

    // 用临时 tab 打开 CompactEditor（不写 DB，保存按钮灰掉）
    // 窗口已存在 → emit 推送新 tab；窗口不存在 → store_pending_temp_tab + 建窗
    if let Some(window) = app.get_webview_window(crate::compact_editor_window::WINDOW_LABEL) {
        // 窗口已存在——通过事件推送新 tab
        let _ = window.emit("compact-editor://open-tab", serde_json::json!({
            "itemId": 0,
            "source": "temp",
            "text": display_text,
            "isTemp": true,
        }));
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        crate::compact_editor_commands::store_pending_temp_tab(display_text, "temp");
        crate::compact_editor_window::create_compact_editor_window(&app, None);
    }
}

/// 用系统浏览器打开 URL + 隐藏浮窗 + 恢复常规窗口。
#[tauri::command]
pub fn action_bar_open_url(url: String, app: AppHandle) {
    hide_action_bar_window(&app);
    restore_hidden_windows(&app);
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
        // CGEvent::location() 返回 Quartz 全局坐标（points/逻辑像素，原点左上角 y 向下），
        // 与 Tauri LogicalPosition 坐标系一致——不除 scale。
        return (point.x, point.y);
    }
    (100.0, 100.0)
}

#[cfg(not(target_os = "macos"))]
fn get_mouse_position() -> (f64, f64) {
    (100.0, 100.0)
}
