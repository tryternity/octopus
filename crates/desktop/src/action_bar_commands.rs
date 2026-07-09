//! AI 命令面板后端命令——模拟 Cmd+C / LLM 调用 / 模拟 Cmd+V / 打开 URL。

use std::sync::atomic::{AtomicBool, Ordering};
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
/// 重入 guard——防止热键连按导致 trigger 重叠执行（丢失 HIDDEN_REGULAR_WINDOWS）
static TRIGGER_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// 热键触发：模拟 Cmd+C → 读剪贴板 → 获取鼠标位置 → 显示浮窗。
#[tauri::command]
pub fn trigger_action_bar(app: AppHandle) {
    // action bar 依赖 macOS 模拟 Cmd+C + CGEvent 鼠标坐标，其他平台尚未实现
    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("[action-bar] 仅 macOS 支持此功能");
        let _ = app;
        return;
    }

    let app_clone = app.clone();
    std::thread::spawn(move || {
        // 重入 guard——防止热键连按导致第二次 trigger 覆盖 HIDDEN_REGULAR_WINDOWS
        if TRIGGER_IN_PROGRESS.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            log::info!("[action-bar] trigger already in progress, skipping");
            return;
        }

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

        // 1. 记录触发前的剪贴板内容
        let clipboard_before = read_clipboard_text(&app_clone);

        // 2. suppress watcher——osascript 模拟 Cmd+C 直接写系统剪贴板，
        //    绕过 write_text 的自动 suppress，需手动抑制防选中文本入库
        let clip_handle = app_clone.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>();
        clip_handle.suppress_next();

        let focus = FocusTracker::new();
        focus.simulate_copy();

        // 3. 等待 200ms 让系统完成复制
        std::thread::sleep(std::time::Duration::from_millis(200));

        // 4. 读剪贴板拿到选中文本
        let clipboard_after = read_clipboard_text(&app_clone);
        let text = match (&clipboard_before, &clipboard_after) {
            (Some(before), Some(after)) if before != after => after.clone(),
            (None, Some(after)) => after.clone(),
            (Some(_), Some(after)) if !after.trim().is_empty() => {
                log::info!("[action-bar] clipboard unchanged but has content, using it");
                after.clone()
            }
            _ => {
                log::warn!("[action-bar] No text available — no selection?");
                finalize_action_bar(&app_clone);
                return;
            }
        };

        if text.trim().is_empty() {
            log::warn!("[action-bar] Selected text is empty");
            finalize_action_bar(&app_clone);
            return;
        }

        // 5. 立即恢复原始剪贴板内容（write_text 自带 suppress，不会入库）
        if let Some(ref original) = clipboard_before {
            if Some(original.as_str()) != clipboard_after.as_deref() {
                write_clipboard_text(&app_clone, original);
            }
        }

        log::info!("[action-bar] got text len={}", text.len());

        // 5. 暂存上下文
        *PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext { text });

        // 6. 获取鼠标位置 + 显示浮窗（主线程）
        // 浮窗在鼠标正上方，X 轴居中对齐鼠标，Y 轴在鼠标上方
        let (mx, my) = get_mouse_position(&app_clone);
        let win_x = (mx - 150.0).max(0.0);
        // 窗口在鼠标正上方——窗口底部贴近鼠标。主菜单行 ~52px（含 padding）
        let win_y = (my - 58.0).max(0.0);
        log::info!("[action-bar] mouse=({},{}) → win_pos=({},{})", mx, my, win_x, win_y);

        let app_for_show = app_clone.clone();
        // NSWindow 操作（set_position/show/set_focus）必须在主线程执行，
        // async_runtime::spawn 跑在 tokio worker 线程，可能触发 AppKit 违规。
        let _ = app_clone.run_on_main_thread(move || {
            show_action_bar_window(&app_for_show, win_x, win_y);
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

/// action bar 所有出口的统一收口：恢复常规窗口 + 重置重入 guard。
/// 剪贴板已在 trigger 阶段即时恢复，此处不再碰剪贴板。
fn finalize_action_bar(app: &AppHandle) {
    restore_hidden_windows(app);
    TRIGGER_IN_PROGRESS.store(false, Ordering::SeqCst);
}

/// 前端 mount 时拉取上下文。
#[tauri::command]
pub fn action_bar_get_context() -> Option<ActionBarContext> {
    PENDING_CONTEXT.lock().unwrap().take()
}

/// 前端隐藏浮窗时调用——恢复被隐藏的常规窗口。
#[tauri::command]
pub fn action_bar_dismiss(app: AppHandle) {
    hide_action_bar_window(&app);
    finalize_action_bar(&app);
}

/// AI 结果通过临时 tab 打开 CompactEditor 展示（不写 DB）。
/// 结果写入剪贴板留给用户——不恢复原始剪贴板（与 dismiss/open_url 不同）。
#[tauri::command]
pub fn action_bar_show_result(result: String, _original_text: String, action: String, app: AppHandle) {
    hide_action_bar_window(&app);

    let label = match action.as_str() {
        "translate" => "翻译",
        "polish" => "润色",
        "summarize" => "摘要",
        "explain" => "解释",
        _ => "AI",
    };
    let display_text = format!("【{}】\n{}", label, result);

    // 结果写入系统剪贴板（方便用户手动粘贴）——write_text 自带 suppress 不会入库
    write_clipboard_text(&app, &result);

    finalize_action_bar(&app);

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
fn get_mouse_position(_app: &AppHandle) -> (f64, f64) {
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
        // CGEvent::location() 返回 Quartz 全局坐标——逻辑像素（points），
        // 原点主屏左上角，y 轴向下。与 Tauri LogicalPosition 坐标系一致。
        // 不除 scale——Quartz 已是逻辑坐标。
        log::info!("[action-bar] mouse location={},{}", point.x, point.y);
        return (point.x, point.y);
    }
    (100.0, 100.0)
}

#[cfg(not(target_os = "macos"))]
fn get_mouse_position(_app: &AppHandle) -> (f64, f64) {
    (100.0, 100.0)
}

// ── 菜单管理命令（设置页 CRUD）──

#[tauri::command]
pub fn list_action_bar_items() -> Result<Vec<octopus_infra::db::ActionBarItem>, String> {
    octopus_infra::db::list_all_action_bar_items().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_action_bar_item(
    parent_id: Option<i64>,
    title: String,
    icon: String,
    action_type: String,
    action_data: String,
) -> Result<i64, String> {
    octopus_infra::db::insert_action_bar_item(parent_id, &title, &icon, &action_type, &action_data)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_action_bar_item(
    id: i64,
    title: String,
    icon: String,
    action_type: String,
    action_data: String,
    is_enabled: bool,
) -> Result<(), String> {
    octopus_infra::db::update_action_bar_item(id, &title, &icon, &action_type, &action_data, is_enabled)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_action_bar_item(id: i64) -> Result<(), String> {
    octopus_infra::db::delete_action_bar_item(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn move_action_bar_item(id: i64, direction: i32) -> Result<(), String> {
    octopus_infra::db::move_action_bar_item(id, direction).map_err(|e| e.to_string())
}

// ── 统一执行入口 ──

/// 按 CJK 检测方向，返回翻译 system prompt。
fn auto_translate_prompt(text: &str) -> &'static str {
    let has_cjk = text.chars().any(|c| {
        matches!(c as u32, 0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
    });
    if has_cjk {
        "Please translate the following text into English. Only output the translation."
    } else {
        "请将以下文本翻译成中文。只输出翻译结果。"
    }
}

/// 简易 URL 编码（避免引入新 crate）
fn simple_url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for &byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// 执行脚本：按第一行 magic comment 分发运行时。
fn run_script(source: &str, text: &str) -> Result<(), String> {
    let first_line = source.lines().next().unwrap_or("").trim();
    let body: String = source.lines().skip(1).collect::<Vec<_>>().join("\n");
    let script = body.replace("{text}", text);

    let result: std::io::Result<std::process::Child> = match first_line {
        "#shell" => std::process::Command::new("sh").arg("-c").arg(&script).spawn(),
        "#osascript" => {
            #[cfg(target_os = "macos")]
            { std::process::Command::new("osascript").arg("-e").arg(&script).spawn() }
            #[cfg(not(target_os = "macos"))]
            { return Err("osascript 仅 macOS 支持".into()); }
        }
        "#powershell" => {
            #[cfg(target_os = "windows")]
            { std::process::Command::new("powershell").arg("-Command").arg(&script).spawn() }
            #[cfg(not(target_os = "windows"))]
            { return Err("powershell 仅 Windows 支持".into()); }
        }
        "#python" => std::process::Command::new("python3").arg("-c").arg(&script).spawn(),
        _ => return Err(format!(
            "未知脚本类型: {}（第一行须为 #shell/#osascript/#powershell/#python）",
            first_line
        )),
    };

    result.map_err(|e| format!("脚本执行失败: {}", e))?;
    Ok(())
}

/// 统一执行菜单项动作。
#[tauri::command]
pub async fn execute_action_bar(item_id: i64, text: String, app: AppHandle) -> Result<(), String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(|e| e.to_string())?
        .ok_or("菜单项不存在")?;

    match item.action_type.as_str() {
        "ai" => {
            let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
            let llm_config = crate::config::llm_config_ignore_mode(&config)
                .ok_or("润色模型未配置，请在设置中配置 LLM")?;
            let prompt: &str = if item.action_data == "auto_translate" {
                auto_translate_prompt(&text)
            } else {
                &item.action_data
            };
            let result = octopus_llm::chat_text_with_prompt(prompt, &text, &llm_config)
                .map_err(|e| e.to_string())?;
            action_bar_show_result(result, text, item.title, app);
        }
        "url" => {
            let url = if item.action_data.is_empty() {
                text.clone()
            } else {
                item.action_data.replace("{text}", &simple_url_encode(&text))
            };
            #[cfg(target_os = "macos")]
            { let _ = std::process::Command::new("open").arg(&url).spawn(); }
            #[cfg(target_os = "windows")]
            { let _ = std::process::Command::new("cmd").args(["/c", "start", "", &url]).spawn(); }
            #[cfg(target_os = "linux")]
            { let _ = std::process::Command::new("xdg-open").arg(&url).spawn(); }
        }
        "script" => {
            run_script(&item.action_data, &text)?;
        }
        "copy" => {
            write_clipboard_text(&app, &text);
        }
        _ => {
            return Err(format!("未知动作类型: {}", item.action_type));
        }
    }

    Ok(())
}
