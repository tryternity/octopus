//! 选中检测 + 应用上下文 + 结果展示（从 action_bar_commands/mod.rs 提取，Task 1.7）。
//!
//! 模块内容（约 770 行，最大）：
//! - `ContextKind` / `ActionBarContext` 类型；
//! - `Selection` enum + `detect_selection`（Finder/Sublime/Cmd+C 三分支，changeCount 隔离）
//!   及其 `CHANGE_COUNT_BASELINE` 共享静态量；
//! - 结果展示 `action_bar_show_result`（CompactEditor 临时 tab）；
//! - 剪贴板读写 + macOS NSPasteboard.changeCount；
//! - 上下文日志（format/truncate/write）+ `build_enriched_text`（拼 LLM 上下文块）；
//! - `get_mouse_position`（mac CGEvent / 非 mac None）。

use tauri::{AppHandle, Manager};
use crate::platform::focus_tracker::FocusTracker;
// 父模块共享状态 + 经 glob re-export 的兄弟 helper
use super::{
    PENDING_CONTEXT,
    finalize_action_bar, primary_monitor_center,
};

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
    pub source: Option<crate::platform::app_context::AppSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surrounding: Option<crate::platform::app_context::SurroundingText>,
}

impl ActionBarContext {
    pub fn text(text: String) -> Self {
        Self { kind: ContextKind::Text, text: Some(text), files: vec![], source: None, surrounding: None }
    }
    pub fn files(files: Vec<String>) -> Self {
        Self { kind: ContextKind::Files, text: None, files, source: None, surrounding: None }
    }
}

/// detect_selection 的 changeCount 基准——上次 detect 结束时（含恢复剪贴板的写入）
/// 记录的 changeCount。下次 detect 的 before 用 max(实时读, 此基准)，
/// 隔离恢复剪贴板写入自身递增 changeCount 对下次检测的污染（现象 2 根因）。
static CHANGE_COUNT_BASELINE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// 微信/WKWebView 嵌套 app 的 Cmd+C 等待 changeCount 递增的超时。
///
/// osascript 的 `keystroke "c"` 投出后，WKWebView 嵌套层处理菜单快捷键 + 写回
/// pasteboard 是**异步**的——实测 80ms 超时时 changeCount 还没变（误判无选中），
/// 300ms 能稳定覆盖大多数情况（实测成功率 ~90%）。如需调优改这个常量即可。
const WECHAT_COPY_POLL_TIMEOUT_MS: u64 = 300;

/// 保存当前 CHANGE_COUNT_BASELINE（供 silent 路径隔离 detect 副作用）。
pub(crate) fn save_change_count_baseline() -> i64 {
    CHANGE_COUNT_BASELINE.load(std::sync::atomic::Ordering::SeqCst)
}

/// 恢复 CHANGE_COUNT_BASELINE（silent 路径 detect 后恢复原值，不污染 ActionBar 路径）。
pub(crate) fn restore_change_count_baseline(val: i64) {
    CHANGE_COUNT_BASELINE.store(val, std::sync::atomic::Ordering::SeqCst);
}

/// 热键触发：模拟 Cmd+C → 读剪贴板 → 获取鼠标位置 → 显示浮窗。
/// 一次触发检测出的完整选中状态——后端唯一的"有什么选中"真相源。
/// 检测完成后，下游所有操作仅读这个枚举，不再碰 changeCount / 剪贴板 / 鼠标坐标。
///
/// 鼠标坐标始终在检测阶段采集（无论有无选中），用于：
///   - 有选中 → 鼠标位置弹出
///   - 无选中 → 忽略，用主屏居中
pub(crate) enum Selection {
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
    pub(crate) fn mouse(&self) -> (f64, f64) {
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
pub(crate) fn detect_selection(app: &AppHandle) -> Selection {
    // 鼠标坐标在检测开始时就采集（后续 Cmd+C 等 sleep 不影响坐标）。
    // P2-3 修复（2026-07-17）：get_mouse_position 失败（CGEvent 权限缺失）时
    // **不放弃选中检测**——用主屏中心作占位 mouse 继续 Finder/Sublime/Cmd+C 三条分支。
    // 原先直接 return Selection::None 会把"位置失败"耦合为"放弃选中"，选中文本按热键
    // 弹空搜索框（AI 操作全失效，比原 bug 仅位置偏移影响更大）。
    // 占位坐标让浮窗弹到主屏中心而非鼠标旁——位置让位给可用性，但选中内容保留。
    let mouse = get_mouse_position(app).unwrap_or_else(|| {
        log::warn!("[action-bar] 鼠标位置采集失败（CGEvent 权限？），用主屏中心占位");
        primary_monitor_center(app)
    });

    // ── Finder 分支：AppleScript 直接拿 selection ──
    if crate::platform::finder_selection::is_finder_frontmost() {
        return match crate::platform::finder_selection::get_finder_selection() {
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

    // ── Sublime 分支：插件精确读选区（绕过 Cmd+C 的 copy_with_empty_selection 陷阱）──
    // Sublime 4 默认 `copy_with_empty_selection: true`——无选中时 Cmd+C 复制当前行，
    // 导致 changeCount +1 且剪贴板有当前行内容，changeCount 方案误判为"有选中"。
    // 插件的 sel_start/sel_end 能精确区分有无选中，不依赖 Cmd+C。
    #[cfg(target_os = "macos")]
    if crate::platform::app_context::sublime_plugin::is_sublime_frontmost() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        return match crate::platform::app_context::sublime_plugin::get_sublime_selection(deadline) {
            Some(text) => {
                log::info!("[action-bar] Sublime 插件选区: len={}, mouse=({},{})", text.len(), mouse.0, mouse.1);
                Selection::Text { text, mouse }
            }
            None => {
                log::info!("[action-bar] Sublime 无选中（插件 sel_start==sel_end）");
                Selection::None
            }
        };
    }

    // ── 非 Finder/非 Sublime 分支：Cmd+C + changeCount 判断有无选中文本 ──
    let clip_handle = app.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>().clone();
    let clipboard_before_text = read_clipboard_text(app);
    let clipboard_before_image = if clip_handle.has_image() {
        clip_handle.read_image().ok()
    } else { None };
    let clipboard_before_files = if clip_handle.has_files() {
        clip_handle.read_files().ok().filter(|f| !f.is_empty())
    } else { None };

    clip_handle.suppress_next();
    // before 取 max(实时读, 上次记录的 baseline)——隔离上次 detect 恢复剪贴板的写入
    // 递增 changeCount 对本次检测的污染（现象 2：无选中误判 Some）。
    let now_count = pasteboard_change_count();
    let baseline = CHANGE_COUNT_BASELINE.load(std::sync::atomic::Ordering::SeqCst);
    let change_count_before = now_count.max(baseline);
    log::info!(
        "[action-bar][detect] before: now={} baseline={} → use={}, clip_text_len={}",
        now_count, baseline, change_count_before,
        clipboard_before_text.as_deref().map(|t| t.len()).unwrap_or(0)
    );

    let focus = FocusTracker::new();
    let copy_dispatch = focus.simulate_copy();
    log::info!("[action-bar][detect] simulate_copy dispatch={:?}", copy_dispatch);
    // 等 Cmd+C 产生剪贴板写入。polling 每 5ms 检查 changeCount，命中即退出。
    //
    // **超时按 dispatch 路径动态化**（2026-07-21 实测调优）：
    // - CGEvent 路径（原生/Electron）：Cmd+C 同步触发，changeCount 通常 < 50ms 递增，80ms 兜底
    // - Osascript 路径（WKWebView 嵌套如微信）：osascript 返回时 keystroke 已投出，但
    //   WKWebView 处理菜单快捷键 + 写回 pasteboard 是异步的，需要 200-400ms。
    //   80ms 超时时 WKWebView 还没完成写入，changeCount 未变 → 误判无选中 → launch 模式。
    //
    // 微信超时由 WECHAT_COPY_POLL_TIMEOUT_MS 控制，方便后续调整。
    let poll_timeout_ms = match copy_dispatch {
        crate::platform::focus_tracker::CopyDispatch::Osascript => WECHAT_COPY_POLL_TIMEOUT_MS,
        crate::platform::focus_tracker::CopyDispatch::CGEvent => 80,
    };
    let poll_deadline = std::time::Instant::now() + std::time::Duration::from_millis(poll_timeout_ms);
    let mut change_count_after = change_count_before;
    while std::time::Instant::now() < poll_deadline {
        change_count_after = pasteboard_change_count();
        if change_count_after > change_count_before {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let change_count_after = change_count_after;
    log::info!("[action-bar][detect] after: changeCount={}", change_count_after);
    // 无选中退出：更新 baseline 到当前 changeCount（含本次 Cmd+C 可能的无效写入）
    if change_count_after <= change_count_before {
        log::info!("[action-bar] changeCount unchanged {}→{} = no selection",
            change_count_before, change_count_after);
        clip_handle.clear_suppress();
        CHANGE_COUNT_BASELINE.store(change_count_after, std::sync::atomic::Ordering::SeqCst);
        return Selection::None;
    }

    // changeCount 递增 → 有选中，读剪贴板拿文本
    let clipboard_after = read_clipboard_text(app);
    log::info!(
        "[action-bar][detect] changed: before_use={} after={}, clip_after_len={}",
        change_count_before, change_count_after,
        clipboard_after.as_deref().map(|t| t.len()).unwrap_or(0)
    );

    // 恢复原始剪贴板——只要 Cmd+C 改了剪贴板就恢复（含选中图片/文件致文本为空的场景），
    // 防止用户原剪贴板内容被 Cmd+C 产生的非文本内容永久覆盖。
    // 注意：恢复写入（write_files/set_image/write_text）自身会递增 changeCount，
    // 恢复后更新 baseline 到新的 changeCount，避免下次检测把恢复写入误认为 Cmd+C。
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
    // 更新 baseline 到恢复后的 changeCount——下次 detect 的 before 至少是这个值。
    let cc_after_restore = pasteboard_change_count();
    CHANGE_COUNT_BASELINE.store(cc_after_restore, std::sync::atomic::Ordering::SeqCst);

    let text = match &clipboard_after {
        Some(t) if !t.trim().is_empty() => t.clone(),
        _ => {
            log::info!("[action-bar] changeCount changed but clipboard empty");
            return Selection::None;
        }
    };

    log::info!("[action-bar] got text len={}, mouse=({},{})", text.len(), mouse.0, mouse.1);
    Selection::Text { text, mouse }
}

/// AI 结果通过临时 tab 打开 CompactEditor 展示（不写 DB）。
/// 结果写入剪贴板留给用户——不恢复原始剪贴板（与 dismiss/open_url 不同）。
#[tauri::command]
pub fn action_bar_show_result(result: String, original_text: String, action: String, app: AppHandle, write_clipboard: bool) {
    action_bar_show_result_internal(result, original_text, action, app, write_clipboard);
}

fn action_bar_show_result_internal(
    result: String,
    _original_text: String,
    _action: String,
    app: AppHandle,
    _write_clipboard: bool,
) {
    // 只在 ActionBar 实际可见时才 hide + depth 操作。
    // Quick Execute（全局快捷键）路径下 ActionBar 从未 show（depth 未 +1），
    // 此时 hide + after_floating_window_hide_keep_active（depth -1）会破坏配对。
    // 用 is_visible 检查替代 is_silent 参数——自动适配 ActionBar 可见/不可见两条路径。
    let action_bar_visible = app.get_webview_window(crate::action_bar::action_bar_window::WINDOW_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if action_bar_visible {
        if let Some(win) = app.get_webview_window(crate::action_bar::action_bar_window::WINDOW_LABEL) {
            let _ = win.hide();
        }
        #[cfg(target_os = "macos")]
        { crate::platform::activation::after_floating_window_hide_keep_active(&app); }
        finalize_action_bar(&app);
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
        crate::commands::compact_editor_commands::TempTabPayload {
            text: display_text.clone(),
            mode: Some("contrast".into()),
            original_text: Some(_original_text),
            translated_text: Some(result.clone()),
            ..Default::default() // translate_session_id=None（LLM 路径不走流式）；item_id/source/is_temp 由 open_temp_compact_editor 补齐
        }
    } else {
        crate::commands::compact_editor_commands::TempTabPayload {
            text: display_text,
            ..Default::default()
        }
    };
    // 投递主线程——create_compact_editor_window 内含 set_dock_icon 需主线程
    let app_for_editor = app.clone();
    let _ = app.run_on_main_thread(move || {
        crate::commands::compact_editor_commands::open_temp_compact_editor(&app_for_editor, &payload);
    });
}

// ── 辅助函数 ──

pub(crate) fn read_clipboard_text(app: &AppHandle) -> Option<String> {
    let handle = app.state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>();
    handle.read_text().ok()
}

pub(crate) fn write_clipboard_text(app: &AppHandle, text: &str) {
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
pub(crate) fn log_app_context(selected_text: &str, extra: &crate::platform::app_context::ExtraContext) {
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
fn format_context_entry(selected_text: &str, extra: &crate::platform::app_context::ExtraContext) -> String {
    let kind_label = match extra.source.kind {
        crate::platform::app_context::AppKind::Editor => "Editor",
        crate::platform::app_context::AppKind::Terminal => "Terminal",
        crate::platform::app_context::AppKind::Browser => "Browser",
        crate::platform::app_context::AppKind::Chat => "Chat",
        crate::platform::app_context::AppKind::Unknown => "Unknown",
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
pub(crate) fn build_enriched_text(text: &str) -> String {
    let ctx = PENDING_CONTEXT.lock();
    let Some(ref ctx) = *ctx else {
        return text.to_string();
    };

    let mut parts: Vec<String> = Vec::new();

    // 来源
    if let Some(ref source) = ctx.source {
        let kind_label = match source.kind {
            crate::platform::app_context::AppKind::Editor => "编辑器",
            crate::platform::app_context::AppKind::Terminal => "终端",
            crate::platform::app_context::AppKind::Browser => "浏览器",
            crate::platform::app_context::AppKind::Chat => "聊天",
            crate::platform::app_context::AppKind::Unknown => "应用",
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
pub(crate) fn get_mouse_position(_app: &AppHandle) -> Option<(f64, f64)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok();
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
        return Some((point.x, point.y));
    }
    // P2-3 修复：CGEvent 失败（输入监控/辅助功能权限缺失）返回 None，
    // 调用方 detect_selection fallback 到 Selection::None 居中——
    // 原先返回 (100,100) 假坐标会让浮窗弹到主屏左上角，与 None 分支居中体验不一致。
    None
}

#[cfg(not(target_os = "macos"))]
fn get_mouse_position(_app: &AppHandle) -> Option<(f64, f64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::app_context::{AppKind, AppSource, ExtraContext, SurroundingText};

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
