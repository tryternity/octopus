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
        let bid = bundle_id.as_deref().unwrap_or("");
        let (surrounding, diagnostics) = if Instant::now() < deadline {
            // Browser 优先用 AppleScript execute javascript（直接读 DOM，比 AX 准确）
            if kind == AppKind::Browser {
                if let Some(result) = gather_browser_via_applescript(
                    bid,
                    selected_text,
                    deadline,
                ) {
                    (Some(result.0), result.1)
                } else {
                    // AppleScript JS 失败（可能未开启 Allow JavaScript from Apple Events），fallback 到 AX
                    let mut diag_prefix = "browser AppleScript JS 失败，fallback 到 AX\n".to_string();
                    match gather_surrounding(pid, kind, selected_text, deadline, bid) {
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
                match gather_surrounding(pid, kind, selected_text, deadline, bid) {
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
///
/// 2026-07-20 perf：从私有改为 pub(crate)，供 finder_selection / sublime_plugin
/// 复用——原各自用 osascript 跑相同 System Events 脚本（每个 ~200-400ms），
/// 改用 NSWorkspace 直调（< 1ms）。
pub(crate) fn frontmost_app() -> Option<(i32, Option<String>, String)> {
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

/// 前台 app 的 bundle id（NSWorkspace 直调，< 1ms）。
/// 替代 osascript "tell System Events to get bundle id of frontmost process"
/// （后者启动 osascript ~200-400ms）。
pub(crate) fn frontmost_bundle_id() -> Option<String> {
    frontmost_app().and_then(|(_, bid, _)| bid)
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
    // 另转义 U+2028/U+2029（JS 字符串字面量中的换行符）。
    let escaped = selected_text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
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

/// 判断路径的 file_name 部分是否精确匹配 filename（大小写敏感）。
fn path_matches_filename(path: &str, filename: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().as_ref() == filename)
        .unwrap_or(false)
}

/// 读文件内容为纯文本。支持纯文本、Office 格式（.docx/.xlsx/.pptx）和 PDF。
/// Office 格式优先用 officecli（如果安装了），否则 fallback 到内置 zip+XML 解析。
/// 子进程（pdftotext/officecli）受 deadline 约束，防永久挂起。
fn read_file_as_text(path: &std::path::Path, deadline: Instant) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();

    match ext.as_str() {
        "pdf" => read_pdf_text(path, deadline),
        "pages" => read_pages_text(path),
        "docx" | "pptx" | "xlsx" => {
            // 优先用 officecli（更健壮，处理修订/批注/公式/图表）
            if let Some(text) = try_officecli_text(path, deadline) {
                return Some(text);
            }
            // Fallback 到内置 zip+XML 解析
            match ext.as_str() {
                "xlsx" => read_xlsx_text(path),
                _ => read_ooxml_text(path, &ext),
            }
        }
        _ => {
            // 纯文本格式：直接读取（可能非 UTF-8，用 lossy 转换）
            let bytes = std::fs::read(path).ok()?;
            if bytes.is_empty() {
                return None;
            }
            // 检查是否为 zip（二进制 Office 文件误存为 .txt）
            if bytes.starts_with(b"PK\x03\x04") {
                return read_ooxml_text(path, "docx");
            }
            Some(String::from_utf8_lossy(&bytes).to_string())
        }
    }
}

/// 尝试用 officecli 提取 Office 文档文本。
/// officecli 是单二进制工具（iOfficeAI/OfficeCLI），支持 .docx/.xlsx/.pptx。
/// 输出格式 `[/path] text`，去掉路径标签后是纯文本。
fn try_officecli_text(path: &std::path::Path, deadline: Instant) -> Option<String> {
    let mut cmd = std::process::Command::new("officecli");
    cmd.arg("view").arg(path).arg("text");
    let output = super::run_command_with_deadline(cmd, deadline)?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    // 去掉每行的 [/path] 前缀
    let raw = String::from_utf8_lossy(&output.stdout);
    let cleaned: String = raw
        .lines()
        .map(|line| {
            // 去掉 [/body/...] 路径前缀
            if let Some(idx) = line.find("] ") {
                &line[idx + 2..]
            } else {
                line
            }
        })
        .collect::<Vec<&str>>()
        .join("\n");

    if cleaned.trim().is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// 从 .pages 文件提取文本。.pages 是 iWork 格式（zip 内 Protobuf iwa），
/// 不是 OOXML。从 Document.iwa 中提取明文 UTF-8 片段（Protobuf 的 string 字段）。
fn read_pages_text(path: &std::path::Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    // 查找 Index/Document.iwa
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).ok()?;
        if file.name() != "Index/Document.iwa" {
            continue;
        }
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buf).ok()?;

        // Protobuf 中 string 字段是 length-delimited（wire type 2）
        // 提取连续的 UTF-8 可打印字符片段（>= 4 字符）
        let mut fragments = Vec::new();
        let mut current = Vec::new();
        for &byte in &buf {
            if byte >= 0x20 && byte != 0x7f || byte >= 0xc0 {
                current.push(byte);
            } else {
                if current.len() >= 6 {
                    if let Ok(s) = std::str::from_utf8(&current) {
                        // 过滤掉明显的二进制噪音（含大量 ? 的串）
                        let readable = s.chars().filter(|c| !c.is_control()).count();
                        if readable >= 4 && !s.contains("\u{200B}\u{200B}") {
                            fragments.push(s.to_string());
                        }
                    }
                }
                current.clear();
            }
        }

        if !fragments.is_empty() {
            return Some(fragments.join(" "));
        }
    }

    None
}

/// 从 PDF 提取文本。优先用系统 pdftotext（poppler），没有则返回 None。
fn read_pdf_text(path: &std::path::Path, deadline: Instant) -> Option<String> {
    let mut cmd = std::process::Command::new("pdftotext");
    cmd.arg(path).arg("-"); // stdout
    let output = super::run_command_with_deadline(cmd, deadline)?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    // PDF 排版换行合并：pdftotext 按 PDF 布局断行，CJK 文本每 N 字换行。
    // 选中文字可能跨行，精确搜索匹配不到。
    // 合并策略：前一行末尾是 CJK 字符 → 合并到前一行（排版换行）。
    let merged = merge_cjk_line_breaks(&raw);
    Some(merged)
}

/// 合并 CJK 排版换行：前一行末尾是 CJK 字符 → 合并（非语义换行）。
fn merge_cjk_line_breaks(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut merged: Vec<String> = Vec::new();

    for line in lines {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            merged.push(String::new());
            continue;
        }
        if let Some(last) = merged.last() {
            let prev = last.trim_end();
            if !prev.is_empty() {
                let last_char = prev.chars().last();
                // 前一行末尾是 CJK → 合并（排版换行，非语义换行）
                if let Some(ch) = last_char {
                    if (ch as u32) >= 0x2e80 {
                        *merged.last_mut().unwrap() = format!("{}{}", prev, line.trim());
                        continue;
                    }
                }
            }
        }
        merged.push(line.to_string());
    }

    merged.join("\n")
}

/// 从 OOXML 格式（.docx/.pptx）中提取纯文本。
/// .docx: word/document.xml 的 <w:t> 节点
/// .pptx: ppt/slides/slideN.xml 的 <a:t> 节点
fn read_ooxml_text(path: &std::path::Path, ext: &str) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    let mut texts = Vec::new();

    for i in 0..archive.len() {
        let file = archive.by_index(i).ok()?;
        let name = file.name().to_string();

        let is_target = match ext {
            "docx" => name == "word/document.xml",
            "pptx" => name.starts_with("ppt/slides/slide") && name.ends_with(".xml"),
            _ => false,
        };

        if !is_target {
            continue;
        }

        let xml_content = std::io::read_to_string(file).ok()?;
        let extracted = extract_text_from_ooxml_xml(&xml_content);
        texts.push(extracted);
    }

    if texts.is_empty() {
        return None;
    }

    Some(texts.join("\n\n"))
}

/// 从 .xlsx 中提取纯文本。遍历 sharedStrings.xml 和 sheetN.xml。
fn read_xlsx_text(path: &std::path::Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    let mut shared_strings: Vec<String> = Vec::new();
    let mut sheet_texts: Vec<String> = Vec::new();

    for i in 0..archive.len() {
        let file = archive.by_index(i).ok()?;
        let name = file.name().to_string();
        let content = std::io::read_to_string(file).ok()?;

        if name == "xl/sharedStrings.xml" {
            // 共享字符串表
            shared_strings = extract_xlsx_shared_strings(&content);
        } else if name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml") {
            // 工作表——提取 cell 值
            sheet_texts.push(extract_xlsx_sheet_text(&content, &shared_strings).join("\t"));
        }
    }

    if sheet_texts.is_empty() {
        return None;
    }

    Some(sheet_texts.join("\n"))
}

/// 从 OOXML XML 中提取 <w:t> 或 <a:t> 文本节点。
fn extract_text_from_ooxml_xml(xml: &str) -> String {
    let mut result = Vec::new();
    let mut in_tag = false;
    let mut tag_name = String::new();
    let mut in_text = false;
    let mut text_content = String::new();

    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_name.clear();
            }
            '>' => {
                in_tag = false;
                let tag = tag_name.trim();
                // <w:t> 或 <a:t>（含属性如 <w:t xml:space="preserve">）
                if tag.starts_with("w:t") || tag.starts_with("a:t") {
                    in_text = true;
                } else if tag.starts_with("/w:t") || tag.starts_with("/a:t") {
                    if !text_content.is_empty() {
                        result.push(text_content.clone());
                        text_content.clear();
                    }
                    in_text = false;
                } else if tag == "w:p" || tag.starts_with("w:p ") || tag == "a:p" {
                    // 段落结束 → 换行
                    if !result.is_empty() && !result.last().unwrap().is_empty() {
                        result.push("\n".to_string());
                    }
                }
            }
            _ if in_tag => tag_name.push(ch),
            _ if in_text => text_content.push(ch),
            _ => {}
        }
    }

    result.join("")
}

/// 从 sharedStrings.xml 提取共享字符串列表。
fn extract_xlsx_shared_strings(xml: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut in_tag = false;
    let mut tag = String::new();
    let mut collect = false;
    let mut buf = String::new();

    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' => {
                in_tag = false;
                let t = tag.trim();
                if t.starts_with("t") && !t.starts_with("/t") {
                    collect = true;
                    buf.clear();
                } else if t.starts_with("/t") {
                    if collect && !buf.is_empty() {
                        strings.push(buf.clone());
                    }
                    collect = false;
                }
            }
            _ if in_tag => tag.push(ch),
            _ if collect => buf.push(ch),
            _ => {}
        }
    }

    strings
}

/// 从 sheetN.xml 提取单元格文本（引用 sharedStrings 或内联值）。
fn extract_xlsx_sheet_text(xml: &str, shared: &[String]) -> Vec<String> {
    // 简化：提取 <c> 标签的 <v> 值和 t="s" 引用
    // 用正则更简洁，但避免额外依赖
    let mut texts = Vec::new();
    let chars: Vec<char> = xml.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // 查找 <c 开头
        if chars[i] == '<' && i + 1 < chars.len() && chars[i + 1] == 'c' {
            // 提取属性直到 >
            let mut attrs = String::new();
            i += 2;
            while i < chars.len() && chars[i] != '>' {
                attrs.push(chars[i]);
                i += 1;
            }
            // 检查是否 t="s"（共享字符串引用）
            let is_shared = attrs.contains("t=\"s\"");
            // 查找 <v> 值
            i += 1;
            if i < chars.len() && chars[i] == '<' {
                let mut tag = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '>' {
                    tag.push(chars[i]);
                    i += 1;
                }
                i += 1; // skip >
                if tag.trim().starts_with("v") {
                    let mut val = String::new();
                    while i < chars.len() && chars[i] != '<' {
                        val.push(chars[i]);
                        i += 1;
                    }
                    if !val.is_empty() {
                        if is_shared {
                            if let Ok(idx) = val.parse::<usize>() {
                                if idx < shared.len() {
                                    texts.push(shared[idx].clone());
                                }
                            }
                        } else {
                            texts.push(val);
                        }
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    texts
}

/// Pages/Keynote AppleScript 文本提取。
/// `body text of document 1` 返回当前文档全文。
fn try_pages_applescript(selected_text: &str, window_title: &Option<String>, deadline: Instant) -> Option<SurroundingText> {
    use std::process::Command;

    let script = r#"tell application "Pages"
    set d to document 1
    return body text of d
end tell"#;

    let mut cmd = Command::new("osascript");
    cmd.args(["-e", script]);
    let output = super::run_command_with_deadline(cmd, deadline)?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let full_text = String::from_utf8_lossy(&output.stdout).to_string();
    log::info!("[app-context] Pages AppleScript: {} chars", full_text.chars().count());

    let result = slice_around_text(&full_text, selected_text, 1000)?;

    Some(SurroundingText {
        before: result.0,
        after: result.1,
        window_title: window_title.clone(),
    })
}

/// 从 bundle_id 推断进程名（用于 lsof -c 匹配）。
fn bundle_id_to_procname(bundle_id: &str) -> String {
    let id = bundle_id.to_ascii_lowercase();
    if id.contains("kingsoft") {
        "wpsoffice".to_string()
    } else if id.contains("iwork.pages") {
        "Pages".to_string()
    } else if id.contains("iwork.keynote") {
        "Keynote".to_string()
    } else if id.contains("iwork.numbers") {
        "Numbers".to_string()
    } else if id.contains("sublimetext") {
        "Sublime".to_string()
    } else {
        // 通用：取 bundle_id 最后一段
        bundle_id
            .split('.')
            .next_back()
            .unwrap_or(bundle_id)
            .to_string()
    }
}

/// 从 lsof -F n 输出中筛选文档文件路径（纯函数，可单测）。
///
/// 过滤规则：
/// - 只保留 `n` 前缀行（lsof -F n 格式）
/// - 排除 /dev/ 设备文件
/// - 排除 .~ 临时锁文件
/// - 只保留 .docx/.xlsx/.pptx/.pdf/.doc/.xls/.ppt 扩展名
fn filter_lsof_doc_files(lsof_output: &str) -> Vec<String> {
    let doc_exts = ["docx", "xlsx", "pptx", "pdf", "doc", "xls", "ppt", "pages"];
    lsof_output
        .lines()
        .filter(|l| l.starts_with('n'))
        .map(|l| &l[1..])
        .filter(|path| {
            !path.starts_with("/dev/") && !path.contains("/.~") && !path.contains(".~")
        })
        .filter(|path| {
            std::path::Path::new(path)
                .extension()
                .map(|ext| {
                    let ext = ext.to_string_lossy().to_lowercase();
                    doc_exts.contains(&ext.as_str())
                })
                .unwrap_or(false)
        })
        .map(|s| s.to_string())
        .collect()
}

/// 通过 lsof 获取进程打开的文件路径，读取内容并匹配选中文本。
///
/// 当 AX 树不含真实编辑器内容且窗口标题不可靠时（WPS/Pages 等），
/// lsof 可以列出进程当前打开的文档文件。
/// 进程名从 bundle_id 推断。
fn try_lsof_context(selected_text: &str, bundle_id: &str, deadline: Instant) -> Option<SurroundingText> {
    // 从 bundle_id 推断进程名
    let proc_name = bundle_id_to_procname(bundle_id);

    // 1. lsof 获取进程打开的文件
    let mut cmd = std::process::Command::new("lsof");
    cmd.args(["-c", &proc_name, "-F", "n"]);
    let output = super::run_command_with_deadline(cmd, deadline)?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // 2. 筛选文档文件
    let candidates = filter_lsof_doc_files(&stdout);

    log::info!("[app-context] lsof ({}): {} 个候选文件", proc_name, candidates.len());

    // 3. 逐个尝试读取 + 匹配选中文本
    for file_path in &candidates {
        let path = std::path::Path::new(file_path);
        let content = match read_file_as_text(path, deadline) {
            Some(c) => c,
            None => continue,
        };

        if let Some(result) = slice_around_text(&content, selected_text, 1000) {
            let window_title = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string());
            log::info!("[app-context] lsof 命中: {}", file_path);
            return Some(SurroundingText {
                before: result.0,
                after: result.1,
                window_title,
            });
        }
    }

    None
}

/// 自绘编辑器 fallback：从磁盘读文件内容获取上下文。
///
/// 当 AX 树不含真实编辑器内容时（Sublime Text、WPS），尝试：
/// 1. 从窗口标题提取文件名
/// 2. 用 App 的 session 文件查找完整路径（Sublime）或直接搜索
/// 3. 读文件内容，用 selected_text 定位切 before/after
///
/// **限制**：仅对纯文本格式（.txt/.md/.rs/.py/.json 等）有效。
/// Office 格式（.docx/.xlsx）是 zip 压缩二进制，read_to_string 会失败。
/// WPS 用户编辑 .txt/.md 时可受益，编辑 .docx 时 fallback 无效。
fn try_read_file_context(
    window_title: &str,
    bundle_id: &str,
    selected_text: &str,
    deadline: Instant,
) -> Option<SurroundingText> {
    // 1. 从标题提取文件名
    let filename = extract_filename_from_title(window_title)?;
    log::info!(
        "[app-context] 磁盘 fallback: 标题='{}' → 文件名='{}'",
        window_title,
        filename
    );

    // 2. 查找文件完整路径
    let file_path = find_file_path(&filename, bundle_id, deadline)?;
    log::info!("[app-context] 磁盘 fallback: 找到文件 {}", file_path.display());

    // 3. 读文件内容——支持纯文本和 Office 格式（.docx/.xlsx/.pptx）
    let content = read_file_as_text(&file_path, deadline)?;
    if content.is_empty() {
        return None;
    }

    // 4. 用 selected_text 定位
    let result = slice_around_text(&content, selected_text, 1000)?;
    Some(SurroundingText {
        before: result.0,
        after: result.1,
        window_title: Some(window_title.to_string()),
    })
}

/// 在全文中搜索 selected_text，返回 (before, after) 各截断到 limit 字。
/// 尝试精确匹配 → 忽略大小写匹配 → 兼容字符归一化匹配。
fn slice_around_text(
    full_text: &str,
    selected_text: &str,
    limit: usize,
) -> Option<(Option<String>, Option<String>)> {
    let sel = selected_text.trim();
    if sel.is_empty() || full_text.is_empty() {
        return None;
    }

    // find 返回 byte offset，转 char offset 以正确处理多字节字符
    let full_chars: Vec<char> = full_text.chars().collect();

    // 尝试 1：精确匹配
    if let Some(pos_bytes) = full_text.find(sel) {
        return Some(char_level_slice(&full_chars, pos_bytes, sel, full_text, limit));
    }

    // 尝试 2：忽略大小写匹配
    let sel_lower = sel.to_lowercase();
    if let Some(pos_bytes) = full_text.to_lowercase().find(&sel_lower) {
        return Some(char_level_slice(&full_chars, pos_bytes, sel, full_text, limit));
    }

    // 尝试 3：NFKC 归一化匹配
    // WPS Cmd+C 可能产生康熙部首（U+2Exx）或全角标点（U+FFxx），
    // 而 pdftotext/officecli 提取的是正常 Unicode → 精确匹配失败。
    use unicode_normalization::UnicodeNormalization;

    let normalize_nfkc = |s: &str| -> String {
        s.nfkc()
            .filter(|c| {
                let cp = *c as u32;
                (0x4E00..=0x9FFF).contains(&cp) || c.is_ascii_alphanumeric()
            })
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };

    let sel_norm = normalize_nfkc(sel);
    if sel_norm.len() >= 5 {
        // 构建归一化映射表：一次遍历，记录每个归一化字符对应的原文 byte offset
        let mut full_norm = String::new();
        let mut norm_to_byte: Vec<usize> = Vec::new(); // norm char index → original byte offset

        let mut byte_pos = 0usize;
        for orig_char in full_text.chars() {
            let char_bytes = orig_char.len_utf8();
            // NFKC 归一化单个字符（兼容字符可能展开为多字符，如全角→半角）
            let nfkc_str: String = orig_char.nfkc().collect();
            for nc in nfkc_str.chars() {
                let cp = nc as u32;
                if (0x4E00..=0x9FFF).contains(&cp) || nc.is_ascii_alphanumeric() {
                    full_norm.push(nc.to_ascii_lowercase());
                    norm_to_byte.push(byte_pos);
                }
            }
            byte_pos += char_bytes;
        }

        if let Some(pos) = full_norm.find(&sel_norm) {
            let char_index = full_norm[..pos].chars().count();
            if char_index < norm_to_byte.len() {
                let pos_bytes = norm_to_byte[char_index];
                return Some(char_level_slice(&full_chars, pos_bytes, sel, full_text, limit));
            }
        }
    }

    None
}

/// 按 byte offset 在 char 数组中切片 before/after。
fn char_level_slice(
    full_chars: &[char],
    pos_bytes: usize,
    sel: &str,
    full_text: &str,
    limit: usize,
) -> (Option<String>, Option<String>) {
    let pos_chars = full_text[..pos_bytes].chars().count();
    let sel_chars = sel.chars().count();

    let before = if pos_chars > 0 {
        let start = pos_chars.saturating_sub(limit);
        Some(full_chars[start..pos_chars].iter().collect())
    } else {
        None
    };
    let end = pos_chars + sel_chars;
    let after = if end < full_chars.len() {
        let after_end = (end + limit).min(full_chars.len());
        Some(full_chars[end..after_end].iter().collect())
    } else {
        None
    };

    (before, after)
}

/// 从窗口标题提取文件名。
/// "test.txt — Sublime Text" → "test.txt"
/// "report.docx - WPS Office" → "report.docx"
/// "untitled — Sublime Text" → None（未保存文件）
fn extract_filename_from_title(title: &str) -> Option<String> {
    // 取 " — " 或 " - " 前面的部分（em dash 优先，hyphen 回退）
    let name_part = if title.contains(" — ") {
        title.split(" — ").next()?
    } else if title.contains(" - ") {
        title.split(" - ").next()?
    } else {
        title
    };
    let name_part = name_part.trim();

    if name_part.is_empty() || name_part == "untitled" || name_result_is_app_name(name_part) {
        return None;
    }

    // 确保看起来像文件名（含扩展名或路径分隔符）
    if name_part.contains('.') || name_part.contains('/') {
        Some(name_part.to_string())
    } else {
        None
    }
}

/// 判断标题前半段是否是 App 名（误匹配防护）。
fn name_result_is_app_name(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        "sublime text" | "wps office" | "code" | "finder" | "terminal" | "safari"
    )
}

/// 查找文件的完整路径。
/// 先查 Sublime session，再查 mdfind（Spotlight）。
fn find_file_path(filename: &str, bundle_id: &str, deadline: Instant) -> Option<std::path::PathBuf> {
    // Sublime session 查找
    if bundle_id.contains("sublimetext") {
        if let Some(path) = find_in_sublime_session(filename) {
            return Some(path);
        }
    }

    // Spotlight fallback（文件名精确匹配）。
    // mdfind 通常 <100ms，但仍受 deadline 约束防异常挂起。
    // 误读同名文件的内容兜底：slice_around_text 的 find() 在误读文件中大概率
    // 不含 selected_text → 返回 None → 自动降级为空 surrounding。
    let mut cmd = std::process::Command::new("mdfind");
    cmd.args(["-name", filename]);
    let output = super::run_command_with_deadline(cmd, deadline)?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // 取第一个匹配
    stdout
        .lines()
        .filter(|l| !l.is_empty())
        .find(|l| {
            // 文件名精确匹配（mdfind -name 可能返回子串匹配）
            std::path::Path::new(l)
                .file_name()
                .map(|n| n.to_string_lossy().as_ref() == filename)
                .unwrap_or(false)
        })
        .map(std::path::PathBuf::from)
}

/// 从 Sublime Text session 文件中查找文件路径。
fn find_in_sublime_session(filename: &str) -> Option<std::path::PathBuf> {
    // Sublime Text 3/4 的 session 路径
    let home = dirs::home_dir()?;
    let candidates = [
        home.join("Library/Application Support/Sublime Text/Local/Session.sublime_session"),
        home.join("Library/Application Support/Sublime Text 3/Local/Session.sublime_session"),
    ];

    for session_path in &candidates {
        let content = match std::fs::read_to_string(session_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(path) = find_file_in_session_json(&json, filename) {
                return Some(path);
            }
        }
    }

    None
}

/// 在 Sublime session JSON 中查找文件路径（纯函数，可单测）。
/// 遍历所有窗口的 file_history 和 buffers，用 file_name 精确匹配。
fn find_file_in_session_json(
    json: &serde_json::Value,
    filename: &str,
) -> Option<std::path::PathBuf> {
    let windows = json["windows"].as_array()?;
    for win in windows {
        // file_history
        if let Some(history) = win["file_history"].as_array() {
            for item in history {
                if let Some(path) = item.as_str() {
                    if path_matches_filename(path, filename) {
                        return Some(std::path::PathBuf::from(path));
                    }
                }
            }
        }
        // buffers
        if let Some(buffers) = win["buffers"].as_array() {
            for buf in buffers {
                if let Some(path) = buf["file"].as_str() {
                    if !path.is_empty() && path_matches_filename(path, filename) {
                        return Some(std::path::PathBuf::from(path));
                    }
                }
            }
        }
    }
    None
}

/// 通过 AX 采集选区周围文本。返回 (surrounding, AX 诊断信息)。
fn gather_surrounding(pid: i32, kind: AppKind, selected_text: &str, deadline: Instant, bundle_id: &str) -> anyhow::Result<(SurroundingText, Option<String>)> {
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
                let surrounding = build_surrounding(focused_element, app_element, kind, selected_text, deadline, bundle_id);
                CFRelease(focused_ref);
                surrounding
            }
            Err(e) => {
                log::info!("[app-context] 无法获取焦点元素（含重试）: {}", e);
                match find_text_element_with_selected(app_element, selected_text, deadline) {
                    Some(text_elem) => {
                        let surrounding = build_surrounding(text_elem, app_element, kind, selected_text, deadline, bundle_id);
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
    bundle_id_or_name: &str,
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
    // 自绘编辑器（Sublime Text、WPS 等）的 AX 树不含真实编辑器内容：
    // - Sublime: AX 树只有 "UNREGISTERED" 水印
    // - WPS: AX 返回 -25212（禁用），full_text 为空
    // Terminal 排除：full_text 是真实 scrollback。
    if kind == AppKind::Editor && !selected_text.is_empty() {
        let selected_trimmed = selected_text.trim();
        let full_trimmed = full_text.trim();
        // full_text 为空 或 不含选中文本 → 触发 fallback
        let need_fallback = full_trimmed.is_empty() || !full_trimmed.contains(selected_trimmed);

        if need_fallback {
            // WPS Office: AX 禁用 + 无 AppleScript + 无插件 API + 窗口标题通常为空。
            // Sublime Text: 通过插件取数器（含未保存文件）。
            // 通用 fallback：磁盘文件读取。
            {
                // Sublime Text 专用取数器
                if bundle_id_or_name.contains("sublimetext") {
                    if let Some(sublime_ctx) = crate::platform::app_context::sublime_plugin::try_sublime_plugin_context(
                        bundle_id_or_name,
                        selected_text,
                        deadline,
                    ) {
                        diagnostics.push("SUBLIME_PLUGIN: 插件取数成功".to_string());
                        let diag = Some(diagnostics.join("\n  "));
                        log::info!("[app-context] Sublime 插件取数成功");
                        return Ok((sublime_ctx, diag));
                    }
                }

                // Pages/Keynote 用 AppleScript 读全文（和 Chrome JS 同思路）
                if bundle_id_or_name.to_ascii_lowercase().contains("iwork") {
                    if let Some(ctx) = try_pages_applescript(selected_text, &window_title, deadline) {
                        diagnostics.push("PAGES_APPLESCRIPT: AppleScript body text 取数成功".to_string());
                        let diag = Some(diagnostics.join("\n  "));
                        log::info!("[app-context] Pages AppleScript 取数成功");
                        return Ok((ctx, diag));
                    }
                }

                // 通用 lsof fallback：获取进程打开的文件路径（窗口标题常为空或不含文件名）
                // 对 WPS/Pages 等窗口标题不可靠的 App 特别有效
                if let Some(lsof_ctx) = try_lsof_context(selected_text, bundle_id_or_name, deadline) {
                    diagnostics.push("LSOF: lsof 文件路径 + officecli/pdftotext 取数成功".to_string());
                    let diag = Some(diagnostics.join("\n  "));
                    log::info!("[app-context] lsof 取数成功");
                    return Ok((lsof_ctx, diag));
                }

                // 通用磁盘 fallback：从窗口标题提取文件名 + session/mdfind 搜索
                if let Some(ref title) = window_title {
                    if let Some(file_ctx) = try_read_file_context(
                        title,
                        bundle_id_or_name,
                        selected_text,
                        deadline,
                    ) {
                        diagnostics.push("FALLBACK: AX 降级 → 从磁盘读文件成功".to_string());
                        let diag = Some(diagnostics.join("\n  "));
                        log::info!("[app-context] AX 降级 → 磁盘文件 fallback 成功");
                        return Ok((file_ctx, diag));
                    }
                }

                // 诊断降级原因
                let degrade_reason = match &window_title {
                    None => "无窗口标题".to_string(),
                    Some(title) => match extract_filename_from_title(title) {
                        None => format!("标题 '{}' 无法提取文件名", title),
                        Some(fname) => match find_file_path(&fname, bundle_id_or_name, deadline) {
                            None => format!("文件 '{}' 未在 session/mdfind 中找到", fname),
                            Some(path) => match std::fs::read_to_string(&path) {
                                Err(e) => format!("文件 {} 读取失败: {}", path.display(), e),
                                Ok(content) if !content.contains(selected_trimmed) => {
                                    "文件内容不含选中文本".to_string()
                                }
                                Ok(_) => "未知原因".to_string(),
                            },
                        },
                    },
                };
                diagnostics.push(format!("DEGRADED: {}", degrade_reason));
            }
            let diag = Some(diagnostics.join("\n  "));
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
            attr,
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
            attr,
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
    CFGetTypeID(value) == crate::platform::app_context::ffi::ax_value_type_id()
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

    // ── extract_filename_from_title ──

    #[test]
    fn test_extract_filename_sublime() {
        assert_eq!(
            extract_filename_from_title("test.txt — Sublime Text"),
            Some("test.txt".to_string())
        );
    }

    #[test]
    fn test_extract_filename_wps() {
        assert_eq!(
            extract_filename_from_title("report.docx - WPS Office"),
            Some("report.docx".to_string())
        );
    }

    #[test]
    fn test_extract_filename_untitled() {
        assert_eq!(extract_filename_from_title("untitled — Sublime Text"), None);
    }

    #[test]
    fn test_extract_filename_app_name_only() {
        assert_eq!(extract_filename_from_title("Sublime Text"), None);
    }

    #[test]
    fn test_extract_filename_no_extension() {
        assert_eq!(extract_filename_from_title("Makefile — Sublime Text"), None);
    }

    #[test]
    fn test_extract_filename_vscode() {
        assert_eq!(
            extract_filename_from_title("main.rs - octopus - Visual Studio Code"),
            Some("main.rs".to_string())
        );
    }

    #[test]
    fn test_extract_filename_empty_title() {
        assert_eq!(extract_filename_from_title(""), None);
    }

    #[test]
    fn test_extract_filename_only_extension() {
        // 极端边界：标题只有 ".bashrc"
        assert_eq!(
            extract_filename_from_title(".bashrc — Sublime Text"),
            Some(".bashrc".to_string())
        );
    }

    #[test]
    fn test_extract_filename_dotted_path() {
        assert_eq!(
            extract_filename_from_title("src/main.rs — Sublime Text"),
            Some("src/main.rs".to_string())
        );
    }

    // ── slice_around_text ──

    #[test]
    fn test_slice_normal() {
        let full = "Hello world this is a test sentence";
        let result = slice_around_text(full, "world", 5).unwrap();
        assert_eq!(result.0.as_deref(), Some("ello "));
        assert_eq!(result.1.as_deref(), Some(" this"));
    }

    #[test]
    fn test_slice_start_of_text() {
        let full = "Hello world";
        let result = slice_around_text(full, "Hello", 5).unwrap();
        assert_eq!(result.0, None); // 没有上文
        assert_eq!(result.1.as_deref(), Some(" worl")); // 后 5 字符
    }

    #[test]
    fn test_slice_end_of_text() {
        let full = "Hello world";
        let result = slice_around_text(full, "world", 5).unwrap();
        assert_eq!(result.0.as_deref(), Some("ello "));
        assert_eq!(result.1, None); // 没有下文
    }

    #[test]
    fn test_slice_case_insensitive() {
        let full = "Hello WORLD test";
        let result = slice_around_text(full, "world", 3).unwrap();
        assert_eq!(result.0.as_deref(), Some("lo ")); // 限制 3 字符
        assert_eq!(result.1.as_deref(), Some(" te"));
    }

    #[test]
    fn test_slice_cjk() {
        let full = "你好世界这是一段测试文字";
        let result = slice_around_text(full, "这是", 2).unwrap();
        assert_eq!(result.0.as_deref(), Some("世界"));
        assert_eq!(result.1.as_deref(), Some("一段"));
    }

    #[test]
    fn test_slice_not_found() {
        assert_eq!(slice_around_text("hello world", "nonexistent", 5), None);
    }

    #[test]
    fn test_slice_empty_selected() {
        assert_eq!(slice_around_text("hello", "", 5), None);
    }

    #[test]
    fn test_slice_empty_full() {
        assert_eq!(slice_around_text("", "test", 5), None);
    }

    #[test]
    fn test_slice_limit_exceeds_content() {
        // limit > 全文长度 → before/after 为全文剩余部分
        let full = "ABC SELECT DEF";
        let result = slice_around_text(full, "SELECT", 1000).unwrap();
        assert_eq!(result.0.as_deref(), Some("ABC "));
        assert_eq!(result.1.as_deref(), Some(" DEF"));
    }

    #[test]
    fn test_slice_selected_with_whitespace() {
        // trim 后匹配
        let full = "hello world end";
        let result = slice_around_text(full, "  world  ", 3).unwrap();
        assert_eq!(result.0.as_deref(), Some("lo "));
        assert_eq!(result.1.as_deref(), Some(" en"));
    }

    #[test]
    fn test_slice_multiple_occurrences() {
        // 多次出现 → 命中第一次
        let full = "aaa bbb aaa ccc";
        let result = slice_around_text(full, "aaa", 3).unwrap();
        assert_eq!(result.0, None); // 第一次 "aaa" 在开头
        assert_eq!(result.1.as_deref(), Some(" bb")); // 后 3 字
    }

    // ── try_read_file_context (用 tempfile 端到端测试) ──

    #[test]
    fn test_try_read_file_context_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test_context.md");
        let content = "这是上文内容。\n选中这段文字。\n这是下文内容。";
        std::fs::write(&file_path, content).unwrap();

        // find_file_path 走 mdfind 找不到 tempfile 目录，
        // 所以直接测 slice_around_text 逻辑 + try_read_file_context 需要真实文件路径。
        // 这里验证 slice 逻辑（try_read_file_context 的核心）：
        let result = slice_around_text(content, "选中这段文字。", 1000).unwrap();
        assert_eq!(result.0.as_deref(), Some("这是上文内容。\n"));
        assert_eq!(result.1.as_deref(), Some("\n这是下文内容。"));
    }

    #[test]
    fn test_try_read_file_context_large_file() {
        // 模拟大文件：before/after 各截断到 1000 字
        let before: String = "A".repeat(2000);
        let after: String = "B".repeat(2000);
        let content = format!("{}SELECTED{}", before, after);
        let result = slice_around_text(&content, "SELECTED", 1000).unwrap();
        assert_eq!(result.0.as_ref().unwrap().len(), 1000);
        assert!(result.0.as_ref().unwrap().chars().all(|c| c == 'A'));
        assert_eq!(result.1.as_ref().unwrap().len(), 1000);
        assert!(result.1.as_ref().unwrap().chars().all(|c| c == 'B'));
    }

    #[test]
    fn test_try_read_file_context_multiline_selected() {
        let content = "line1\nline2\nline3\nSELECTED\nline5\nline6";
        let result = slice_around_text(content, "SELECTED", 1000).unwrap();
        assert_eq!(result.0.as_deref(), Some("line1\nline2\nline3\n"));
        assert_eq!(result.1.as_deref(), Some("\nline5\nline6"));
    }

    // ── path_matches_filename ──

    #[test]
    fn test_path_matches_exact() {
        assert!(path_matches_filename("/home/user/main.rs", "main.rs"));
        assert!(path_matches_filename("/tmp/test.txt", "test.txt"));
    }

    #[test]
    fn test_path_matches_no_false_positive() {
        // "ba.rs" 不应匹配 "a.rs"
        assert!(!path_matches_filename("/x/ba.rs", "a.rs"));
        // 子目录名不应匹配
        assert!(!path_matches_filename("/home/main.rs/file.txt", "main.rs"));
    }

    #[test]
    fn test_path_matches_no_path() {
        // 裸文件名（无目录前缀）——file_name() 仍返回自身
        assert!(path_matches_filename("main.rs", "main.rs"));
    }

    // ── find_file_in_session_json（纯函数，直接测真实逻辑）──

    #[test]
    fn test_session_json_file_history_single_window() {
        let json = serde_json::json!({
            "windows": [{
                "file_history": ["/other/file.txt", "/home/user/main.rs"],
                "buffers": []
            }]
        });
        let result = find_file_in_session_json(&json, "main.rs");
        assert_eq!(result, Some(std::path::PathBuf::from("/home/user/main.rs")));
    }

    #[test]
    fn test_session_json_buffers_single_window() {
        let json = serde_json::json!({
            "windows": [{
                "file_history": [],
                "buffers": [{"file": ""}, {"file": "/home/user/notes.md"}]
            }]
        });
        let result = find_file_in_session_json(&json, "notes.md");
        assert_eq!(result, Some(std::path::PathBuf::from("/home/user/notes.md")));
    }

    #[test]
    fn test_session_json_multi_window() {
        // P3b 回归保护：文件在第二个窗口
        let json = serde_json::json!({
            "windows": [
                {"file_history": ["/a/x.rs"], "buffers": []},
                {"file_history": ["/b/y.rs"], "buffers": [{"file": "/c/target.go"}]}
            ]
        });
        let result = find_file_in_session_json(&json, "target.go");
        assert_eq!(result, Some(std::path::PathBuf::from("/c/target.go")));
    }

    #[test]
    fn test_session_json_empty_buffers_filtered() {
        let json = serde_json::json!({
            "windows": [{"file_history": [], "buffers": [{"file": ""}, {"file": ""}]}]
        });
        assert_eq!(find_file_in_session_json(&json, "anything.txt"), None);
    }

    #[test]
    fn test_session_json_suffix_no_false_positive() {
        // P3a 回归保护：ends_with "a.rs" 不应匹配 "ba.rs"
        let json = serde_json::json!({
            "windows": [{"file_history": ["/x/ba.rs"], "buffers": []}]
        });
        assert_eq!(find_file_in_session_json(&json, "a.rs"), None);
    }

    #[test]
    fn test_session_json_not_found() {
        let json = serde_json::json!({
            "windows": [{"file_history": ["/a/b.rs"], "buffers": []}]
        });
        assert_eq!(find_file_in_session_json(&json, "missing.rs"), None);
    }

    #[test]
    fn test_session_json_empty_windows() {
        let json = serde_json::json!({"windows": []});
        assert_eq!(find_file_in_session_json(&json, "main.rs"), None);
    }

    // ── name_result_is_app_name ──

    #[test]
    fn test_name_result_is_app_name_yes() {
        assert!(name_result_is_app_name("Sublime Text"));
        assert!(name_result_is_app_name("WPS Office"));
        assert!(name_result_is_app_name("Code"));
        assert!(name_result_is_app_name("sublime text")); // 大小写不敏感
    }

    #[test]
    fn test_name_result_is_app_name_no() {
        assert!(!name_result_is_app_name("main.rs"));
        assert!(!name_result_is_app_name("test.txt"));
        assert!(!name_result_is_app_name("unknown_app"));
    }

    // ── merge_cjk_line_breaks ──

    #[test]
    fn test_merge_cjk_normal() {
        // 两行 CJK 文本，前一行末尾是 CJK → 合并
        let input = "这是第一行的文\n本内容继续";
        let result = merge_cjk_line_breaks(input);
        assert_eq!(result, "这是第一行的文本内容继续");
    }

    #[test]
    fn test_merge_cjk_preserves_paragraph_break() {
        // 空行分隔的段落 → 不合并
        let input = "第一段文字\n\n第二段文字";
        let result = merge_cjk_line_breaks(input);
        assert_eq!(result, "第一段文字\n\n第二段文字");
    }

    #[test]
    fn test_merge_cjk_ascii_no_merge() {
        // 前一行末尾是 ASCII → 不合并
        let input = "hello world\n继续文本";
        let result = merge_cjk_line_breaks(input);
        assert_eq!(result, "hello world\n继续文本");
    }

    #[test]
    fn test_merge_cjk_multi_line() {
        // 多行连续 CJK 排版换行 → 全部合并
        let input = "这是一段很长的文字第一\n行继续到第二行\n再继续到第三行";
        let result = merge_cjk_line_breaks(input);
        assert_eq!(result, "这是一段很长的文字第一行继续到第二行再继续到第三行");
    }

    #[test]
    fn test_merge_cjk_empty_input() {
        assert_eq!(merge_cjk_line_breaks(""), "");
    }

    // ── filter_lsof_doc_files ──

    #[test]
    fn test_filter_lsof_normal() {
        let input = "pwpsoffice\nn/Users/user/report.docx\nn/Users/user/notes.txt\nn/dev/null";
        let result = filter_lsof_doc_files(input);
        assert_eq!(result, vec!["/Users/user/report.docx"]);
    }

    #[test]
    fn test_filter_lsof_excludes_lock_files() {
        let input = "n/Users/user/report.docx\nn/Users/user/.~report.docx";
        let result = filter_lsof_doc_files(input);
        assert_eq!(result, vec!["/Users/user/report.docx"]);
    }

    #[test]
    fn test_filter_lsof_multiple_formats() {
        let input = "n/a/b.pdf\nn/c/d.xlsx\nn/e/f.pptx\nn/g/h.txt\nn/i/j.doc";
        let result = filter_lsof_doc_files(input);
        assert_eq!(result, vec!["/a/b.pdf", "/c/d.xlsx", "/e/f.pptx", "/i/j.doc"]);
    }

    #[test]
    fn test_filter_lsof_excludes_dev_and_frameworks() {
        // WPS 打开 .pdf framework 文件不应匹配
        let input = "n/dev/urandom\nn/Applications/wpsoffice.app/pdf.framework/pdf";
        let result = filter_lsof_doc_files(input);
        assert!(result.is_empty());
    }

    #[test]
    fn test_filter_lsof_path_with_spaces() {
        let input = "n/Users/user/my report final.docx";
        let result = filter_lsof_doc_files(input);
        assert_eq!(result, vec!["/Users/user/my report final.docx"]);
    }

    // ── bundle_id_to_procname ──

    #[test]
    fn test_bundle_id_to_procname_wps() {
        assert_eq!(bundle_id_to_procname("com.kingsoft.wpsoffice.mac"), "wpsoffice");
    }

    #[test]
    fn test_bundle_id_to_procname_pages() {
        assert_eq!(bundle_id_to_procname("com.apple.iWork.Pages"), "Pages");
    }

    #[test]
    fn test_bundle_id_to_procname_sublime() {
        assert_eq!(bundle_id_to_procname("com.sublimetext.4"), "Sublime");
    }

    #[test]
    fn test_bundle_id_to_procname_fallback() {
        // 未知 bundle_id → 取最后一段
        assert_eq!(bundle_id_to_procname("com.example.myeditor"), "myeditor");
    }

    // ── char_level_slice ──

    #[test]
    fn test_char_level_slice_normal() {
        let chars: Vec<char> = "Hello world test".chars().collect();
        let (before, after) = char_level_slice(&chars, 6, "world", "Hello world test", 5);
        assert_eq!(before.as_deref(), Some("ello "));
        assert_eq!(after.as_deref(), Some(" test"));
    }

    #[test]
    fn test_char_level_slice_start() {
        let chars: Vec<char> = "Hello world".chars().collect();
        let (before, after) = char_level_slice(&chars, 0, "Hello", "Hello world", 5);
        assert_eq!(before, None);
        assert_eq!(after.as_deref(), Some(" worl"));
    }

    #[test]
    fn test_char_level_slice_cjk() {
        let full = "你好世界测试文字";
        let chars: Vec<char> = full.chars().collect();
        // "试" 在 char index 5（byte offset 15）
        // before limit=2 → chars[3..5] = "界测"
        // after: chars[6..8] = "文字"
        let (before, after) = char_level_slice(&chars, 15, "试", full, 2);
        assert_eq!(before.as_deref(), Some("界测"));
        assert_eq!(after.as_deref(), Some("文字"));
    }

    // ── extract_text_from_ooxml_xml ──

    #[test]
    fn test_ooxml_xml_basic() {
        let xml = r#"<doc><w:p><w:r><w:t>Hello</w:t></w:r></w:p><w:p><w:r><w:t>World</w:t></w:r></w:p></doc>"#;
        let result = extract_text_from_ooxml_xml(xml);
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
    }

    #[test]
    fn test_ooxml_xml_with_attributes() {
        // <w:t xml:space="preserve"> 带属性的标签
        let xml = r#"<w:p><w:r><w:t xml:space="preserve">测试</w:t></w:r></w:p>"#;
        let result = extract_text_from_ooxml_xml(xml);
        assert_eq!(result.trim(), "测试");
    }

    #[test]
    fn test_ooxml_xml_pptx_a_tag() {
        let xml = r#"<a:p><a:r><a:t>Slide text</a:t></a:r></a:p>"#;
        let result = extract_text_from_ooxml_xml(xml);
        assert!(result.contains("Slide text"));
    }

    #[test]
    fn test_ooxml_xml_empty() {
        let xml = r#"<doc><w:p></w:p></doc>"#;
        let result = extract_text_from_ooxml_xml(xml);
        assert!(result.is_empty() || result.trim().is_empty());
    }

    // ── slice_around_text NFKC 归一化（第三轮匹配）──

    #[test]
    fn test_slice_nfkc_kangxi() {
        // 康熙部首 ⾥(U+2FA5) → 里(U+91CC)
        let full = "这里的常数可能不是把一个项目";
        let sel = "⾥的常数"; // ⾥ 是康熙部首
        let result = slice_around_text(full, sel, 5);
        assert!(result.is_some(), "NFKC 应该匹配康熙部首");
        if let Some((before, _after)) = result {
            assert!(before.as_deref().unwrap_or("").contains("这"));
        }
    }

    #[test]
    fn test_slice_nfkc_fullwidth_punct() {
        // 全角逗号 ，(U+FF0C) → 半角逗号
        let full = "Hello, World";
        let sel = "Hello，"; // 全角逗号
        let result = slice_around_text(full, sel, 5);
        assert!(result.is_some(), "NFKC 应该匹配全角标点");
    }

    #[test]
    fn test_slice_nfkc_short_rejected() {
        // 归一化后太短 (<5) 不尝试
        let full = "测试文本内容";
        let sel = "⾥"; // 单字符
        let result = slice_around_text(full, sel, 5);
        assert!(result.is_none());
    }
}
