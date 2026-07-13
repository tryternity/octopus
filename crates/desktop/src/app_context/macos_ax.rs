//! macOS Accessibility 实现。
//!
//! 取数路径：
//! 1. NSWorkspace.frontmostApplication → pid + bundleId + name
//! 2. classify_app(bundleId) → AppKind
//! 3. AXUIElementCreateApplication(pid) → app element
//! 4. AXFocusedUIElement → focused element
//! 5. AXSelectedTextRange + AXValue → 切前后文
//! 6. Terminal 特例：全文作 scrollback 截断
//! 7. AXTitle → 窗口标题

#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

use core_foundation::base::{CFGetTypeID, CFRelease, CFRetain, CFTypeRef, TCFType};
use core_foundation::string::CFString;

use super::ffi::*;
use super::*;

/// AX 调用整体超时上限。
const AX_TIMEOUT: Duration = Duration::from_millis(500);
/// Editor before/after 截断字数。
const SURROUNDING_LIMIT: usize = 1000;
/// Terminal scrollback 最大行数。
const TERMINAL_MAX_LINES: usize = 30;
/// Terminal scrollback 最大字数。
const TERMINAL_MAX_CHARS: usize = 1000;

pub struct AxProvider;

impl super::ContextProvider for AxProvider {
    fn gather(&self, selected_text: &str) -> anyhow::Result<ExtraContext> {
        // 1. 前台应用信息
        let (pid, bundle_id, name) =
            frontmost_app().ok_or_else(|| anyhow::anyhow!("无法获取前台应用"))?;

        let kind = bundle_id
            .as_deref()
            .map(classify_app)
            .unwrap_or(AppKind::Unknown);

        let source = AppSource {
            bundle_id: bundle_id.clone(),
            name,
            kind,
        };

        // 2. 采集 surrounding（失败 → None，不阻断 source 返回）
        let deadline = Instant::now() + AX_TIMEOUT;
        let (surrounding, diagnostics) = if Instant::now() < deadline {
            // Browser 优先用 AppleScript execute javascript（直接读 DOM，比 AX 准确）
            if kind == AppKind::Browser {
                if let Some(result) = gather_browser_via_applescript(
                    bundle_id.as_deref().unwrap_or(""),
                    selected_text,
                    deadline,
                ) {
                    (Some(result.0), result.1)
                } else {
                    // AppleScript JS 失败（可能未开启 Allow JavaScript from Apple Events），fallback 到 AX
                    let mut diag_prefix = "browser AppleScript JS 失败，fallback 到 AX\n".to_string();
                    match gather_surrounding(pid, kind, selected_text, deadline) {
                        Ok((s, diag)) => {
                            let full_diag = match diag {
                                Some(d) => format!("{}{}", diag_prefix, d),
                                None => diag_prefix,
                            };
                            (Some(s), Some(full_diag))
                        }
                        Err(e) => {
                            diag_prefix.push_str(&format!("gather_surrounding error: {}", e));
                            (None, Some(diag_prefix))
                        }
                    }
                }
            } else {
                match gather_surrounding(pid, kind, selected_text, deadline) {
                    Ok((s, diag)) => (Some(s), diag),
                    Err(e) => {
                        log::info!("[app-context] surrounding 采集失败（降级）: {}", e);
                        (None, Some(format!("gather_surrounding error: {}", e)))
                    }
                }
            }
        } else {
            log::warn!("[app-context] gather 超时（source 已获取，surrounding 跳过）");
            (None, Some("gather timeout (surrounding skipped)".to_string()))
        };

        Ok(ExtraContext { source, surrounding, diagnostics })
    }
}

/// 通过 NSWorkspace 获取前台应用 (pid, bundle_id, name)。
fn frontmost_app() -> Option<(i32, Option<String>, String)> {
    use objc2_app_kit::NSWorkspace;

    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let pid = app.processIdentifier();

    if pid < 0 {
        return None;
    }

    let bundle_id = app.bundleIdentifier().map(|s| s.to_string());
    let name = app
        .localizedName()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("pid:{}", pid));

    Some((pid, bundle_id, name))
}

/// 将 AX 错误码翻译为人类可读描述。
fn ax_error_desc(err: AXError) -> &'static str {
    match err {
        0 => "kAXErrorSuccess (成功)",
        -25200 => "kAXErrorFailure (通用失败)",
        -25202 => "kAXErrorIllegalArgument (非法参数)",
        -25203 => "kAXErrorInvalidUIElement (无效 UI 元素)",
        -25204 => "kAXErrorInvalidUIElementObserver (无效观察者)",
        -25205 => "kAXErrorCannotComplete (操作无法完成)",
        -25206 => "kAXErrorAttributeUnsupported (属性不支持)",
        -25207 => "kAXErrorActionUnsupported (动作不支持)",
        -25208 => "kAXErrorNotificationUnsupported (通知不支持)",
        -25209 => "kAXErrorNotImplemented (未实现)",
        -25210 => "kAXErrorNotificationAlreadyRegistered (通知已注册)",
        -25211 => "kAXErrorNotificationNotRegistered (通知未注册)",
        -25212 => "kAXErrorAPIDisabled (AX API 被禁用——检查辅助功能权限)",
        -25213 => "kAXErrorNoValue (无值)",
        -25214 => "kAXErrorParameterizedAttributeUnsupported (参数化属性不支持)",
        -25215 => "kAXErrorNotEnoughPrecision (精度不足)",
        _ => "未知 AX 错误码",
    }
}

/// 通过 AppleScript 在浏览器中执行 JS 获取选区上下文。
///
/// 直接读 DOM（Selection API + Range），比 AX 遍历准确得多。
/// 需要用户在浏览器中开启「Allow JavaScript from Apple Events」：
/// - Chrome/Edge: 菜单栏 → View → Developer → Allow JavaScript from Apple Events
/// - Safari: 偏好设置 → 高级 → 勾选「在菜单栏中显示开发菜单」→ 开发菜单 → 勾选「允许从 Apple 事件执行 JavaScript」
fn gather_browser_via_applescript(
    bundle_id: &str,
    selected_text: &str,
    deadline: Instant,
) -> Option<(SurroundingText, Option<String>)> {
    use std::process::Command;

    // JS 源码写入临时文件——用传入的 selected_text 在 DOM 中搜索定位，
    // 不依赖 window.getSelection()（execute javascript 时选区可能已清空）。
    // 转义 selected_text 中的反斜杠和双引号，防止注入。
    let escaped = selected_text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    let js_source = format!(
        r#"(function(){{
  var sel="{escaped}";
  sel=sel.trim();
  if(!sel) return JSON.stringify({{before:"",after:"",title:document.title}});
  function findIn(text){{
    var i=text.indexOf(sel);
    if(i>=0) return i;
    i=text.toLowerCase().indexOf(sel.toLowerCase());
    if(i>=0) return i;
    var mid=sel.slice(Math.floor(sel.length*0.2),Math.floor(sel.length*0.8)).trim();
    if(mid.length>5){{
      i=text.indexOf(mid);
      if(i>=0) return i;
      i=text.toLowerCase().indexOf(mid.toLowerCase());
      if(i>=0) return i;
    }}
    var head=sel.slice(0,30).trim();
    var tail=sel.slice(-30).trim();
    if(head.length>5&&tail.length>5){{
      var h=text.indexOf(head);
      var t=text.indexOf(tail);
      if(h>=0&&t>=0&&t>=h) return h;
    }}
    if(tail.length>5){{
      i=text.indexOf(tail);
      if(i>=0) return i;
    }}
    return -1;
  }}
  var bodyText=document.body.textContent||"";
  if(findIn(bodyText)<0) return JSON.stringify({{before:"",after:"",title:document.title}});
  var targetChars=2000+sel.length;
  var all=document.querySelectorAll("*");
  var best=document.body;
  var bestLen=bodyText.length;
  for(var i=0;i<all.length&&i<5000;i++){{
    var t=all[i].textContent||"";
    if(t.length<bestLen&&findIn(t)>=0){{
      best=all[i];
      bestLen=t.length;
    }}
  }}
  var scope=best;
  while(scope&&scope.parentNode&&scope!==document.body){{
    if((scope.textContent||"").length>=targetChars) break;
    scope=scope.parentNode;
  }}
  if(!scope) scope=document.body;
  var full=scope.textContent||"";
  var idx=findIn(full);
  if(idx<0) return JSON.stringify({{before:"",after:"",title:document.title}});
  var ml=1000;
  var b=idx>0?full.slice(Math.max(0,idx-ml),idx):"";
  var end=idx+sel.length;
  if(end>full.length) end=full.length;
  var a=end<full.length?full.slice(end,end+ml):"";
  return JSON.stringify({{before:b,after:a,title:document.title}});
}})()
"#,
        escaped = escaped
    );

    let js_path = std::env::temp_dir().join(format!(
        "octopus_browser_ctx_{}_{}.js",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    if let Err(e) = std::fs::write(&js_path, js_source) {
        log::warn!("[app-context] 写 JS 临时文件失败: {}", e);
        return None;
    }

    // RAII guard：无论成功/失败/超时，Drop 时删文件
    struct TempFileGuard(std::path::PathBuf);
    impl Drop for TempFileGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _file_guard = TempFileGuard(js_path.clone());

    // Chrome/Edge 和 Safari 的 AppleScript JS 语法完全不同：
    // - Chrome/Edge: execute (active tab of window 1) javascript jsCode
    // - Safari:      do JavaScript jsCode in document 1
    let script = match bundle_id {
        "com.google.Chrome" | "com.microsoft.edgemac" => {
            let app_name = if bundle_id == "com.google.Chrome" { "Google Chrome" } else { "Microsoft Edge" };
            format!(
                r#"tell application "{}"
    set jsCode to (read POSIX file "{}" as «class utf8»)
    execute (active tab of window 1) javascript jsCode
end tell"#,
                app_name,
                js_path.display()
            )
        }
        "com.apple.Safari" => format!(
            r#"tell application "Safari"
    set jsCode to (read POSIX file "{}" as «class utf8»)
    do JavaScript jsCode in document 1
end tell"#,
            js_path.display()
        ),
        _ => return None,
    };

    // spawn + 超时轮询——防止 osascript 挂起（浏览器未响应/权限弹窗）导致线程泄漏。
    // 必须配 Stdio::piped()，否则 spawn 默认继承父进程 stdout → child.stdout 为 None。
    use std::process::Stdio;
    let mut child = match Command::new("osascript")
        .args(["-e", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[app-context] osascript spawn 失败: {}", e);
            return None;
        }
    };

    // spawn 后立即 take stdout/stderr + 起线程并发读——防 pipe 满导致子进程写阻塞
    // （与 spawn_script 的 wait_with_timeout_secs 范式一致）
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();
    let stdout_thread = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        if let Some(mut s) = stdout_handle {
            let _ = s.read_to_string(&mut buf);
        }
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        if let Some(mut s) = stderr_handle {
            let _ = s.read_to_string(&mut buf);
        }
        buf
    });

    // 轮询等待，超时杀进程
    let mut timed_out = false;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        timed_out = true;
    }
    if timed_out {
        log::warn!("[app-context] browser osascript 超时，已终止");
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        return None;
    }

    // join 读线程拿到输出
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    let status = child.wait().ok();

    if status.as_ref().is_some_and(|s| !s.success()) {
        log::info!("[app-context] browser AppleScript JS 失败: {}", stderr.trim());
        return None;
    }

    let json_str = stdout;
    let json_str = json_str.trim();

    log::info!("[app-context] browser JS result: {} chars", json_str.len());

    let result: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let before = result["before"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from);
    let after = result["after"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from);
    let window_title = result["title"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from);

    if before.is_none() && after.is_none() && window_title.is_none() {
        return None;
    }

    Some((
        SurroundingText {
            before,
            after,
            window_title,
        },
        Some(format!("browser: AppleScript execute javascript ({} chars result)", json_str.len())),
    ))
}

/// 通过 AX 采集选区周围文本。返回 (surrounding, AX 诊断信息)。
fn gather_surrounding(pid: i32, kind: AppKind, selected_text: &str, deadline: Instant) -> anyhow::Result<(SurroundingText, Option<String>)> {
    unsafe {
        // 辅助功能权限诊断
        let trusted = AXIsProcessTrustedWithOptions(std::ptr::null());
        if !trusted {
            log::warn!("[app-context] AXIsProcessTrusted()=false，辅助功能权限可能缺失");
        }

        let app_element = AXUIElementCreateApplication(pid);
        if app_element.is_null() {
            return Err(anyhow::anyhow!("AXUIElementCreateApplication 返回 null"));
        }

        // 获取焦点元素——首次失败（特别是 -25212 kAXErrorAPIDisabled）时重试一次。
        // Chrome 等 App 的 AX 树有初始化延迟，200ms 后重试常能成功。
        let focused_result = get_attribute_value(app_element, &ax_focused_ui_element());
        let focused_result = match focused_result {
            Err(ref e) if e.to_string().contains("-25212") && Instant::now() < deadline => {
                log::info!("[app-context] AXFocusedUIElement 返回 -25212，200ms 后重试");
                std::thread::sleep(std::time::Duration::from_millis(200));
                get_attribute_value(app_element, &ax_focused_ui_element())
            }
            other => other,
        };

        let result = match focused_result {
            Ok(focused_ref) => {
                let focused_element = focused_ref as AXUIElementRef;
                let surrounding = build_surrounding(focused_element, app_element, kind, selected_text, deadline);
                CFRelease(focused_ref);
                surrounding
            }
            Err(e) => {
                log::info!("[app-context] 无法获取焦点元素（含重试）: {}", e);
                match find_text_element_with_selected(app_element, selected_text, deadline) {
                    Some(text_elem) => {
                        let surrounding = build_surrounding(text_elem, app_element, kind, selected_text, deadline);
                        CFRelease(text_elem as CFTypeRef);
                        let mut result = surrounding?;
                        result.1 = Some(format!(
                            "{}\n  fallback: find_text_element 从 app_element 子树找到文本元素",
                            result.1.unwrap_or_default()
                        ));
                        Ok(result)
                    }
                    None => Err(e),
                }
            }
        };

        CFRelease(app_element as CFTypeRef);
        result
    }
}

/// 从焦点元素构建 surrounding。
///
/// 某些编辑器（Sublime Text、自绘 UI）的焦点元素不是文本区本身，
/// 需要遍历 AX 子树找到 AXTextArea / AXTextField 角色的元素。
unsafe fn build_surrounding(
    focused_element: AXUIElementRef,
    app_element: AXUIElementRef,
    kind: AppKind,
    selected_text: &str,
    deadline: Instant,
) -> anyhow::Result<(SurroundingText, Option<String>)> {
    let mut diagnostics: Vec<String> = Vec::new();

    // 焦点元素角色
    let focused_role = get_attribute_string(focused_element, &ax_role()).unwrap_or_default();
    diagnostics.push(format!("focused_role={}", focused_role));

    let (text_element, owns_text_element) = if is_text_element(&focused_role) {
        (focused_element, false)
    } else {
        match find_text_element_with_selected(focused_element, selected_text, deadline) {
            Some(child) => {
                let child_role =
                    get_attribute_string(child, &ax_role()).unwrap_or_default();
                diagnostics.push(format!("found_text_child_role={}", child_role));
                (child, true)
            }
            None => {
                diagnostics.push("no_text_child_found".to_string());
                (focused_element, false)
            }
        }
    };

    // 窗口标题
    let window_title = get_attribute_string(focused_element, &ax_title())
        .or_else(|_| get_attribute_string(app_element, &ax_title()))
        .ok();

    // 全文——过滤终端 scrollback 中的 null bytes（\0）和其他 C0 控制字符
    // iTerm2/Terminal 的 AXValue 常含 \0 间隔（UTF-16 残留）或控制序列残留
    let (full_text, full_text_err) = match get_attribute_string(text_element, &ax_value()) {
        Ok(s) => (strip_control_chars(&s), None),
        Err(e) => (String::new(), Some(e.to_string())),
    };
    diagnostics.push(format!(
        "ax_value_len={} err={}",
        full_text.chars().count(),
        full_text_err.as_deref().unwrap_or("none")
    ));

    // 选区范围
    let (range, range_err) = match get_selected_range(text_element) {
        Ok(r) => (r, None),
        Err(e) => (TextRange { start: 0, end: 0 }, Some(e.to_string())),
    };
    diagnostics.push(format!(
        "selected_range=({}, {}) err={}",
        range.start,
        range.end,
        range_err.as_deref().unwrap_or("none")
    ));

    // full_text 前 300 字预览（诊断偏移不对应问题）
    let preview: String = full_text.chars().take(300).collect();
    diagnostics.push(format!("full_text_preview={:?}", preview));

    // 用完后释放 find_text_element 返回的 +1 引用
    if owns_text_element {
        CFRelease(text_element as CFTypeRef);
    }

    // ── 内容校验：AX 树可能不含真实编辑器文本 ──
    // 自绘编辑器（Sublime Text、Vim GUI 等）的 AX 树只有静态文本
    // （如试用版水印 "UNREGISTERED"），不包含编辑器实际内容。
    // 判定：full_text 不包含选中文本 → AX 无真实内容 → 降级返回 None。
    if !full_text.is_empty() && !selected_text.is_empty() {
        let selected_trimmed = selected_text.trim();
        let full_trimmed = full_text.trim();
        if !full_trimmed.contains(selected_trimmed) {
            diagnostics.push(format!(
                "DEGRADED: full_text 不含选中文本，AX 树无真实内容（如 Sublime 自绘编辑器）"
            ));
            let diag = Some(diagnostics.join("\n  "));
            log::info!("[app-context] AX 诊断（降级）:\n  {}", diag.as_ref().unwrap());
            return Ok((
                SurroundingText {
                    before: None,
                    after: None,
                    window_title,
                },
                diag,
            ));
        }
    }

    let mut surrounding = if kind == AppKind::Terminal {
        // Terminal 模式：AXSelectedTextRange 和 AXValue 不在同一坐标系（iTerm2 的
        // range 基于整个终端缓冲区，AXValue 只是可见 scrollback），
        // 改为用选中文本在全文中搜索定位，按定位点切 before/after。
        let (before, after) = if !full_text.is_empty() && !selected_text.is_empty() {
            match full_text.find(selected_text.trim()) {
                Some(pos) => {
                    let before_text = &full_text[..pos];
                    let after_start = pos + selected_text.trim().len();
                    let after_text = if after_start < full_text.len() {
                        &full_text[after_start..]
                    } else {
                        ""
                    };
                    let b = truncate_text_tail(before_text, TERMINAL_MAX_LINES, TERMINAL_MAX_CHARS);
                    let a = truncate_text_head(after_text, TERMINAL_MAX_CHARS);
                    (b, a)
                }
                None => {
                    // 选中文本不在可见 scrollback 里（可能在光标行以下不可见区域）
                    // 退化：取 scrollback 末尾作 before
                    let b = truncate_text_tail(&full_text, TERMINAL_MAX_LINES, TERMINAL_MAX_CHARS);
                    (b, None)
                }
            }
        } else {
            (None, None)
        };
        SurroundingText {
            before,
            after,
            window_title,
        }
    } else {
        let mut s = extract_surrounding(&full_text, range, SURROUNDING_LIMIT);
        s.window_title = window_title;
        s
    };

    // 清理空字符串
    if surrounding.before.as_ref().is_some_and(|b| b.is_empty()) {
        surrounding.before = None;
    }
    if surrounding.after.as_ref().is_some_and(|a| a.is_empty()) {
        surrounding.after = None;
    }

    let diag_string = diagnostics.join("\n  ");
    log::info!("[app-context] AX 诊断:\n  {}", diag_string);

    Ok((surrounding, Some(diag_string)))
}

/// 判断 AX 角色是否为文本元素。
unsafe fn is_text_element(role: &str) -> bool {
    matches!(
        role,
        "AXTextArea" | "AXTextField" | "AXText" | "AXStaticText"
    )
}

/// 移除终端 scrollback 文本中的 C0 控制字符（保留 \n \t \r）。
/// iTerm2/Terminal 的 AXValue 常含 \0 间隔（UTF-16 编码残留）或终端控制序列。
fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || c == '\r' || !c.is_control())
        .collect()
}

/// 截取文本末尾：取最后 max_lines 行或 max_chars 字符（先达到者为准）。
/// 用于 Terminal before（选中文本之前的历史输出）。
fn truncate_text_tail(s: &str, max_lines: usize, max_chars: usize) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let lines: Vec<&str> = s.lines().collect();
    let start_line = lines.len().saturating_sub(max_lines);
    let by_lines: String = lines[start_line..].join("\n");

    if by_lines.chars().count() > max_chars {
        let char_start = by_lines.chars().count().saturating_sub(max_chars);
        let result: String = by_lines.chars().skip(char_start).collect();
        Some(result)
    } else {
        Some(by_lines)
    }
}

/// 截取文本头部：取前 max_chars 字符。
/// 用于 Terminal after（选中文本之后的输出）。
fn truncate_text_head(s: &str, max_chars: usize) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        Some(s.to_string())
    } else {
        let head: String = chars[..max_chars].iter().collect();
        Some(format!("{}…", head))
    }
}

/// 递归遍历 AX 子树，找到文本元素。
/// 优先找包含 selected_text 的文本元素；找不到则回退到第一个文本元素。
unsafe fn find_text_element_with_selected(
    element: AXUIElementRef,
    selected_text: &str,
    deadline: Instant,
) -> Option<AXUIElementRef> {
    // 第一轮：找包含 selected_text 的文本元素
    if let Some(found) = find_text_element_depth(element, 0, 8, Some(selected_text), deadline) {
        return Some(found);
    }
    // 回退：任意文本元素（仍检查 deadline）
    find_text_element_depth(element, 0, 8, None, deadline)
}

/// 递归遍历。selected_text = Some(t) 时只匹配包含 t 的文本元素；None 时匹配任意。
unsafe fn find_text_element_depth(
    element: AXUIElementRef,
    depth: usize,
    max_depth: usize,
    selected_text: Option<&str>,
    deadline: Instant,
) -> Option<AXUIElementRef> {
    if depth >= max_depth {
        return None;
    }

    // 超时检查——每层递归入口检查，避免庞大的 AX 树卡住
    if Instant::now() >= deadline {
        log::warn!("[app-context] find_text_element 超时 (depth={})", depth);
        return None;
    }

    // 当前元素角色
    let role = get_attribute_string(element, &ax_role()).unwrap_or_default();
    if is_text_element(&role) {
        // 读取 AXValue 检查是否匹配
        let value = match get_attribute_string(element, &ax_value()) {
            Ok(s) => Some(strip_control_chars(&s)),
            Err(_) => {
                // AXValue 读取失败也要检查是否纯粹无值——get_attribute_value 判断
                match get_attribute_value(element, &ax_value()) {
                    Ok(v) => {
                        CFRelease(v);
                        Some(String::new()) // 有值但不是 CFString
                    }
                    Err(_) => None, // 无值
                }
            }
        };

        if let Some(text) = &value {
            match selected_text {
                Some(sel) if !sel.is_empty() && text.contains(sel.trim()) => {
                    CFRetain(element as CFTypeRef);
                    return Some(element);
                }
                None if !text.is_empty() => {
                    CFRetain(element as CFTypeRef);
                    return Some(element);
                }
                _ => {}
            }
        }
    }

    // 遍历子元素
    let children = match get_attribute_value(element, &ax_children()) {
        Ok(v) => v,
        Err(_) => return None,
    };

    if !is_cf_array(children) {
        CFRelease(children);
        return None;
    }

    let cf_array =
        core_foundation::array::CFArray::<CFTypeRef>::wrap_under_create_rule(children as *const _);
    let count = cf_array.len().min(100);

    for i in 0..count {
        let Some(child_ref) = cf_array.get(i) else {
            continue;
        };
        let child: AXUIElementRef = *child_ref as AXUIElementRef;
        if child.is_null() {
            continue;
        }
        if let Some(found) = find_text_element_depth(child, depth + 1, max_depth, selected_text, deadline) {
            return Some(found);
        }
    }

    None
}

/// 安全读取 AX 属性值（返回 CFTypeRef，调用方负责 CFRelease）。
unsafe fn get_attribute_value(
    element: AXUIElementRef,
    attr: &CFString,
) -> anyhow::Result<CFTypeRef> {
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != 0 || value.is_null() {
        return Err(anyhow::anyhow!(
            "AXUIElementCopyAttributeValue({}) error: {} ({})",
            attr.to_string(),
            err,
            ax_error_desc(err),
        ));
    }
    Ok(value)
}

/// 读取 AX 字符串属性。
///
/// AX 属性值可能是任意 CFType（CFString / CFNumber / CFBoolean ...），
/// 盲目按 CFString 解转会调 `_fastCStringContents` 到非字符串类型上导致崩溃。
/// 这里用 CFGetTypeID 检查类型，非 CFString 返回 Err。
unsafe fn get_attribute_string(
    element: AXUIElementRef,
    attr: &CFString,
) -> anyhow::Result<String> {
    let value = get_attribute_value(element, attr)?;

    if !is_cf_string(value) {
        let actual_type = core_foundation::base::CFGetTypeID(value);
        CFRelease(value);
        return Err(anyhow::anyhow!(
            "AX 属性 {} 返回非 CFString 类型 (CFTypeID={})",
            attr.to_string(),
            actual_type
        ));
    }

    let cf_string = CFString::wrap_under_create_rule(value as *const _);
    Ok(cf_string.to_string())
}

/// 检查 CFTypeRef 是否为 CFString 类型（null → false）。
unsafe fn is_cf_string(value: CFTypeRef) -> bool {
    if value.is_null() {
        return false;
    }
    CFGetTypeID(value) == CFString::type_id()
}

/// 检查 CFTypeRef 是否为 CFArray 类型（null → false）。
/// AXChildren 在某些元素上返回 CFBoolean/CFNull 而非 CFArray，
/// 盲目 wrap 为 CFArray 会类型混淆崩溃（与 CFString 同类 bug）。
unsafe fn is_cf_array(value: CFTypeRef) -> bool {
    if value.is_null() {
        return false;
    }
    CFGetTypeID(value) == core_foundation::array::CFArray::<CFTypeRef>::type_id()
}

/// 检查 CFTypeRef 是否为 AXValue 类型（null → false）。
/// AXSelectedTextRange 应返回 AXValue，非 AXValue 调 AXValueGetValue 是 UB。
unsafe fn is_cf_value(value: CFTypeRef) -> bool {
    if value.is_null() {
        return false;
    }
    // AXValueGetTypeID 在 ApplicationServices framework 中，用 FFI 获取
    CFGetTypeID(value) == crate::app_context::ffi::ax_value_type_id()
}

/// 读取选区范围 (start, end)，单位为 UTF-16 字符偏移（AX 的 CFRange 单位）。
unsafe fn get_selected_range(element: AXUIElementRef) -> anyhow::Result<TextRange> {
    let value = get_attribute_value(element, &ax_selected_text_range())?;

    // 类型守卫：AXSelectedTextRange 应返回 AXValue 类型，
    // 非 AXValue（如 CFString/CFNumber）直接调用 AXValueGetValue 是未定义行为。
    if !is_cf_value(value) {
        let actual_type = CFGetTypeID(value);
        CFRelease(value);
        return Err(anyhow::anyhow!(
            "AXSelectedTextRange 返回非 AXValue 类型 (CFTypeID={})",
            actual_type
        ));
    }

    let ax_value = value as AXValueRef;

    let mut range = CFRange {
        location: 0,
        length: 0,
    };
    let ok = AXValueGetValue(
        ax_value,
        kAXValueCFRangeType,
        &mut range as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(value);

    if ok == 0 {
        return Err(anyhow::anyhow!("AXValueGetValue failed for CFRange"));
    }

    // AX range 是 UTF-16 偏移，近似为 Unicode 标量偏移（CJK BMP 内一致）
    Ok(TextRange {
        start: range.location.max(0) as usize,
        end: (range.location + range.length).max(0) as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    // ── is_cf_string ──

    /// CFString 的 CFTypeRef 能被 `is_cf_string` 识别为 true。
    #[test]
    fn test_is_cf_string_true() {
        let s = CFString::new("hello world");
        let raw: CFTypeRef = s.as_CFTypeRef(); // get rule，不转移所有权
        let result = unsafe { is_cf_string(raw) };
        assert!(result);
    }

    /// CFNumber 的 CFTypeRef 被识别为 false——这是线上崩溃的根因：
    /// AX 返回 CFNumber 而代码按 CFString 解转。
    #[test]
    fn test_is_cf_string_false_for_number() {
        let n = CFNumber::from(42i32);
        let raw: CFTypeRef = n.as_CFTypeRef();
        let result = unsafe { is_cf_string(raw) };
        assert!(!result);
    }

    /// CFBoolean 也不是 CFString。
    #[test]
    fn test_is_cf_string_false_for_boolean() {
        let b = CFBoolean::true_value();
        let raw: CFTypeRef = b.as_CFTypeRef();
        let result = unsafe { is_cf_string(raw) };
        assert!(!result);
    }

    /// null 指针 → false，不应崩溃。
    #[test]
    fn test_is_cf_string_null() {
        let result = unsafe { is_cf_string(std::ptr::null()) };
        assert!(!result);
    }

    /// CJK（多字节）CFString 也能正确识别。
    #[test]
    fn test_is_cf_string_cjk() {
        let s = CFString::new("你好世界");
        let raw: CFTypeRef = s.as_CFTypeRef();
        let result = unsafe { is_cf_string(raw) };
        assert!(result);
    }

    /// 确认 CFGetTypeID 对 CFString 和 CFNumber 返回不同的值——
    /// 这是 is_cf_string 安全性的基础。
    #[test]
    fn test_cf_typeid_distinct() {
        let s = CFString::new("test");
        let n = CFNumber::from(1i32);
        let s_id = unsafe { CFGetTypeID(s.as_CFTypeRef()) };
        let n_id = unsafe { CFGetTypeID(n.as_CFTypeRef()) };
        assert_ne!(s_id, n_id, "CFString 和 CFNumber 必须有不同的 CFTypeID");
        assert_eq!(s_id, CFString::type_id());
    }

    /// 端到端模拟「AX 返回 CFNumber 被当 CFString 解转」的旧崩溃路径：
    /// 创建一个 CFNumber，用 is_cf_string 判定为 false → 不走 CFString 解转 → 不崩溃。
    /// 如果去掉类型检查直接 wrap_under_create_rule 会 NSException crash（此测试验证护栏生效）。
    #[test]
    fn test_type_check_prevents_crash_on_wrong_type() {
        let n = CFNumber::from(99i32);

        // 模拟 AX 返回的 create-rule CFTypeRef（+1 retain，需手动 release）
        // CFNumber::as_CFTypeRef() 是 get-rule（不 +1），手动 retain 模拟 create
        let raw: CFTypeRef = n.as_CFTypeRef();
        // 手动 retain 模拟 AX "Copy" 语义（返回 +1）
        // core-foundation 没有直接 CFRetain，但 CFGetTypeID 本身是 get-rule 无需 retain
        // 这里只需验证 is_cf_string 返回 false，保证后续不会 wrap 为 CFString

        let is_string = unsafe { is_cf_string(raw) };
        assert!(!is_string, "CFNumber 不应被识别为 CFString");

        // 如果 is_string 为 false，get_attribute_string 会走 Err 路径，
        // 不会执行 CFString::wrap_under_create_rule → 不触发 _fastCStringContents 崩溃。
        // （get_attribute_string 的 AX 调用部分无法在单测中模拟，这里验证的是判定逻辑）
    }

    // ── is_cf_array ──

    /// CFArray 的 CFTypeRef 被正确识别。
    #[test]
    fn test_is_cf_array_true() {
        let arr: core_foundation::array::CFArray<CFString> =
            core_foundation::array::CFArray::from_CFTypes(&[CFString::new("hi")]);
        let raw: CFTypeRef = arr.as_CFTypeRef();
        let result = unsafe { is_cf_array(raw) };
        assert!(result);
    }

    /// CFBoolean（AXChildren 在某些元素上返回 false）→ 不是 CFArray。
    /// 这正是第二个崩溃的根因：AXChildren 返回 CFBoolean，被当 CFArray wrap。
    #[test]
    fn test_is_cf_array_false_for_boolean() {
        let b = CFBoolean::false_value();
        let raw: CFTypeRef = b.as_CFTypeRef();
        let result = unsafe { is_cf_array(raw) };
        assert!(!result);
    }

    /// CFNumber 也不是 CFArray。
    #[test]
    fn test_is_cf_array_false_for_number() {
        let n = CFNumber::from(0i32);
        let raw: CFTypeRef = n.as_CFTypeRef();
        let result = unsafe { is_cf_array(raw) };
        assert!(!result);
    }

    /// null → false。
    #[test]
    fn test_is_cf_array_null() {
        let result = unsafe { is_cf_array(std::ptr::null()) };
        assert!(!result);
    }

    // ── strip_control_chars ──

    #[test]
    fn test_strip_null_bytes() {
        // iTerm2 AXValue 的典型模式：CJK 字符间混 \0
        let input = "\0项\0目\0是\0多\0\0crate\0Rust";
        assert_eq!(strip_control_chars(input), "项目是多crateRust");
    }

    #[test]
    fn test_strip_keeps_newlines_tabs() {
        let input = "line1\n\0line2\t\0end\r\n";
        assert_eq!(strip_control_chars(input), "line1\nline2\tend\r\n");
    }

    #[test]
    fn test_strip_cjk_no_nulls() {
        let input = "你好世界 hello";
        assert_eq!(strip_control_chars(input), "你好世界 hello");
    }

    #[test]
    fn test_strip_all_control() {
        let input = "\0\0\0\x01\x02\x03";
        assert_eq!(strip_control_chars(input), "");
    }

    #[test]
    fn test_strip_normal_text_unchanged() {
        let input = "正常文本 without any control chars";
        assert_eq!(strip_control_chars(input), input);
    }

    // ── ax_error_desc ──

    #[test]
    fn test_ax_error_desc_known() {
        assert_eq!(ax_error_desc(-25212), "kAXErrorAPIDisabled (AX API 被禁用——检查辅助功能权限)");
        assert_eq!(ax_error_desc(-25205), "kAXErrorCannotComplete (操作无法完成)");
        assert_eq!(ax_error_desc(-25200), "kAXErrorFailure (通用失败)");
        assert_eq!(ax_error_desc(0), "kAXErrorSuccess (成功)");
    }

    #[test]
    fn test_ax_error_desc_unknown() {
        assert_eq!(ax_error_desc(-99999), "未知 AX 错误码");
    }

    // ── truncate_text_tail / truncate_text_head ──

    #[test]
    fn test_tail_by_lines() {
        let s = "l1\nl2\nl3\nl4\nl5\nSELECTED";
        // 6 lines, take last 2 → ["l5", "SELECTED"]
        let result = truncate_text_tail(s, 2, 1000).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "l5");
        assert_eq!(lines[1], "SELECTED");
    }

    #[test]
    fn test_tail_by_chars() {
        let s = "abcdefghijklmnopqrstuvwxyz";
        let result = truncate_text_tail(s, 1000, 5).unwrap();
        assert_eq!(result, "vwxyz");
    }

    #[test]
    fn test_tail_empty() {
        assert_eq!(truncate_text_tail("", 10, 10), None);
    }

    #[test]
    fn test_head_normal() {
        let s = "abcdef";
        assert_eq!(truncate_text_head(s, 3), Some("abc…".to_string()));
    }

    #[test]
    fn test_head_short() {
        let s = "ab";
        assert_eq!(truncate_text_head(s, 5), Some("ab".to_string()));
    }

    #[test]
    fn test_head_empty() {
        assert_eq!(truncate_text_head("", 5), None);
    }
}
