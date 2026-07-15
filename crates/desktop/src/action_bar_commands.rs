//! AI 命令面板后端命令——模拟 Cmd+C / LLM 调用 / 模拟 Cmd+V / 打开 URL。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

use crate::action_bar_window::{hide_action_bar_window, show_action_bar_window};
use crate::focus_tracker::FocusTracker;

/// 选中对象类型。
#[derive(Clone, serde::Serialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ContextKind {
    Text,
    Files,
}

/// 暂存选中对象 + 上下文（trigger 时写入，前端 mount 时读）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    pub kind: ContextKind,
    pub text: Option<String>,
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::app_context::AppSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surrounding: Option<crate::app_context::SurroundingText>,
}

impl ActionBarContext {
    pub fn text(text: String) -> Self {
        Self { kind: ContextKind::Text, text: Some(text), files: vec![], source: None, surrounding: None }
    }
    pub fn files(files: Vec<String>) -> Self {
        Self { kind: ContextKind::Files, text: None, files, source: None, surrounding: None }
    }
}

static PENDING_CONTEXT: Mutex<Option<ActionBarContext>> = Mutex::new(None);
/// 重入 guard——防止热键连按导致 trigger 重叠执行
static TRIGGER_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// 热键触发：模拟 Cmd+C → 读剪贴板 → 获取鼠标位置 → 显示浮窗。
/// 一次触发检测出的完整选中状态——后端唯一的"有什么选中"真相源。
/// 检测完成后，下游所有操作仅读这个枚举，不再碰 changeCount / 剪贴板 / 鼠标坐标。
///
/// 鼠标坐标始终在检测阶段采集（无论有无选中），用于：
///   - 有选中 → 鼠标位置弹出
///   - 无选中 → 忽略，用主屏居中
enum Selection {
    /// 无选中——居中搜索模式
    None,
    /// 选中文本
    Text {
        text: String,
        mouse: (f64, f64),
    },
    /// 选中文件（Finder）
    File {
        files: Vec<String>,
        #[allow(dead_code)]
        parent_dir: Option<String>,
        mouse: (f64, f64),
    },
    /// 选中文件夹（Finder）
    Folder {
        folders: Vec<String>,
        #[allow(dead_code)]
        parent_dir: Option<String>,
        mouse: (f64, f64),
    },
}

impl Selection {
    /// 是否有选中（None → false，其余 → true）
    #[allow(dead_code)]
    fn has_selection(&self) -> bool {
        !matches!(self, Selection::None)
    }

    /// 鼠标坐标（None 时返回 (0,0) 不使用）
    fn mouse(&self) -> (f64, f64) {
        match self {
            Selection::None => (0.0, 0.0),
            Selection::Text { mouse, .. }
            | Selection::File { mouse, .. }
            | Selection::Folder { mouse, .. } => *mouse,
        }
    }
}

/// 从路径列表提取公共父目录。
fn common_parent_dir(paths: &[String]) -> Option<String> {
    if paths.is_empty() { return None; }
    let first = std::path::Path::new(&paths[0]);
    let parent = first.parent()?;
    parent.to_str().map(|s| s.to_string())
}

/// 检测当前选中状态。Finder 走 AppleScript，其余走 Cmd+C + changeCount。
/// 返回的 Selection 携带全部信息（选中内容 + 鼠标坐标），下游不再碰检测细节。
fn detect_selection(app: &AppHandle) -> Selection {
    // 鼠标坐标在检测开始时就采集（后续 Cmd+C 等 sleep 不影响坐标）
    let mouse = get_mouse_position(app);

    // ── Finder 分支：AppleScript 直接拿 selection ──
    if crate::finder_selection::is_finder_frontmost() {
        return match crate::finder_selection::get_finder_selection() {
            Ok(files) if !files.is_empty() => {
                let has_folder = files.iter().any(|p| std::path::Path::new(p).is_dir());
                let parent_dir = common_parent_dir(&files);
                if has_folder {
                    log::info!("[action-bar] Finder: {} folders", files.len());
                    Selection::Folder { folders: files, parent_dir, mouse }
                } else {
                    log::info!("[action-bar] Finder: {} files", files.len());
                    Selection::File { files, parent_dir, mouse }
                }
            }
            Ok(_) => {
                log::info!("[action-bar] Finder 空选中");
                Selection::None
            }
            Err(e) => {
                log::warn!("[action-bar] Finder selection 失败: {}", e);
                Selection::None
            }
        };
    }

    // ── 非 Finder 分支：Cmd+C + changeCount 判断有无选中文本 ──
    let clip_handle = app.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>().clone();
    let clipboard_before_text = read_clipboard_text(app);
    let clipboard_before_image = if clip_handle.has_image() {
        clip_handle.read_image().ok()
    } else { None };
    let clipboard_before_files = if clip_handle.has_files() {
        clip_handle.read_files().ok().filter(|f| !f.is_empty())
    } else { None };

    clip_handle.suppress_next();
    let change_count_before = pasteboard_change_count();

    let focus = FocusTracker::new();
    focus.simulate_copy();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let change_count_after = pasteboard_change_count();
    if change_count_after == change_count_before {
        log::info!("[action-bar] changeCount unchanged {}→{} = no selection",
            change_count_before, change_count_after);
        clip_handle.clear_suppress();
        return Selection::None;
    }

    // changeCount 递增 → 有选中，读剪贴板拿文本
    let clipboard_after = read_clipboard_text(app);
    let text = match &clipboard_after {
        Some(t) if !t.trim().is_empty() => t.clone(),
        _ => {
            log::info!("[action-bar] changeCount changed but clipboard empty");
            clip_handle.clear_suppress();
            return Selection::None;
        }
    };

    // 恢复原始剪贴板
    clip_handle.clear_suppress();
    if let Some(ref files) = clipboard_before_files {
        let _ = clip_handle.write_files(files.clone());
    } else if let Some(img) = clipboard_before_image {
        let _ = clip_handle.set_image(img);
    } else if let Some(ref original) = clipboard_before_text {
        if clipboard_after.as_deref() != Some(original.as_str()) {
            write_clipboard_text(app, original);
        }
    }

    log::info!("[action-bar] got text len={}, mouse=({},{})", text.len(), mouse.0, mouse.1);
    Selection::Text { text, mouse }
}

#[tauri::command]
pub fn trigger_action_bar(app: AppHandle) {
    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("[action-bar] 仅 macOS 支持此功能");
        let _ = app;
        return;
    }

    let app_clone = app.clone();
    std::thread::spawn(move || {
        if TRIGGER_IN_PROGRESS.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            log::info!("[action-bar] trigger already in progress, skipping");
            return;
        }

        // ── 检测：一次性拿到全部信息，changeCount 只在这里出现 ──
        let sel = detect_selection(&app_clone);

        // ── 路由：仅依赖 Selection，不再碰检测细节 ──
        match &sel {
            Selection::None => {
                *PENDING_CONTEXT.lock().unwrap() = None;
                show_action_bar_centered(&app_clone);
                finalize_action_bar(&app_clone);
            }
            Selection::Text { text, mouse } => {
                let text_for_gather = text.clone();
                *PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext::text(text.clone()));
                show_action_bar_at_mouse_with_pos(&app_clone, *mouse);
                // 后台采集上下文
                std::thread::spawn(move || {
                    match crate::app_context::gather_context(&text_for_gather) {
                        Ok(extra) => {
                            log_app_context(&text_for_gather, &extra);
                            let mut guard = PENDING_CONTEXT.lock().unwrap();
                            if let Some(ref ctx) = *guard {
                                if ctx.text.as_deref() == Some(&text_for_gather) {
                                    if let Some(ref mut ctx) = *guard {
                                        ctx.source = Some(extra.source);
                                        ctx.surrounding = extra.surrounding;
                                    }
                                } else {
                                    log::info!("[action-bar] gather 回填跳过：ctx 已被新触发覆盖");
                                }
                            }
                        }
                        Err(e) => log::warn!("[action-bar] context gather 失败（降级到仅 text）: {}", e),
                    }
                });
            }
            Selection::File { files, .. } => {
                *PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext::files(files.clone()));
                show_action_bar_at_mouse_with_pos(&app_clone, sel.mouse());
            }
            Selection::Folder { folders, .. } => {
                *PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext::files(folders.clone()));
                show_action_bar_at_mouse_with_pos(&app_clone, sel.mouse());
            }
        }
    });
}

/// 在指定鼠标坐标附近显示浮窗（含碰撞检测）。
fn show_action_bar_at_mouse_with_pos(app: &AppHandle, mouse: (f64, f64)) {
    let (mx, my) = mouse;
    // 不截断——副屏在主屏左/上方时坐标可为负值
    let mut win_x = mx - 190.0;
    let win_y = my - 42.0;

    // 碰撞检测：防止浮窗溢出显示器边缘
    const WIN_W: f64 = 380.0;
    if let Some(monitor) = app.available_monitors().ok().and_then(|monitors| {
        monitors.into_iter().find(|m| {
            let scale = m.scale_factor();
            let mon_left = m.position().x as f64 / scale;
            let mon_top = m.position().y as f64 / scale;
            let mon_right = (m.position().x as f64 + m.size().width as f64) / scale;
            let mon_bottom = (m.position().y as f64 + m.size().height as f64) / scale;
            mx >= mon_left && mx < mon_right && my >= mon_top && my < mon_bottom
        })
    }) {
        let scale = monitor.scale_factor();
        let mon_x = monitor.position().x as f64 / scale;
        let mon_w = monitor.size().width as f64 / scale;
        let mon_right = mon_x + mon_w;
        if win_x + WIN_W > mon_right { win_x = mon_right - WIN_W; }
        if win_x < mon_x { win_x = mon_x; }
    }

    log::info!("[action-bar] mouse=({},{}) → win_pos=({},{})", mx, my, win_x, win_y);

    let app_for_show = app.clone();
    let _ = app.run_on_main_thread(move || {
        show_action_bar_window(&app_for_show, win_x, win_y);
    });
}

/// 无选中时在主屏幕居中显示浮窗——水平居中，垂直位于屏幕上 1/5 位置（类似 Alfred/Wox）。
fn show_action_bar_centered(app: &AppHandle) {
    const WIN_W: f64 = 380.0;

    // 强制用主显示器
    let (mon_x, mon_y, mon_w, mon_h) = match app.primary_monitor().ok().flatten() {
        Some(m) => {
            let scale = m.scale_factor();
            (
                m.position().x as f64 / scale,
                m.position().y as f64 / scale,
                m.size().width as f64 / scale,
                m.size().height as f64 / scale,
            )
        }
        None => (0.0, 0.0, 1440.0, 900.0),
    };

    let win_x = mon_x + (mon_w - WIN_W) / 2.0;
    // 上 1/5 位置
    let win_y = mon_y + mon_h / 5.0;

    log::info!("[action-bar] centered: monitor=({},{},{},{}) → win_pos=({},{})", mon_x, mon_y, mon_w, mon_h, win_x, win_y);

    let app_for_show = app.clone();
    let _ = app.run_on_main_thread(move || {
        show_action_bar_window(&app_for_show, win_x, win_y);
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
pub fn action_bar_show_result(result: String, original_text: String, action: String, app: AppHandle, write_clipboard: bool) {
    action_bar_show_result_internal(result, original_text, action, app, write_clipboard, false);
}

/// Run And Paste 模式：写剪贴板 + 模拟 ⌘V，不弹 CompactEditor。
pub fn action_bar_run_and_paste(result: String, app: AppHandle) {
    action_bar_show_result_internal(result, String::new(), String::new(), app, true, true);
}

fn action_bar_show_result_internal(
    result: String,
    _original_text: String,
    _action: String,
    app: AppHandle,
    write_clipboard: bool,
    auto_paste: bool,
) {
    // 只隐藏浮窗本身——不调 hide_action_bar_window（含 after_floating_window_hide → deactivate），
    // 因为接下来要展示 CompactEditor，deactivate 会导致新窗口被压在后台。
    // 但必须递减 FLOAT_DEPTH + 恢复隐藏窗口（after_floating_window_hide_keep_active），
    // 否则 depth 永久泄漏导致后续焦点协调瘫痪。
    if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
        let _ = win.hide();
    }

    #[cfg(target_os = "macos")]
    { crate::activation::after_floating_window_hide_keep_active(&app); }

    // 结果写入系统剪贴板——write_text 自带 suppress 不会入库
    if write_clipboard || auto_paste {
        write_clipboard_text(&app, &result);
    }

    finalize_action_bar(&app);

    // ── Run And Paste 模式：写剪贴板 + 模拟 ⌘V，不弹 CompactEditor ──
    if auto_paste {
        log::info!("[action-bar] Run And Paste: result written to clipboard, simulating ⌘V");
        let app_clone = app.clone();
        let paste_result = result.clone();
        std::thread::spawn(move || {
            // 等待 clipboard 写入完成 + 窗口隐藏
            std::thread::sleep(std::time::Duration::from_millis(100));
            let config = octopus_infra::config::load_config().unwrap_or_default();
            let handle = app_clone.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>();
            if let Err(e) = crate::paste::paste(&paste_result, &handle, &config) {
                log::warn!("[action-bar] Run And Paste failed: {}", e);
            }
        });
        return;
    }

    // ── 常规模式：打开 CompactEditor 展示结果 ──
    let label = match _action.as_str() {
        "translate" => "翻译",
        "polish" => "润色",
        "summarize" => "摘要",
        "explain" => "解释",
        // script 同步路径传入的 action 是菜单项 title，直接用作 label
        _ => &_action,
    };
    let display_text = format!("【{}】\n{}", label, result);

    // 用临时 tab 打开 CompactEditor（不写 DB，保存按钮灰掉）。
    // 翻译 action 且有原文 → contrast 模式（左原文右译文）；其他 → single。
    let payload = if _action == "translate" && !_original_text.is_empty() {
        crate::compact_editor_commands::TempTabPayload {
            text: display_text.clone(),
            mode: Some("contrast".into()),
            original_text: Some(_original_text),
            translated_text: Some(result.clone()),
        }
    } else {
        crate::compact_editor_commands::TempTabPayload {
            text: display_text,
            ..Default::default()
        }
    };
    // 投递主线程——create_compact_editor_window 内含 set_dock_icon 需主线程
    let app_for_editor = app.clone();
    let _ = app.run_on_main_thread(move || {
        crate::compact_editor_commands::open_temp_compact_editor(&app_for_editor, &payload);
    });
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

/// 读取系统剪贴板的 changeCount（macOS NSPasteboard）。
/// 每次 Cmd+C（或程序写剪贴板）都会递增，与内容是否相同无关。
/// 用于判断 Cmd+C 是否真正产生了复制操作（有无选中文本）。
#[cfg(target_os = "macos")]
fn pasteboard_change_count() -> i64 {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    unsafe {
        let cls = objc2::class!(NSPasteboard);
        let pb: *mut AnyObject = msg_send![cls, generalPasteboard];
        let count: i64 = msg_send![pb, changeCount];
        count
    }
}

#[cfg(not(target_os = "macos"))]
fn pasteboard_change_count() -> i64 { 0 }

/// 将采集到的应用上下文以结构化文本追加写入 ~/.octopus/logs/action-bar.log，
/// 方便直接验证 AX 取数结果（而非通过 AI 结果间接判断）。
fn log_app_context(selected_text: &str, extra: &crate::app_context::ExtraContext) {
    let log_path = context_log_path();

    let entry = format_context_entry(selected_text, extra);

    if let Err(e) = write_context_log(&log_path, &entry) {
        log::warn!("[action-bar] 上下文日志写入失败 {}: {}", log_path.display(), e);
    }
}

/// 默认日志路径：~/.octopus/logs/action-bar.log
fn context_log_path() -> std::path::PathBuf {
    let mut p = dirs::home_dir().unwrap_or_default();
    p.push(".octopus");
    p.push("logs");
    p.push("action-bar.log");
    p
}

/// 将上下文格式化为日志条目（纯函数，可单测）。
fn format_context_entry(selected_text: &str, extra: &crate::app_context::ExtraContext) -> String {
    let kind_label = match extra.source.kind {
        crate::app_context::AppKind::Editor => "Editor",
        crate::app_context::AppKind::Terminal => "Terminal",
        crate::app_context::AppKind::Browser => "Browser",
        crate::app_context::AppKind::Chat => "Chat",
        crate::app_context::AppKind::Unknown => "Unknown",
    };

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");

    let before_preview = extra
        .surrounding
        .as_ref()
        .and_then(|s| s.before.as_ref())
        .map(|b| truncate_for_log(b, 500))
        .unwrap_or_else(|| "(无)".to_string());

    let after_preview = extra
        .surrounding
        .as_ref()
        .and_then(|s| s.after.as_ref())
        .map(|a| truncate_for_log(a, 500))
        .unwrap_or_else(|| "(无)".to_string());

    let window_title = extra
        .surrounding
        .as_ref()
        .and_then(|s| s.window_title.as_ref())
        .map(|t| t.as_str())
        .unwrap_or("(无)");

    format!(
        "═══════════════════════════════════════════════════\n\
         [{timestamp}]\n\
         【应用】{name} ({kind})\n\
         【BundleID】{bundle}\n\
         【窗口标题】{title}\n\
         【选中文本】({len} 字)\n{selected}\n\n\
         【上文 before】\n{before}\n\n\
         【下文 after】\n{after}\n\n\
         【AX 诊断】\n{diag}\n\n",
        timestamp = timestamp,
        name = extra.source.name,
        kind = kind_label,
        bundle = extra.source.bundle_id.as_deref().unwrap_or("(未知)"),
        title = window_title,
        len = selected_text.chars().count(),
        selected = truncate_for_log(selected_text, 1000),
        before = before_preview,
        after = after_preview,
        diag = extra.diagnostics.as_deref().unwrap_or("(无)"),
    )
}

/// 将日志条目写入文件：先创建父目录（非文件路径本身），再追加。
fn write_context_log(path: &std::path::Path, entry: &str) -> std::io::Result<()> {
    // 创建父目录，不是文件路径本身——曾经误用 create_dir_all(&path) 把日志文件创建成目录
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(entry.as_bytes())?;
    Ok(())
}

/// 截断文本到指定字符数并添加省略标记。
fn truncate_for_log(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let head: String = chars[..max_chars].iter().collect();
        format!("{}… ({} 字，已截断)", head, chars.len())
    }
}

/// 将 ActionBarContext 的 source/surrounding 拼成 LLM 可理解的情境块，
/// 追加到原始选中文本前面。供 AI 动作（润色/摘要/解释/翻译）使用。
fn build_enriched_text(text: &str) -> String {
    let ctx = PENDING_CONTEXT.lock().unwrap();
    let Some(ref ctx) = *ctx else {
        return text.to_string();
    };

    let mut parts: Vec<String> = Vec::new();

    // 来源
    if let Some(ref source) = ctx.source {
        let kind_label = match source.kind {
            crate::app_context::AppKind::Editor => "编辑器",
            crate::app_context::AppKind::Terminal => "终端",
            crate::app_context::AppKind::Browser => "浏览器",
            crate::app_context::AppKind::Chat => "聊天",
            crate::app_context::AppKind::Unknown => "应用",
        };
        parts.push(format!("【来源】{}（{}）", source.name, kind_label));
    }

    // 前后文
    if let Some(ref surr) = ctx.surrounding {
        if let Some(ref title) = surr.window_title {
            parts.push(format!("【窗口】{}", title));
        }
        if let Some(ref before) = surr.before {
            parts.push(format!("【上文】\n{}", before));
        }
        if let Some(ref after) = surr.after {
            parts.push(format!("【下文】\n{}", after));
        }
    }

    if parts.is_empty() {
        return text.to_string();
    }

    format!("{}\n\n【选中文本】\n{}", parts.join("\n\n"), text)
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
    agent: String,
    accepts: String,
    trigger_keyword: Option<String>,
) -> Result<i64, String> {
    // 同级菜单项最多 35 个（9 数字 + 26 字母快捷键上限）
    let all = octopus_infra::db::list_all_action_bar_items().map_err(|e| e.to_string())?;
    let sibling_count = all.iter().filter(|i| i.parent_id == parent_id).count();
    if sibling_count >= 35 {
        return Err("同级菜单项已达上限 35 个（快捷键 1-9 + a-z）".into());
    }
    octopus_infra::db::insert_action_bar_item(parent_id, &title, &icon, &action_type, &action_data, is_async, write_output_to_clipboard, &shortcut, &agent, &accepts, trigger_keyword.as_deref().unwrap_or(""))
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
    agent: String,
    accepts: String,
    trigger_keyword: Option<String>,
) -> Result<(), String> {
    octopus_infra::db::update_action_bar_item(id, &title, &icon, &action_type, &action_data, is_enabled, is_async, write_output_to_clipboard, &shortcut, &agent, &accepts, trigger_keyword.as_deref().unwrap_or(""))
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

/// 设置菜单项的 auto_paste（Run And Paste 模式）。
#[tauri::command]
pub fn set_auto_paste(id: i64, auto_paste: bool) -> Result<(), String> {
    octopus_infra::db::set_auto_paste(id, auto_paste).map_err(|e| e.to_string())
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
            // 自动：优先 opus-mt（轻量），其次 m2m100，否则 LLM
            let models = octopus_translation::discover_translation_models();
            if models.iter().any(|m| m.name == "opus-mt" && m.downloaded) {
                TranslateStrategy::Local("local:opus-mt".into())
            } else if models.iter().any(|m| m.name == "m2m100-418M" && m.downloaded) {
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

/// 执行翻译（公共逻辑）：解析引擎策略 + 执行翻译。
/// 供 do_translate_streaming 和 finalize 翻译路径复用。
pub(crate) fn do_translate(text: &str, config: &octopus_infra::config::AppConfig) -> Result<String, String> {
    let (source_lang, target_lang) = detect_translate_direction(text);
    match resolve_translate_strategy(config) {
        TranslateStrategy::Local(spec) => {
            // opus-mt 需要方向信息来加载对应子目录
            if spec.starts_with("local:opus-mt") {
                let engine = octopus_translation::load_opus_mt(source_lang, target_lang)
                    .map_err(|e| e.to_string())?;
                return engine.translate(text, source_lang, target_lang)
                    .map_err(|e| e.to_string());
            }
            // m2m100 等其他引擎
            let manager = octopus_translation::TranslationManager::new(&spec);
            let engine = manager.engine()
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "翻译引擎加载失败".to_string())?;
            engine.translate(text, source_lang, target_lang)
                .map_err(|e| e.to_string())
        }
        TranslateStrategy::Llm => {
            let llm_config = crate::config::llm_config_ignore_mode(config)
                .ok_or_else(|| "翻译引擎未配置，请在设置中配置本地翻译模型或 LLM".to_string())?;
            let prompt = auto_translate_prompt(text);
            octopus_llm::chat_text_with_prompt(prompt, text, &llm_config)
                .map_err(|e| e.to_string())
        }
    }
}

/// 流式翻译：按段落（换行）切分，逐段翻译，每段完成 emit 累积结果。
/// 前端 listen "translate-progress"（增量更新）+ "translate-done"（翻译完成）。
fn do_translate_streaming(text: &str, app: &AppHandle) {
    let config = match octopus_infra::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            let _ = app.emit("translate-done", format!("❌ 配置加载失败: {}", e));
            return;
        }
    };

    // 按换行切分段落，逐段翻译
    let segments: Vec<&str> = text.split('\n').collect();
    let total = segments.len();
    let mut accumulated = String::new();

    for (i, seg) in segments.iter().enumerate() {
        if seg.trim().is_empty() {
            if i < total - 1 { accumulated.push('\n'); }
            continue;
        }
        match do_translate(seg, &config) {
            Ok(t) => {
                accumulated.push_str(&t);
            }
            Err(e) => {
                accumulated = format!("❌ 翻译失败: {}", e);
                break;
            }
        }
        if i < total - 1 { accumulated.push('\n'); }

        // 每段完成 emit 增量结果（前端实时更新译文区）
        let _ = app.emit("translate-progress", &accumulated);
    }

    let _ = app.emit("translate-done", &accumulated);
}

/// 前端工具栏翻译按钮调用。fire-and-forget：立即返回，翻译结果通过事件 emit。
/// 前端 invoke 后立即切 contrast 模式（译文区显示 loading），listen translate-progress/done 更新。
#[tauri::command]
pub fn translate_text(text: String, app: AppHandle) -> Result<(), String> {
    let app_clone = app.clone();
    std::thread::spawn(move || {
        do_translate_streaming(&text, &app_clone);
    });
    Ok(())
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

/// file:// URL 路径编码：仅编码空格（macOS file:// URL 的最小需求）
fn url_encode_path(path: &str) -> String {
    path.replace(' ', "%20")
}

/// 渲染 agent prompt 模板：替换 {{task}} 和 {{files}} 占位符。
/// task: 用户输入的任务描述（无 {{task}} 占位符时忽略）
/// files: 文件路径列表（换行分隔注入 {{files}}）
pub fn render_agent_prompt(template: &str, task: &str, files: &[String]) -> String {
    template
        .replace("{{task}}", task)
        .replace("{{files}}", &files.join("\n"))
}

/// 按格式格式化文件路径列表（copy_path 动作用）。
/// format: "plain"（纯路径）/ "url"（file:// URL）/ "quoted"（带引号），其他值同 plain。
pub fn format_paths(files: &[String], format: &str) -> String {
    match format {
        "url" => files.iter().map(|f| format!("file://{}", url_encode_path(f))).collect::<Vec<_>>().join("\n"),
        "quoted" => files.iter().map(|f| format!("\"{}\"", f)).collect::<Vec<_>>().join("\n"),
        _ => files.join("\n"),
    }
}

/// 从文件列表推导工作目录：首个文件的父目录，无文件时 fallback HOME 或 /tmp。
pub fn derive_cwd(files: &[String]) -> String {
    files.first()
        .and_then(|f| std::path::Path::new(f).parent().map(|p| p.to_string_lossy().to_string()))
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
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

    // 从 PENDING_CONTEXT 取 files（Files 场景）
    let app_state_files: Vec<String> = PENDING_CONTEXT.lock().unwrap()
        .as_ref().map(|c| c.files.clone()).unwrap_or_default();

    match item.action_type.as_str() {
        "ai" => {
            let config = octopus_infra::config::load_config().map_err(|e| e.to_string())?;

            // 翻译特殊处理：优先本地引擎
            if item.action_data == "auto_translate" {
                match resolve_translate_strategy(&config) {
                    TranslateStrategy::Local(_) => {
                        // 流式翻译：立即隐藏浮窗 + 打开 contrast tab（译文区 loading），
                        // 后台逐段翻译，通过 emit 事件实时更新译文区。
                        // 用户无需等待，翻译结果在编辑器中逐段出现。
                        if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
                            let _ = win.hide();
                        }
                        #[cfg(target_os = "macos")]
                        { crate::activation::after_floating_window_hide_keep_active(&app); }
                        finalize_action_bar(&app);

                        let original_text = text.clone();
                        let payload = crate::compact_editor_commands::TempTabPayload {
                            text: "【翻译】\n⏳ 正在翻译…".into(),
                            mode: Some("contrast".into()),
                            original_text: Some(original_text.clone()),
                            translated_text: Some("⏳ 正在翻译…".into()),
                            ..Default::default()
                        };
                        // 投递主线程——create_compact_editor_window 内含 set_dock_icon
                        // 需主线程的 MainThreadMarker，worker 线程直接调会被跳过
                        let app_for_editor = app.clone();
                        let _ = app.run_on_main_thread(move || {
                            crate::compact_editor_commands::open_temp_compact_editor(&app_for_editor, &payload);
                        });

                        let app_clone = app.clone();
                        std::thread::spawn(move || {
                            do_translate_streaming(&original_text, &app_clone);
                        });
                        return Ok(true);
                    }
                    TranslateStrategy::Llm => {
                        let llm_config = crate::config::llm_config_ignore_mode(&config)
                            .ok_or("润色模型未配置，请在设置中配置 LLM")?;
                        let enriched_text = build_enriched_text(&text);
                        let prompt = auto_translate_prompt(&enriched_text);
                        let result = octopus_llm::chat_text_with_prompt(prompt, &enriched_text, &llm_config)
                        .map_err(|e| e.to_string())?;
                        if item.auto_paste {
                            action_bar_run_and_paste(result, app.clone());
                        } else {
                            action_bar_show_result(result, text, "translate".into(), app.clone(), true);
                        }
                        return Ok(true);
                    }
                }
            }

            // 非 auto_translate 的 AI 操作（润色/摘要/解释），仍走 LLM
            let llm_config = crate::config::llm_config_ignore_mode(&config)
                .ok_or("润色模型未配置，请在设置中配置 LLM")?;
            let enriched_text = build_enriched_text(&text);
            let result = octopus_llm::chat_text_with_prompt(&item.action_data, &enriched_text, &llm_config)
                .map_err(|e| e.to_string())?;
            if item.auto_paste {
                action_bar_run_and_paste(result, app.clone());
            } else {
                action_bar_show_result(result, String::new(), item.title, app.clone(), true);
            }
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
                    if item.auto_paste {
                        action_bar_run_and_paste(result.stdout, app.clone());
                    } else if write_output {
                        write_clipboard_text(app, &result.stdout);
                        action_bar_show_result(result.stdout, text, item_title, app.clone(), false);
                    } else {
                        action_bar_show_result(result.stdout, text, item_title, app.clone(), false);
                    }
                    return Ok(true);
                }
                // 成功无输出 → 正常关闭
                Ok(false)
            }
        }
        "agent" => {
            // agent 桥接：渲染命令 → Terminal.app 启动
            let adapter_key = item.agent.clone();
            let adapters = crate::agent_adapter::list_adapters();
            let adapter = adapters.into_iter().find(|a| a.key == adapter_key)
                .ok_or_else(|| format!("Agent adapter '{}' 不存在", adapter_key))?;
            if !adapter.is_available {
                return Err(format!("{} 未安装（未在 PATH 找到 `{}`）", adapter.display_name, adapter.detect_binary));
            }
            let prompt = render_agent_prompt(&item.action_data, &text, &app_state_files);
            let cwd = derive_cwd(&app_state_files);
            let cwd_path = std::path::Path::new(&cwd);
            let command = crate::agent_adapter::render_command(
                &adapter.command_template, &prompt, &app_state_files, &cwd,
            );
            let launcher = crate::terminal_launcher::TerminalAppLauncher;
            use crate::terminal_launcher::TerminalLauncher;
            launcher.spawn(&command, cwd_path)?;
            Ok(false)
        }
        "copy_path" => {
            let formatted = format_paths(&app_state_files, &item.action_data);
            write_clipboard_text(app, &formatted);
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

// ── Agent Adapter 管理命令（设置页用）──

#[tauri::command]
pub fn list_agent_adapters() -> Result<Vec<crate::agent_adapter::AgentAdapter>, String> {
    Ok(crate::agent_adapter::list_adapters())
}

#[tauri::command]
pub fn create_agent_adapter(
    key: String, display_name: String, detect_binary: String, command_template: String,
) -> Result<i64, String> {
    // 拒绝与内置 adapter 同名——避免 find() 永远命中内置项
    if crate::agent_adapter::is_builtin_key(&key) {
        return Err(format!("key '{}' 与内置 adapter 冲突", key));
    }
    octopus_infra::db::insert_agent_adapter_record(&key, &display_name, &detect_binary, &command_template)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_agent_adapter(
    id: i64, key: String, display_name: String, detect_binary: String, command_template: String,
) -> Result<(), String> {
    // 与 create 对称：拒绝改名为内置 key
    if crate::agent_adapter::is_builtin_key(&key) {
        return Err(format!("key '{}' 与内置 adapter 冲突", key));
    }
    octopus_infra::db::update_agent_adapter_record(id, &key, &display_name, &detect_binary, &command_template)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_agent_adapter(id: i64) -> Result<(), String> {
    octopus_infra::db::delete_agent_adapter_record(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn refresh_agent_detection() -> Result<Vec<crate::agent_adapter::AgentAdapter>, String> {
    Ok(crate::agent_adapter::refresh_detection())
}

// ── Agent Voice（语音联动）──

/// agent 项含 {{task}} 时：创建 agent_task → 隐藏浮窗 → 触发音录。
#[tauri::command]
pub fn trigger_agent_voice(
    item_id: i64,
    app: AppHandle,
    coordinator: tauri::State<'_, crate::coordinator::Coordinator>,
) -> Result<(), String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(|e| e.to_string())?
        .ok_or("菜单项不存在")?;

    let files: Vec<String> = PENDING_CONTEXT.lock().unwrap()
        .as_ref().map(|c| c.files.clone()).unwrap_or_default();

    let cwd = derive_cwd(&files);
    let context = serde_json::json!({
        "kind": "files",
        "files": files,
        "cwd": cwd,
        "prompt_template": item.action_data,
    }).to_string();

    let task_id = uuid::Uuid::new_v4().to_string();
    octopus_infra::db::insert_agent_task(&task_id, &item.agent, &context)
        .map_err(|e| e.to_string())?;

    // 隐藏 action bar 浮窗
    if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
        let _ = win.hide();
    }
    #[cfg(target_os = "macos")]
    { crate::activation::after_floating_window_hide(&app); }
    finalize_action_bar(&app);

    // 触发 agent 录音
    coordinator.start_agent_recording(task_id);
    Ok(())
}

#[tauri::command]
pub fn list_agent_tasks(limit: Option<i64>) -> Result<Vec<octopus_infra::db::AgentTask>, String> {
    octopus_infra::db::list_agent_tasks(limit.unwrap_or(100)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_agent_task(id: String) -> Result<(), String> {
    octopus_infra::db::delete_agent_task(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn retry_agent_task(id: String, app: AppHandle) -> Result<(), String> {
    let task = octopus_infra::db::load_agent_task(&id)
        .map_err(|e| e.to_string())?
        .ok_or("task 不存在")?;
    if task.status != "failed" && task.status != "done" {
        return Err("仅 failed/done 状态可重试".into());
    }
    if task.transcribed_text.trim().is_empty() {
        return Err("识别结果为空，无法重试".into());
    }
    crate::coordinator::retry_agent_task(&app, &id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── render_agent_prompt ──

    #[test]
    fn test_render_agent_prompt_with_task_and_files() {
        let prompt = render_agent_prompt(
            "{{task}}\n\n文件列表：\n{{files}}",
            "制作PPT",
            &["/a.pdf".into(), "/b.pdf".into()],
        );
        assert_eq!(prompt, "制作PPT\n\n文件列表：\n/a.pdf\n/b.pdf");
    }

    #[test]
    fn test_render_agent_prompt_no_task_placeholder() {
        // 无 {{task}}——task 参数被忽略
        let prompt = render_agent_prompt("整理这些文件：{{files}}", "ignored task", &["/a".into()]);
        assert_eq!(prompt, "整理这些文件：/a");
    }

    #[test]
    fn test_render_agent_prompt_no_files_placeholder() {
        let prompt = render_agent_prompt("执行：{{task}}", "do something", &["/a".into()]);
        assert_eq!(prompt, "执行：do something");
    }

    #[test]
    fn test_render_agent_prompt_no_placeholders() {
        let prompt = render_agent_prompt("固定命令", "ignored", &[]);
        assert_eq!(prompt, "固定命令");
    }

    #[test]
    fn test_render_agent_prompt_empty_task() {
        let prompt = render_agent_prompt("{{task}}", "", &[]);
        assert_eq!(prompt, "");
    }

    #[test]
    fn test_render_agent_prompt_empty_files() {
        let prompt = render_agent_prompt("文件：{{files}}", "task", &[]);
        assert_eq!(prompt, "文件：");
    }

    #[test]
    fn test_render_agent_prompt_multiple_files() {
        let prompt = render_agent_prompt("{{files}}", "", &["/a".into(), "/b".into(), "/c".into()]);
        assert_eq!(prompt, "/a\n/b\n/c");
    }

    // ── format_paths ──

    #[test]
    fn test_format_paths_plain() {
        let result = format_paths(&["/a.pdf".into(), "/b.pdf".into()], "plain");
        assert_eq!(result, "/a.pdf\n/b.pdf");
    }

    #[test]
    fn test_format_paths_url() {
        let result = format_paths(&["/a/b.pdf".into()], "url");
        assert_eq!(result, "file:///a/b.pdf");
    }

    #[test]
    fn test_format_paths_url_with_spaces() {
        let result = format_paths(&["/a/b c.pdf".into()], "url");
        assert_eq!(result, "file:///a/b%20c.pdf");
    }

    #[test]
    fn test_format_paths_quoted() {
        let result = format_paths(&["/a.pdf".into(), "/b.pdf".into()], "quoted");
        assert_eq!(result, "\"/a.pdf\"\n\"/b.pdf\"");
    }

    #[test]
    fn test_format_paths_unknown_format_defaults_plain() {
        let result = format_paths(&["/a".into()], "unknown");
        assert_eq!(result, "/a");
    }

    #[test]
    fn test_format_paths_empty_list() {
        assert_eq!(format_paths(&[], "plain"), "");
        assert_eq!(format_paths(&[], "url"), "");
        assert_eq!(format_paths(&[], "quoted"), "");
    }

    // ── url_encode_path ──

    #[test]
    fn test_url_encode_path_no_spaces() {
        assert_eq!(url_encode_path("/a/b.pdf"), "/a/b.pdf");
    }

    #[test]
    fn test_url_encode_path_multiple_spaces() {
        assert_eq!(url_encode_path("/a/b c d.pdf"), "/a/b%20c%20d.pdf");
    }

    // ── derive_cwd ──

    #[test]
    fn test_derive_cwd_from_file() {
        let cwd = derive_cwd(&["/Users/x/projects/file.pdf".into()]);
        assert_eq!(cwd, "/Users/x/projects");
    }

    #[test]
    fn test_derive_cwd_root_file() {
        let cwd = derive_cwd(&["/file.txt".into()]);
        assert_eq!(cwd, "/");
    }

    #[test]
    fn test_derive_cwd_empty_files_fallback() {
        // 空列表——fallback 到 HOME 或 /tmp（不验证具体值，只验证不 panic）
        let cwd = derive_cwd(&[]);
        assert!(!cwd.is_empty());
    }

    // ── app_context 相关（main 引入）──

    use crate::app_context::{AppKind, AppSource, ExtraContext, SurroundingText};

    fn sample_extra() -> ExtraContext {
        ExtraContext {
            source: AppSource {
                bundle_id: Some("com.apple.TextEdit".to_string()),
                name: "TextEdit".to_string(),
                kind: AppKind::Editor,
            },
            surrounding: Some(SurroundingText {
                before: Some("上文内容".to_string()),
                after: Some("下文内容".to_string()),
                window_title: Some("report.txt".to_string()),
            }),
            diagnostics: None,
        }
    }

    #[test]
    fn test_truncate_for_log_short() {
        assert_eq!(truncate_for_log("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_for_log_exact() {
        assert_eq!(truncate_for_log("abcde", 5), "abcde");
    }

    #[test]
    fn test_truncate_for_log_truncated() {
        let result = truncate_for_log("abcdefghij", 3);
        assert!(result.starts_with("abc…"));
        assert!(result.contains("10 字"));
    }

    #[test]
    fn test_truncate_for_log_cjk() {
        let result = truncate_for_log("你好世界你好世界", 4);
        assert!(result.starts_with("你好世界…"));
        assert!(result.contains("8 字"));
    }

    #[test]
    fn test_format_entry_contains_source() {
        let entry = format_context_entry("选中的文字", &sample_extra());
        assert!(entry.contains("TextEdit"));
        assert!(entry.contains("Editor"));
        assert!(entry.contains("com.apple.TextEdit"));
    }

    #[test]
    fn test_format_entry_contains_surrounding() {
        let entry = format_context_entry("选中", &sample_extra());
        assert!(entry.contains("上文内容"));
        assert!(entry.contains("下文内容"));
        assert!(entry.contains("report.txt"));
    }

    #[test]
    fn test_format_entry_no_surrounding() {
        let extra = ExtraContext {
            source: AppSource {
                bundle_id: None,
                name: "UnknownApp".to_string(),
                kind: AppKind::Unknown,
            },
            surrounding: None,
            diagnostics: None,
        };
        let entry = format_context_entry("hello", &extra);
        assert!(entry.contains("UnknownApp"));
        assert!(entry.contains("(无)"));
        assert!(entry.contains("(未知)"));
    }

    #[test]
    fn test_format_entry_selected_char_count() {
        let entry = format_context_entry("你好world", &sample_extra());
        assert!(entry.contains("(7 字)"));
    }

    #[test]
    fn test_write_creates_file_not_directory() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("logs").join("action-bar.log");
        write_context_log(&log_path, "test entry\n").unwrap();
        assert!(log_path.is_file(), "日志路径应该是文件，不是目录");
        assert!(!log_path.is_dir());
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert_eq!(content, "test entry\n");
    }

    #[test]
    fn test_write_appends_multiple_entries() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("deep").join("nested").join("action-bar.log");
        write_context_log(&log_path, "first\n").unwrap();
        write_context_log(&log_path, "second\n").unwrap();
        let content = std::fs::read_to_string(&log_path).unwrap();
        assert_eq!(content, "first\nsecond\n");
    }

    #[test]
    fn test_write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("a").join("b").join("c").join("action-bar.log");
        write_context_log(&log_path, "deep path\n").unwrap();
        assert!(log_path.is_file());
        assert_eq!(std::fs::read_to_string(&log_path).unwrap(), "deep path\n");
    }

    #[test]
    fn test_write_existing_directory_at_path_fails() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("action-bar.log");
        std::fs::create_dir_all(&log_path).unwrap();
        let result = write_context_log(&log_path, "test\n");
        assert!(result.is_err(), "路径已是目录时写入应失败");
    }
}
