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
/// 重入 guard——防止热键连按导致 trigger 重叠执行
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
        // 重入 guard——防止热键连按导致 trigger 重叠执行
        if TRIGGER_IN_PROGRESS.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            log::info!("[action-bar] trigger already in progress, skipping");
            return;
        }

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
                clip_handle.clear_suppress();
                finalize_action_bar(&app_clone);
                return;
            }
        };

        if text.trim().is_empty() {
            log::warn!("[action-bar] Selected text is empty");
            clip_handle.clear_suppress();
            finalize_action_bar(&app_clone);
            return;
        }

        // suppress_next 已完成使命——watcher 有 200ms 窗口消费 flag。
        // 若剪贴板未变化（unchanged 路径），watcher 不触发，flag 残留会
        // 导致用户下次手动复制被静默吞掉，在此显式清除。
        // write_text 自带独立 suppress，不受此清除影响。
        clip_handle.clear_suppress();

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
        // 不截断——副屏在主屏左/上方时坐标可为负值
        let mut win_x = mx - 190.0;
        let win_y = my - 42.0;

        // 碰撞检测：防止浮窗溢出显示器边缘
        // Monitor position/size 返回物理像素，需 ÷ scale_factor() 转逻辑坐标
        const WIN_W: f64 = 380.0;
        if let Some(monitor) = app_clone.available_monitors().ok().and_then(|monitors| {
            monitors.into_iter().find(|m| {
                let scale = m.scale_factor();
                let mx_phys = m.position().x as f64;
                let my_phys = m.position().y as f64;
                let mw_phys = m.size().width as f64;
                let mh_phys = m.size().height as f64;
                let mon_left = mx_phys / scale;
                let mon_top = my_phys / scale;
                let mon_right = (mx_phys + mw_phys) / scale;
                let mon_bottom = (my_phys + mh_phys) / scale;
                mx >= mon_left && mx < mon_right && my >= mon_top && my < mon_bottom
            })
        }) {
            let scale = monitor.scale_factor();
            let mon_x = monitor.position().x as f64 / scale;
            let mon_w = monitor.size().width as f64 / scale;
            let mon_right = mon_x + mon_w;
            // 右溢出：贴右边缘
            if win_x + WIN_W > mon_right {
                win_x = mon_right - WIN_W;
            }
            // 左溢出：贴左边缘
            if win_x < mon_x {
                win_x = mon_x;
            }
        }

        log::info!("[action-bar] mouse=({},{}) → win_pos=({},{})", mx, my, win_x, win_y);

        let app_for_show = app_clone.clone();
        // NSWindow 操作（set_position/show/set_focus）必须在主线程执行，
        // async_runtime::spawn 跑在 tokio worker 线程，可能触发 AppKit 违规。
        let _ = app_clone.run_on_main_thread(move || {
            show_action_bar_window(&app_for_show, win_x, win_y);
        });
    });
}

/// action bar 所有出口的统一收口：重置重入 guard。
fn finalize_action_bar(_app: &AppHandle) {
    TRIGGER_IN_PROGRESS.store(false, Ordering::SeqCst);
}

/// 前端 mount 时拉取上下文。
#[tauri::command]
pub fn action_bar_get_context() -> Option<ActionBarContext> {
    // 非消耗读取（clone）——防止 mount + show 竞态导致第二次拿到 None
    PENDING_CONTEXT.lock().unwrap().clone()
}

/// 前端隐藏浮窗时调用。
#[tauri::command]
pub fn action_bar_dismiss(app: AppHandle) {
    hide_action_bar_window(&app);
    finalize_action_bar(&app);
}

/// AI 结果通过临时 tab 打开 CompactEditor 展示（不写 DB）。
/// 结果写入剪贴板留给用户——不恢复原始剪贴板（与 dismiss/open_url 不同）。
#[tauri::command]
pub fn action_bar_show_result(result: String, _original_text: String, action: String, app: AppHandle) {
    // 只隐藏浮窗本身——不调 hide_action_bar_window（含 after_floating_window_hide → deactivate），
    // 因为接下来要展示 CompactEditor，deactivate 会导致新窗口被压在后台。
    // 但必须递减 FLOAT_DEPTH + 恢复隐藏窗口（after_floating_window_hide_keep_active），
    // 否则 depth 永久泄漏导致后续焦点协调瘫痪。
    if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
        let _ = win.hide();
    }

    #[cfg(target_os = "macos")]
    { crate::activation::after_floating_window_hide_keep_active(&app); }

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
    _is_async: bool,
    _write_output_to_clipboard: bool,
) -> Result<i64, String> {
    // TODO Task 4 会接入新参数，先用默认值
    octopus_infra::db::insert_action_bar_item(parent_id, &title, &icon, &action_type, &action_data, true, false)
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
    _is_async: bool,
    _write_output_to_clipboard: bool,
) -> Result<(), String> {
    // TODO Task 4 会接入新参数，先用默认值
    octopus_infra::db::update_action_bar_item(id, &title, &icon, &action_type, &action_data, is_enabled, true, false)
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

/// URL 查询参数编码：保留 RFC 3986 unreserved（A-Za-z0-9-_.~），其余百分号编码。
/// 复用 percent-encoding 库（项目已为 file 协议引入）。
fn url_encode_param(s: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
    /// unreserved 之外需编码的 ASCII 字符
    const ENCODE_SET: &AsciiSet = &CONTROLS
        .add(b' ').add(b'!').add(b'"').add(b'#').add(b'$').add(b'%').add(b'&')
        .add(b'\'').add(b'(').add(b')').add(b'*').add(b'+').add(b',').add(b'/')
        .add(b':').add(b';').add(b'<').add(b'=').add(b'>').add(b'?').add(b'@')
        .add(b'[').add(b'\\').add(b']').add(b'^').add(b'`').add(b'{').add(b'|')
        .add(b'}');
    utf8_percent_encode(s, ENCODE_SET).to_string()
}

// ── 脚本执行（spawn_script + wait_with_timeout + 运行时探测）──

struct ScriptResult {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

/// 探测 JS 运行时——优先级 node → bun → deno
fn detect_js_runtime() -> Option<(&'static str, &'static str)> {
    for (bin, flag) in [("node", "-e"), ("bun", "eval"), ("deno", "eval")] {
        if std::process::Command::new(bin).arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().is_ok()
        {
            return Some((bin, flag));
        }
    }
    None
}

/// 探测 TS 运行时——优先级 npx tsx → bun → deno
fn detect_ts_runtime() -> Option<(&'static str, Vec<&'static str>)> {
    if std::process::Command::new("npx").args(["--yes", "tsx", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().is_ok()
    {
        return Some(("npx", vec!["--yes", "tsx", "-e"]));
    }
    if std::process::Command::new("bun").arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().is_ok()
    {
        return Some(("bun", vec!["eval"]));
    }
    if std::process::Command::new("deno").arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().is_ok()
    {
        return Some(("deno", vec!["eval"]));
    }
    None
}

/// 按 magic comment 分发运行时，spawn 子进程。
/// capture_output=true 时 stdout/stderr 用 pipe（同步模式），false 时用 null（异步模式）。
fn spawn_script(source: &str, text: &str, capture_output: bool) -> Result<(std::process::Child, String), String> {
    use std::process::Stdio;
    let first_line = source.lines().next().unwrap_or("").trim();
    let script: String = source.lines().skip(1).collect::<Vec<_>>().join("\n");

    let stdout_cfg = if capture_output { Stdio::piped() } else { Stdio::null() };
    let stderr_cfg = if capture_output { Stdio::piped() } else { Stdio::null() };

    let cmd_result: Result<std::process::Command, String> = match first_line {
        "#shell" => {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(&script);
            Ok(c)
        }
        "#osascript" => {
            #[cfg(target_os = "macos")]
            { let mut c = std::process::Command::new("osascript"); c.arg("-e").arg(&script); Ok(c) }
            #[cfg(not(target_os = "macos"))]
            { Err("osascript 仅 macOS 支持".into()) }
        }
        "#powershell" => {
            #[cfg(target_os = "windows")]
            { let mut c = std::process::Command::new("powershell"); c.arg("-Command").arg(&script); Ok(c) }
            #[cfg(not(target_os = "windows"))]
            { Err("powershell 仅 Windows 支持".into()) }
        }
        "#python" => {
            let mut c = std::process::Command::new("python3");
            c.arg("-c").arg(&script);
            Ok(c)
        }
        "#node" => {
            let mut c = std::process::Command::new("node");
            c.arg("-e").arg(&script);
            Ok(c)
        }
        "#deno" => {
            let mut c = std::process::Command::new("deno");
            c.arg("eval").arg(&script);
            Ok(c)
        }
        "#bun" => {
            let mut c = std::process::Command::new("bun");
            c.arg("eval").arg(&script);
            Ok(c)
        }
        "#javascript" => {
            let (bin, flag) = detect_js_runtime()
                .ok_or_else(|| "未检测到 JS 运行时，请安装 Node.js / Bun / Deno 之一".to_string())?;
            let mut c = std::process::Command::new(bin);
            c.arg(flag).arg(&script);
            Ok(c)
        }
        "#typescript" => {
            let (bin, args) = detect_ts_runtime()
                .ok_or_else(|| "未检测到 TS 运行时，请安装 tsx（npm i -g tsx）/ Bun / Deno 之一".to_string())?;
            let mut c = std::process::Command::new(bin);
            for a in &args { c.arg(a); }
            c.arg(&script);
            Ok(c)
        }
        _ => return Err(format!(
            "未知脚本类型: {}（第一行须为 #shell/#osascript/#powershell/#python/#node/#deno/#bun/#javascript/#typescript）",
            first_line
        )),
    };

    let mut cmd = cmd_result?;
    cmd.env("OCTOPUS_TEXT", text);
    cmd.stdout(stdout_cfg);
    cmd.stderr(stderr_cfg);
    let child = cmd.spawn().map_err(|e| format!("脚本执行失败: {}", e))?;
    Ok((child, first_line.to_string()))
}

/// 轮询等待子进程退出，60 秒超时强杀。捕获 stdout/stderr。
fn wait_with_timeout(mut child: std::process::Child) -> ScriptResult {
    use std::io::Read;
    for _ in 0..120 {
        match child.try_wait() {
            Ok(Some(_)) => {
                let mut stdout_buf = String::new();
                if let Some(ref mut stdout) = child.stdout { let _ = stdout.read_to_string(&mut stdout_buf); }
                let mut stderr_buf = String::new();
                if let Some(ref mut stderr) = child.stderr { let _ = stderr.read_to_string(&mut stderr_buf); }
                let code = child.wait().ok().and_then(|s| s.code());
                return ScriptResult { exit_code: code, stdout: stdout_buf, stderr: stderr_buf, timed_out: false };
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(500)),
            Err(_) => {
                let mut stdout_buf = String::new();
                if let Some(ref mut stdout) = child.stdout { let _ = stdout.read_to_string(&mut stdout_buf); }
                let mut stderr_buf = String::new();
                if let Some(ref mut stderr) = child.stderr { let _ = stderr.read_to_string(&mut stderr_buf); }
                return ScriptResult { exit_code: None, stdout: stdout_buf, stderr: stderr_buf, timed_out: false };
            }
        }
    }
    // 超时强杀
    let _ = child.kill();
    let _ = child.wait();
    let mut stdout_buf = String::new();
    if let Some(ref mut stdout) = child.stdout { let _ = stdout.read_to_string(&mut stdout_buf); }
    let mut stderr_buf = String::new();
    if let Some(ref mut stderr) = child.stderr { let _ = stderr.read_to_string(&mut stderr_buf); }
    ScriptResult { exit_code: None, stdout: stdout_buf, stderr: stderr_buf, timed_out: true }
}

/// 执行菜单项动作核心逻辑（不含收口）。
/// Ok(true) = ai 已自行收口；Ok(false) = 成功需外层统一收口；Err = 异常需外层 finalize。
fn execute_action_bar_inner(item_id: i64, text: String, app: &AppHandle) -> Result<bool, String> {
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
            action_bar_show_result(result, text, item.title, app.clone());
            Ok(true)
        }
        "url" => {
            let url = if item.action_data.is_empty() {
                // 选中文本即 URL——补 scheme（缺 https:// 时 macOS open 当文件路径）
                let raw = text.trim();
                if raw.starts_with("http://") || raw.starts_with("https://") || raw.contains("://") {
                    raw.to_string()
                } else {
                    format!("https://{}", raw)
                }
            } else {
                item.action_data.replace("{text}", &url_encode_param(&text))
            };
            #[cfg(target_os = "macos")]
            { let _ = std::process::Command::new("open").arg(&url).spawn(); }
            #[cfg(target_os = "windows")]
            { let _ = std::process::Command::new("cmd").args(["/c", "start", "", &url]).spawn(); }
            #[cfg(target_os = "linux")]
            { let _ = std::process::Command::new("xdg-open").arg(&url).spawn(); }
            Ok(false)
        }
        "script" => {
            // 临时 fire-and-forget（Task 3 会重构为 async/sync 模式 + 结果捕获）
            let (mut child, _script_type) = spawn_script(&item.action_data, &text, false)?;
            std::thread::spawn(move || {
                let _ = wait_with_timeout(child);
            });
            Ok(false)
        }
        "copy" => {
            write_clipboard_text(app, &text);
            Ok(false)
        }
        _ => Err(format!("未知动作类型: {}", item.action_type)),
    }
}

/// 统一执行菜单项动作。
#[tauri::command]
pub async fn execute_action_bar(item_id: i64, text: String, app: AppHandle) -> Result<(), String> {
    match execute_action_bar_inner(item_id, text, &app) {
        Ok(true) => Ok(()),
        Ok(false) => {
            // url/script/copy 成功 → 统一收口：标准隐藏 + 焦点交还 + 重入锁复位
            // hide_action_bar_window 含 after_floating_window_hide（NSApplication::deactivate），
            // 本 command 是 async → 跑在 tokio worker 线程，MainThreadMarker::new() 返回 None
            // 导致 deactivate 静默跳过。投递到主线程执行（与 trigger_action_bar 的 show 同模式）。
            let app_for_hide = app.clone();
            let _ = app.run_on_main_thread(move || {
                hide_action_bar_window(&app_for_hide);
            });
            finalize_action_bar(&app);
            Ok(())
        }
        Err(e) => {
            // 异常路径：仅重置重入锁（不 hide——前端切 error 视图需窗口可见，
            // error 视图关闭时 action_bar_dismiss 走 hide + after_hide 递减 depth）
            finalize_action_bar(&app);
            Err(e)
        }
    }
}
