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

use core_foundation::base::{CFGetTypeID, CFRelease, CFTypeRef, TCFType};
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

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

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
}
