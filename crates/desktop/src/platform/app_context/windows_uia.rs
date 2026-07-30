//! Windows UIAutomation 实现。
//!
//! 取数路径：
//! 1. GetForegroundWindow → GetWindowThreadProcessId → pid
//! 2. K32GetModuleFileNameEx / QueryFullProcessImageNameW → exe 名
//! 3. classify_app(exe_name) → AppKind
//! 4. CoCreateInstance(CUIAutomation) → IUIAutomation
//! 5. GetFocusedElement → 焦点元素
//! 6. GetCurrentPattern(TextPattern) → GetSelection → GetText 切前后文
//!
//! 权限：UIAutomation 通常无需特殊权限（Low IL 进程即可读）。

#![cfg(target_os = "windows")]

use std::time::{Duration, Instant};

use super::*;

const UIA_TIMEOUT: Duration = Duration::from_millis(500);
const SURROUNDING_LIMIT: usize = 1000;
const TERMINAL_MAX_LINES: usize = 30;
const TERMINAL_MAX_CHARS: usize = 1000;

pub struct UiaProvider;

impl super::ContextProvider for UiaProvider {
    fn gather(&self, selected_text: &str) -> anyhow::Result<ExtraContext> {
        let deadline = Instant::now() + UIA_TIMEOUT;

        // 1. 前台窗口 + 进程信息
        let (_pid, exe_name) = frontmost_process().ok_or_else(|| anyhow::anyhow!("无法获取前台窗口进程"))?;
        let kind = classify_app(&exe_name);

        let source = AppSource {
            bundle_id: Some(exe_name.clone()),
            name: exe_name,
            kind,
        };

        // 2. 采集 surrounding
        let surrounding = if Instant::now() < deadline {
            match gather_surrounding(selected_text, kind, deadline) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::info!("[app-context] surrounding 采集失败（降级）: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(ExtraContext {
            source,
            surrounding,
            diagnostics: None,
        })
    }
}

/// 获取前台窗口的 (pid, exe_name)。
fn frontmost_process() -> Option<(u32, String)> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::Foundation::CloseHandle;

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }

        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, false, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);

        if ok.is_err() {
            return Some((pid, format!("pid:{}", pid)));
        }

        let exe_path = String::from_utf16_lossy(&buf[..len as usize]);
        let exe_name = std::path::Path::new(&exe_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("pid:{}", pid));

        Some((pid, exe_name))
    }
}

/// 通过 UIAutomation 采集选区周围文本。
fn gather_surrounding(selected_text: &str, kind: AppKind, deadline: Instant) -> anyhow::Result<SurroundingText> {
    use windows::Win32::System::Com::{
        CoInitializeEx, CoCreateInstance, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextPattern,
        UIA_TextPatternId, UIA_WindowControlTypeId,
    };
    use windows::core::Interface;

    unsafe {
        // 初始化 COM（忽略 S_FALSE = 已初始化）
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let uia: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;

        let focused = uia.GetFocusedElement()?;

        // 窗口标题——焦点元素的 Name 通常是控件 label 而非窗口标题，
        // 需向上找 Window 祖先的 Name
        let window_title = find_window_title(&uia, &focused);

        // 尝试 TextPattern 获取全文
        // Windows 暂用全文 find 定位（与 macOS Terminal 路径相同）。
        // TextPattern.GetSelection + ExpandToEnclosingUnit 段落级精准扩展留作 v2。
        let text_pattern: IUIAutomationTextPattern = focused
            .GetCurrentPattern(UIA_TextPatternId)
            .ok()
            .and_then(|p| p.cast().ok());

        let full_text = if let Some(ref tp) = text_pattern {
            let doc_range = tp.get_DocumentRange().ok();
            doc_range
                .and_then(|r| r.GetText(-1).ok())
                .map(|t| t.to_string())
                .unwrap_or_default()
        } else {
            // 无 TextPattern，尝试 ValuePattern
            use windows::Win32::UI::Accessibility::UIA_ValuePatternId;
            let value_pattern = focused.GetCurrentPattern(UIA_ValuePatternId).ok();
            value_pattern
                .and_then(|p| {
                    use windows::Win32::UI::Accessibility::IUIAutomationValuePattern;
                    let vp: IUIAutomationValuePattern = p.cast().ok()?;
                    vp.CurrentValue().ok().map(|v| v.to_string())
                })
                .unwrap_or_default()
        };

        return Ok(build_surrounding_from_text(
            &full_text,
            selected_text,
            kind,
            window_title,
            deadline,
        ));
    }
}

/// 从焦点元素向上找 Window 祖先取标题（焦点元素 Name 通常是控件 label，不是窗口标题）。
unsafe fn find_window_title(
    uia: &IUIAutomation,
    focused: &windows::Win32::UI::Accessibility::IUIAutomationElement,
) -> Option<String> {
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationCondition, UIA_WindowControlTypeId,
    };
    use windows::core::Interface;

    let cond = uia
        .CreatePropertyCondition(
            windows::Win32::UI::Accessibility::UIA_ControlTypePropertyId,
            &windows::core::VARIANT::from(UIA_WindowControlTypeId as i32),
        )
        .ok()?;

    // 简化：用 TreeWalker 向上找 Window
    let walker = uia.CreateTreeWalker(&cond).ok()?;
    let window_element = walker
        .NormalizeElement(focused)
        .ok()
        .or_else(|| Some(focused.clone().into()))?;

    let window_elem: IUIAutomationElement = window_element.cast().ok()?;
    window_elem.CurrentName().ok().map(|n| n.to_string())
}

/// 从全文 + selected_text 构建 SurroundingText（内容校验 + find 定位 + 截断）。
fn build_surrounding_from_text(
    full_text: &str,
    selected_text: &str,
    kind: AppKind,
    window_title: Option<String>,
    deadline: Instant,
) -> SurroundingText {
    if Instant::now() >= deadline {
        log::warn!("[app-context] Windows UIA 超时");
        return SurroundingText { before: None, after: None, window_title };
    }

    // 内容校验（Terminal 排除）
    let sel_trimmed = selected_text.trim();
    if kind != AppKind::Terminal
        && !full_text.is_empty()
        && !sel_trimmed.is_empty()
        && !full_text.contains(sel_trimmed)
    {
        return SurroundingText { before: None, after: None, window_title };
    }

    // 用 selected_text 在 full_text 中搜索定位切 before/after
    let (before, after) = if !full_text.is_empty() && !sel_trimmed.is_empty() {
        match full_text.find(sel_trimmed) {
            Some(pos) => {
                let before_text = &full_text[..pos];
                let after_start = pos + sel_trimmed.len();
                let after_text = if after_start < full_text.len() {
                    &full_text[after_start..]
                } else {
                    ""
                };
                let b = if kind == AppKind::Terminal {
                    truncate_terminal_tail(before_text)
                } else {
                    truncate_text_tail(before_text, 1000, SURROUNDING_LIMIT)
                };
                let a = truncate_text_head(after_text, SURROUNDING_LIMIT);
                (b, a)
            }
            None => {
                if kind == AppKind::Terminal {
                    (truncate_terminal_tail(full_text), None)
                } else {
                    (None, None)
                }
            }
        }
    } else {
        (None, None)
    };

    SurroundingText { before, after, window_title }
}

/// Terminal scrollback 截断（简化版，行数/字数限制）。
fn truncate_terminal_tail(s: &str) -> Option<String> {
    truncate_text_tail(s, TERMINAL_MAX_LINES, TERMINAL_MAX_CHARS)
}

/// 截取文本末尾：取最后 max_lines 行或 max_chars 字符。
fn truncate_text_tail(s: &str, max_lines: usize, max_chars: usize) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let lines: Vec<&str> = s.lines().collect();
    let start_line = lines.len().saturating_sub(max_lines);
    let by_lines: String = lines[start_line..].join("\n");

    if by_lines.chars().count() > max_chars {
        let char_start = by_lines.chars().count().saturating_sub(max_chars);
        Some(by_lines.chars().skip(char_start).collect())
    } else {
        Some(by_lines)
    }
}

/// 截取文本头部：取前 max_chars 字符。
fn truncate_text_head(s: &str, max_chars: usize) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        Some(s.to_string())
    } else {
        Some(chars[..max_chars].iter().collect())
    }
}
