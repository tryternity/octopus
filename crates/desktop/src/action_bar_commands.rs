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

        // 1. 记录触发前的剪贴板内容（文本 + 图片 + 文件，防非文本数据丢失）
        let clip_handle = app_clone.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>().clone();
        let clipboard_before_text = read_clipboard_text(&app_clone);
        let clipboard_before_image = if clip_handle.has_image() {
            clip_handle.read_image().ok()
        } else { None };
        let clipboard_before_files = if clip_handle.has_files() {
            clip_handle.read_files().ok().filter(|f| !f.is_empty())
        } else { None };

        // 2. suppress watcher
        clip_handle.suppress_next();

        let focus = FocusTracker::new();
        focus.simulate_copy();

        // 3. 等待 200ms 让系统完成复制
        std::thread::sleep(std::time::Duration::from_millis(200));

        // 4. 读剪贴板拿到选中文本
        let clipboard_after = read_clipboard_text(&app_clone);
        let text = match (&clipboard_before_text, &clipboard_after) {
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

        // 5. 恢复原始剪贴板内容（优先级：文件 > 图片 > 文本）
        if let Some(ref files) = clipboard_before_files {
            let _ = clip_handle.write_files(files.clone());
        } else if let Some(img) = clipboard_before_image {
            let _ = clip_handle.set_image(img);
        } else if let Some(ref original) = clipboard_before_text {
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
pub fn action_bar_show_result(result: String, _original_text: String, action: String, app: AppHandle, write_clipboard: bool) {
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
        // script 同步路径传入的 action 是菜单项 title，直接用作 label
        _ => &action,
    };
    let display_text = format!("【{}】\n{}", label, result);

    // 结果写入系统剪贴板（仅 write_clipboard=true 时）——write_text 自带 suppress 不会入库
    if write_clipboard {
        write_clipboard_text(&app, &result);
    }

    finalize_action_bar(&app);

    // 用临时 tab 打开 CompactEditor（不写 DB，保存按钮灰掉）。决策逻辑见
    // compact_editor_commands::open_temp_compact_editor（托盘「图文编辑」复用同一路径）。
    crate::compact_editor_commands::open_temp_compact_editor(&app, &display_text);
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
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: String,
) -> Result<i64, String> {
    // 同级菜单项最多 35 个（9 数字 + 26 字母快捷键上限）
    let all = octopus_infra::db::list_all_action_bar_items().map_err(|e| e.to_string())?;
    let sibling_count = all.iter().filter(|i| i.parent_id == parent_id).count();
    if sibling_count >= 35 {
        return Err("同级菜单项已达上限 35 个（快捷键 1-9 + a-z）".into());
    }
    octopus_infra::db::insert_action_bar_item(parent_id, &title, &icon, &action_type, &action_data, is_async, write_output_to_clipboard, &shortcut)
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
    is_async: bool,
    write_output_to_clipboard: bool,
    shortcut: String,
) -> Result<(), String> {
    octopus_infra::db::update_action_bar_item(id, &title, &icon, &action_type, &action_data, is_enabled, is_async, write_output_to_clipboard, &shortcut)
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

// ── 脚本执行记录 ──

#[tauri::command]
pub fn list_script_runs(limit: Option<i64>, item_id: Option<i64>) -> Result<Vec<octopus_infra::db::ScriptRun>, String> {
    octopus_infra::db::list_script_runs(limit, item_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_script_runs(keep_recent: Option<i64>) -> Result<(), String> {
    octopus_infra::db::clear_script_runs(keep_recent).map_err(|e| e.to_string())
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

enum TranslateStrategy {
    /// 本地翻译引擎 spec（如 "local:m2m100"），引擎加载延迟到后台线程
    Local(String),
    Llm,
}

fn resolve_translate_strategy(config: &octopus_infra::config::AppConfig) -> TranslateStrategy {
    match config.translate_engine.as_str() {
        "llm" => TranslateStrategy::Llm,
        spec if spec.starts_with("local:") => TranslateStrategy::Local(spec.to_string()),
        _ => {
            // 自动：有本地则用本地
            let models = octopus_translation::discover_translation_models();
            if models.iter().any(|m| m.downloaded) {
                TranslateStrategy::Local("local:m2m100-418M".into())
            } else {
                TranslateStrategy::Llm
            }
        }
    }
}

fn detect_translate_direction(text: &str) -> (&'static str, &'static str) {
    let has_cjk = text.chars().any(|c| {
        matches!(c as u32, 0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
    });
    if has_cjk {
        ("zh", "en")
    } else {
        ("en", "zh")
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

/// 从 ScriptResult 生成 error_msg——超时/异常退出/非零退出码都有描述
fn script_error_msg(result: &ScriptResult) -> String {
    if result.timed_out {
        "执行超时（60秒）".to_string()
    } else if result.exit_code.is_none() {
        "进程异常退出".to_string()
    } else if result.exit_code != Some(0) {
        format!("进程以错误码 {} 退出", result.exit_code.unwrap())
    } else {
        String::new()
    }
}

use std::sync::OnceLock;

/// 探测 JS 运行时——优先级 node → bun → deno（结果缓存，仅首次探测）
fn detect_js_runtime() -> Option<(&'static str, &'static str)> {
    static CACHE: OnceLock<Option<(&'static str, &'static str)>> = OnceLock::new();
    *CACHE.get_or_init(|| {
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
    })
}

/// 探测 TS 运行时——优先级 bun → deno → npx tsx（结果缓存，仅首次探测）
fn detect_ts_runtime() -> Option<(&'static str, Vec<&'static str>)> {
    static CACHE: OnceLock<Option<(&'static str, Vec<&'static str>)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        // bun/deno 原生支持 TS，探测仅本地进程，毫秒级
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
        // npx tsx 作为 fallback（可能触发联网下载，最慢）
        if std::process::Command::new("npx").args(["--yes", "tsx", "--version"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status().is_ok()
        {
            return Some(("npx", vec!["--yes", "tsx", "-e"]));
        }
        None
    }).clone()
}

/// 按 magic comment 分发运行时，spawn 子进程。
/// capture_output=true 时 stdout/stderr 用 pipe（同步模式），false 时用 null（异步模式）。
fn spawn_script(source: &str, text: &str, capture_output: bool, pkg_dir: &Option<String>) -> Result<(std::process::Child, String, Option<std::path::PathBuf>), String> {
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
            #[cfg(target_os = "windows")]
            let py = "python";
            #[cfg(not(target_os = "windows"))]
            let py = "python3";
            let mut c = std::process::Command::new(py);
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
    // 选中文本传递：≤200KB 用环境变量 OCTOPUS_TEXT；超出写临时文件，
    // OCTOPUS_TEXT 设为 "_____ULTRA_LONG_TEXT_____:/tmp/octopus-text-xxx" 供消费方读取
    let mut text_tmp: Option<std::path::PathBuf> = None;
    const TEXT_LIMIT: usize = 200_000;
    if text.len() > TEXT_LIMIT {
        let tmp_path = std::env::temp_dir().join(format!(
            "octopus-text-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        ));
        if let Err(e) = std::fs::write(&tmp_path, text) {
            log::warn!("[script] 临时文件写入失败，回退截断: {}", e);
            // 按字节截断（非字符），确保 UTF-8 边界安全 + 严格 < TEXT_LIMIT 字节
            let mut end = TEXT_LIMIT;
            while !text.is_char_boundary(end) { end -= 1; }
            cmd.env("OCTOPUS_TEXT", &text[..end]);
        } else {
            let marker = format!(
                "_____ULTRA_LONG_TEXT_____:{}",
                tmp_path.to_string_lossy()
            );
            cmd.env("OCTOPUS_TEXT", &marker);
            text_tmp = Some(tmp_path);
        }
    } else {
        cmd.env("OCTOPUS_TEXT", text);
    }
    if let Some(dir) = pkg_dir {
        cmd.env("OCTOPUS_PACKAGE_DIR", dir);
    }
    cmd.stdout(stdout_cfg);
    cmd.stderr(stderr_cfg);
    let child = cmd.spawn().map_err(|e| {
        // spawn 失败——清理临时文件防泄露
        if let Some(ref p) = text_tmp { let _ = std::fs::remove_file(p); }
        format!("脚本执行失败: {}", e)
    })?;
    Ok((child, first_line.to_string(), text_tmp))
}

/// 轮询等待子进程退出，60 秒超时强杀。并发读取 stdout/stderr 防管道死锁。
fn wait_with_timeout(child: std::process::Child) -> ScriptResult {
    wait_with_timeout_secs(child, 60)
}

/// 异步脚本等待——不超时，阻塞等待自然退出（0 CPU 占用）。
fn wait_forever(mut child: std::process::Child) -> ScriptResult {
    use std::io::Read;

    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut stdout) = stdout_handle { let _ = stdout.read_to_string(&mut buf); }
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut stderr) = stderr_handle { let _ = stderr.read_to_string(&mut buf); }
        buf
    });

    // 阻塞等待子进程退出——无轮询，CPU 0%
    let code = child.wait().ok().and_then(|s| s.code());

    let stdout_buf = stdout_thread.join().unwrap_or_default();
    let stderr_buf = stderr_thread.join().unwrap_or_default();
    ScriptResult { exit_code: code, stdout: stdout_buf, stderr: stderr_buf, timed_out: false }
}

fn wait_with_timeout_secs(mut child: std::process::Child, timeout_secs: u32) -> ScriptResult {
    use std::io::Read;

    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut stdout) = stdout_handle { let _ = stdout.read_to_string(&mut buf); }
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut stderr) = stderr_handle { let _ = stderr.read_to_string(&mut buf); }
        buf
    });

    let mut timed_out = false;
    let polls = timeout_secs.saturating_mul(2); // 500ms × 2 = 1s
    for _ in 0..polls {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(500)),
            Err(_) => break,
        }
    }
    if child.try_wait().map(|s| s.is_none()).unwrap_or(true) {
        let _ = child.kill();
        let _ = child.wait();
        timed_out = true;
    }

    let stdout_buf = stdout_thread.join().unwrap_or_default();
    let stderr_buf = stderr_thread.join().unwrap_or_default();
    let code = child.wait().ok().and_then(|s| s.code());
    ScriptResult { exit_code: code, stdout: stdout_buf, stderr: stderr_buf, timed_out }
}

/// 生成 ISO 8601 时间戳（UTC），不依赖 chrono
fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!("{}", secs)
}

/// 异步执行脚本——spawn 后立即返回，后台线程收割并落库
fn run_script_async(source: &str, text: &str, item_id: i64, pkg_dir: Option<String>) -> Result<(), String> {
    let (child, script_type, text_tmp) = spawn_script(source, text, false, &pkg_dir)?;
    let started = std::time::Instant::now();
    let started_at = now_iso8601();
    std::thread::spawn(move || {
        let result = wait_forever(child);
        // 清理超长文本临时文件
        if let Some(ref p) = text_tmp { let _ = std::fs::remove_file(p); }
        let duration_ms = started.elapsed().as_millis() as i64;
        let finished_at = now_iso8601();
        let error_msg = script_error_msg(&result);
        let _ = octopus_infra::db::insert_script_run(
            item_id, &script_type, result.exit_code,
            &result.stdout, &result.stderr, &error_msg,
            &started_at, Some(&finished_at), Some(duration_ms),
        );
    });
    Ok(())
}

/// 同步执行脚本（阻塞）——等待完成，返回结果并落库
fn run_script_sync_blocking(source: &str, text: &str, item_id: i64, pkg_dir: Option<String>) -> Result<ScriptResult, String> {
    let (child, script_type, text_tmp) = spawn_script(source, text, true, &pkg_dir)?;
    let started = std::time::Instant::now();
    let started_at = now_iso8601();
    let mut result = wait_with_timeout(child);
    // 清理超长文本临时文件
    if let Some(ref p) = text_tmp { let _ = std::fs::remove_file(p); }
    let duration_ms = started.elapsed().as_millis() as i64;
    let finished_at = now_iso8601();
    let error_msg = script_error_msg(&result);
    let _ = octopus_infra::db::insert_script_run(
        item_id, &script_type, result.exit_code,
        &result.stdout, &result.stderr, &error_msg,
        &started_at, Some(&finished_at), Some(duration_ms),
    );
    // 标记已落库（ScriptResult 原样返回给上层）
    let _ = &mut result; // 消费 mut borrow
    Ok(result)
}

/// 执行菜单项动作核心逻辑（不含收口）。
/// Ok(true) = ai 已自行收口；Ok(false) = 成功需外层统一收口；Err = 异常需外层 finalize。
async fn execute_action_bar_inner(item_id: i64, text: String, app: &AppHandle) -> Result<bool, String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(|e| e.to_string())?
        .ok_or("菜单项不存在")?;

    match item.action_type.as_str() {
        "ai" => {
            let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;

            // 翻译特殊处理：优先本地引擎
            if item.action_data == "auto_translate" {
                let (source_lang, target_lang) = detect_translate_direction(&text);
                match resolve_translate_strategy(&config) {
                    TranslateStrategy::Local(spec) => {
                        // 本地翻译耗时很长——立即打开 CompactEditor 显示 loading，
                        // 引擎加载 + 翻译都在后台线程执行，不阻塞主线程
                        action_bar_show_result("⏳ 正在翻译…".into(), text.clone(), item.title.clone(), app.clone(), false);

                        let app_clone = app.clone();
                        std::thread::spawn(move || {
                            let manager = octopus_translation::TranslationManager::new(&spec);
                            match manager.engine() {
                                Ok(Some(engine)) => {
                                    let result = engine.translate(&text, source_lang, target_lang);
                                    match result {
                                        Ok(translated) => {
                                            let display = format!("【翻译】\n{}", translated);
                                            let _ = app_clone.emit("translate-done", &display);
                                        }
                                        Err(e) => {
                                            let _ = app_clone.emit("translate-done", format!("【翻译】\n❌ {}", e));
                                        }
                                    }
                                }
                                _ => {
                                    let _ = app_clone.emit("translate-done", "【翻译】\n❌ 引擎加载失败");
                                }
                            }
                        });
                        return Ok(true);
                    }
                    TranslateStrategy::Llm => {
                        let llm_config = crate::config::llm_config_ignore_mode(&config)
                            .ok_or("润色模型未配置，请在设置中配置 LLM")?;
                        let prompt = auto_translate_prompt(&text);
                        let result = octopus_llm::chat_text_with_prompt(prompt, &text, &llm_config)
                        .map_err(|e| e.to_string())?;
                        action_bar_show_result(result, text, item.title, app.clone(), true);
                        return Ok(true);
                    }
                }
            }

            // 非 auto_translate 的 AI 操作（润色/摘要/解释），仍走 LLM
            let llm_config = crate::config::llm_config_ignore_mode(&config)
                .ok_or("润色模型未配置，请在设置中配置 LLM")?;
            let result = octopus_llm::chat_text_with_prompt(&item.action_data, &text, &llm_config)
                .map_err(|e| e.to_string())?;
            action_bar_show_result(result, text, item.title, app.clone(), true);
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
            let is_async = item.is_async;
            let write_output = item.write_output_to_clipboard;
            let item_title = item.title.clone();
            let item_id = item.id;

            // Package 脚本（action_data 是绝对路径）vs 内联脚本
            let is_pkg = std::path::Path::new(&item.action_data).is_absolute();
            let source = if is_pkg {
                std::fs::read_to_string(&item.action_data)
                    .map_err(|e| format!("脚本文件不存在或无法读取: {}", e))?
            } else {
                item.action_data.clone()
            };
            let pkg_dir = if is_pkg {
                std::path::Path::new(&item.action_data).parent()
                    .map(|p| p.to_string_lossy().to_string())
            } else { None };

            if is_async {
                run_script_async(&source, &text, item_id, pkg_dir)?;
                Ok(false)
            } else {
                let text_clone = text.clone();
                let result = tokio::task::spawn_blocking(move || {
                    run_script_sync_blocking(&source, &text_clone, item_id, pkg_dir)
                }).await.map_err(|e| format!("脚本执行线程异常: {}", e))??;

                if result.timed_out {
                    return Err("脚本执行超时（60秒），已强制终止".into());
                }
                if let Some(code) = result.exit_code {
                    if code != 0 {
                        let detail = if result.stderr.is_empty() { String::new() } else { format!("\n{}", result.stderr) };
                        return Err(format!("脚本退出码 {}{}", code, detail));
                    }
                }
                // 成功
                if !result.stdout.is_empty() {
                    if write_output {
                        write_clipboard_text(app, &result.stdout);
                    }
                    action_bar_show_result(result.stdout, text, item_title, app.clone(), false);
                    return Ok(true);
                }
                // 成功无输出 → 正常关闭
                Ok(false)
            }
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
    match execute_action_bar_inner(item_id, text, &app).await {
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
