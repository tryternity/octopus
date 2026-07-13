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

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::CFString;

use super::ffi::*;
use super::*;

/// AX 调用整体超时上限。
const AX_TIMEOUT: Duration = Duration::from_millis(500);
/// Editor before/after 截断字数。
const SURROUNDING_LIMIT: usize = 2000;
/// Terminal scrollback 最大行数。
const TERMINAL_MAX_LINES: usize = 50;
/// Terminal scrollback 最大字数。
const TERMINAL_MAX_CHARS: usize = 2000;

pub struct AxProvider;

impl super::ContextProvider for AxProvider {
    fn gather(&self) -> anyhow::Result<ExtraContext> {
        let deadline = Instant::now() + AX_TIMEOUT;

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
        let surrounding = if deadline.checked_duration_since(Instant::now()).is_some() {
            match gather_surrounding(pid, kind) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::info!("[app-context] surrounding 采集失败（降级）: {}", e);
                    None
                }
            }
        } else {
            log::warn!("[app-context] gather 超时（source 已获取，surrounding 跳过）");
            None
        };

        Ok(ExtraContext { source, surrounding })
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

/// 通过 AX 采集选区周围文本。
fn gather_surrounding(pid: i32, kind: AppKind) -> anyhow::Result<SurroundingText> {
    unsafe {
        let app_element = AXUIElementCreateApplication(pid);
        if app_element.is_null() {
            return Err(anyhow::anyhow!("AXUIElementCreateApplication 返回 null"));
        }

        // 获取焦点元素
        let result = match get_attribute_value(app_element, &ax_focused_ui_element()) {
            Ok(focused_ref) => {
                let focused_element = focused_ref as AXUIElementRef;
                let surrounding = build_surrounding(focused_element, app_element, kind);
                CFRelease(focused_ref);
                surrounding
            }
            Err(e) => {
                log::info!("[app-context] 无法获取焦点元素: {}", e);
                Err(e)
            }
        };

        CFRelease(app_element as CFTypeRef);
        result
    }
}

/// 从焦点元素构建 surrounding。
unsafe fn build_surrounding(
    focused_element: AXUIElementRef,
    app_element: AXUIElementRef,
    kind: AppKind,
) -> anyhow::Result<SurroundingText> {
    // 窗口标题：优先从焦点元素取，回退到 app element
    let window_title = get_attribute_string(focused_element, &ax_title())
        .or_else(|_| get_attribute_string(app_element, &ax_title()))
        .ok();

    // 全文
    let full_text = get_attribute_string(focused_element, &ax_value()).unwrap_or_default();

    // 选区范围
    let range =
        get_selected_range(focused_element).unwrap_or(TextRange { start: 0, end: 0 });

    let mut surrounding = if kind == AppKind::Terminal {
        // Terminal 特例：before 取 scrollback 截断
        let before = if !full_text.is_empty() {
            Some(truncate_terminal_scrollback(
                &full_text,
                range.start,
                TERMINAL_MAX_LINES,
                TERMINAL_MAX_CHARS,
            ))
        } else {
            None
        };
        SurroundingText {
            before,
            after: None,
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

    Ok(surrounding)
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
            "AXUIElementCopyAttributeValue error: {}",
            err
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

    // CFString 的 type id（进程内缓存）
    let string_type_id = core_foundation::string::CFString::type_id();

    if core_foundation::base::CFGetTypeID(value) != string_type_id {
        CFRelease(value);
        return Err(anyhow::anyhow!(
            "AX 属性 {} 返回非 CFString 类型",
            attr.to_string()
        ));
    }

    let cf_string = CFString::wrap_under_create_rule(value as *const _);
    Ok(cf_string.to_string())
}

/// 读取选区范围 (start, end)，单位为 UTF-16 字符偏移（AX 的 CFRange 单位）。
unsafe fn get_selected_range(element: AXUIElementRef) -> anyhow::Result<TextRange> {
    let value = get_attribute_value(element, &ax_selected_text_range())?;
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
