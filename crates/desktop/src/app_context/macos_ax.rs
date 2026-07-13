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
        let (surrounding, diagnostics) = if deadline.checked_duration_since(Instant::now()).is_some() {
            match gather_surrounding(pid, kind, selected_text) {
                Ok((s, diag)) => (Some(s), diag),
                Err(e) => {
                    log::info!("[app-context] surrounding 采集失败（降级）: {}", e);
                    (None, Some(format!("gather_surrounding error: {}", e)))
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

/// 通过 AX 采集选区周围文本。返回 (surrounding, AX 诊断信息)。
fn gather_surrounding(pid: i32, kind: AppKind, selected_text: &str) -> anyhow::Result<(SurroundingText, Option<String>)> {
    unsafe {
        let app_element = AXUIElementCreateApplication(pid);
        if app_element.is_null() {
            return Err(anyhow::anyhow!("AXUIElementCreateApplication 返回 null"));
        }

        // 获取焦点元素
        let result = match get_attribute_value(app_element, &ax_focused_ui_element()) {
            Ok(focused_ref) => {
                let focused_element = focused_ref as AXUIElementRef;
                let surrounding = build_surrounding(focused_element, app_element, kind, selected_text);
                CFRelease(focused_ref);
                surrounding
            }
            Err(e) => {
                log::info!("[app-context] 无法获取焦点元素: {}", e);
                Err(e)
            }
        };

        CFRelease(app_element as CFTypeRef);
        result.map(|(s, diag)| (s, diag))
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
) -> anyhow::Result<(SurroundingText, Option<String>)> {
    let mut diagnostics: Vec<String> = Vec::new();

    // 焦点元素角色
    let focused_role = get_attribute_string(focused_element, &ax_role()).unwrap_or_default();
    diagnostics.push(format!("focused_role={}", focused_role));

    let (text_element, owns_text_element) = if is_text_element(&focused_role) {
        (focused_element, false)
    } else {
        match find_text_element(focused_element) {
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

/// 递归遍历 AX 子树，找到第一个有 AXValue 的文本元素（AXTextArea/AXTextField）。
/// 深度限制 5 层，广度限制每层 50 个子元素（防止巨树卡死）。
///
/// **返回的元素经过 CFRetain（+1）**，调用方必须 CFRelease。
/// 因为子元素从 CFArray 中取得，数组 Drop 后子元素会被释放——
/// 必须 CFRetain 防止 use-after-free。
unsafe fn find_text_element(element: AXUIElementRef) -> Option<AXUIElementRef> {
    find_text_element_depth(element, 0, 5)
}

unsafe fn find_text_element_depth(
    element: AXUIElementRef,
    depth: usize,
    max_depth: usize,
) -> Option<AXUIElementRef> {
    if depth >= max_depth {
        return None;
    }

    // 当前元素角色
    let role = get_attribute_string(element, &ax_role()).unwrap_or_default();
    if is_text_element(&role) {
        // 确认有 AXValue（必须释放，否则泄漏）
        let has_value = match get_attribute_value(element, &ax_value()) {
            Ok(v) => {
                CFRelease(v);
                true
            }
            Err(_) => false,
        };
        if has_value {
            // CFRetain 防止 use-after-free：
            // 如果 element 来自父层的 CFArray，数组 Drop 时会释放它。
            CFRetain(element as CFTypeRef);
            return Some(element);
        }
    }

    // 遍历子元素——AXChildren 可能返回非 CFArray（如 CFBoolean false），需类型检查
    let children = match get_attribute_value(element, &ax_children()) {
        Ok(v) => v,
        Err(_) => return None,
    };

    // 类型检查：AXChildren 必须是 CFArray，否则释放后返回 None
    if !is_cf_array(children) {
        CFRelease(children);
        return None;
    }

    let cf_array =
        core_foundation::array::CFArray::<CFTypeRef>::wrap_under_create_rule(children as *const _);
    let count = cf_array.len().min(50);

    for i in 0..count {
        let Some(child_ref) = cf_array.get(i) else {
            continue;
        };
        let child: AXUIElementRef = *child_ref as AXUIElementRef;
        if child.is_null() {
            continue;
        }
        if let Some(found) = find_text_element_depth(child, depth + 1, max_depth) {
            return Some(found); // found 已被 CFRetain，安全返回
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

/// 检查 CFTypeRef 是否为 CFArray 类型（null → false）。
/// AXChildren 在某些元素上返回 CFBoolean/CFNull 而非 CFArray，
/// 盲目 wrap 为 CFArray 会类型混淆崩溃（与 CFString 同类 bug）。
unsafe fn is_cf_array(value: CFTypeRef) -> bool {
    if value.is_null() {
        return false;
    }
    CFGetTypeID(value) == core_foundation::array::CFArray::<CFTypeRef>::type_id()
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
}
