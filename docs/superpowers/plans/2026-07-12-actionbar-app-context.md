# Action Bar 应用上下文获取 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 action bar 触发时通过 macOS Accessibility API 获取选中文本的来源应用与前后文，让 LLM 动作具备情境感知。

**Architecture:** 新建 `app_context/` 模块，定义平台无关 `ContextProvider` trait + 纯函数辅助。macOS 实现走 NSWorkspace（前台 App 信息）+ AXUIElement C FFI（AX 树读取）。`trigger_action_bar` 拿到选中文本后调 `gather()` 采集上下文，拼入 `ActionBarContext`。`execute_action_bar_inner` 的 AI 分支读取上下文构建增强 prompt。失败降级到「仅 text」零回归。

**Tech Stack:** Rust, macOS Accessibility (AXUIElement / ApplicationServices), objc2-app-kit (NSWorkspace), core-foundation 0.10, Tauri 2, React/TypeScript 前端。

**Spec:** [`2026-07-12-actionbar-app-context-design.md`](../specs/2026-07-12-actionbar-app-context-design.md)

## Global Constraints

- **平台**：MVP 仅 macOS 实现；Windows/Linux 为 trait stub（返回 None/Err），不阻塞编译
- **降级铁律**：上下文获取任何失败都不得阻塞浮窗显示——`gather()` 失败只记日志，`ActionBarContext` 仅含 `text`
- **权限**：无新增——复用现有辅助功能权限（模拟 Cmd+C 已需）
- **Terminal scrollback**：`before` 从选区起点向前取，以 50 行或 2000 字先达到者为准
- **Editor before/after**：各默认 2000 字截断
- **AX 超时**：整体 500ms 上限，超时返回已采集的部分字段
- **测试命令**：`cargo test -p octopus-desktop`（单元测试内联在源文件 `#[cfg(test)] mod tests`）
- **语言**：中文注释，与代码库一致
- **坐标**：本项目不涉及坐标转换（AX 返回的是文本，不是坐标）

---

## File Structure

```
crates/desktop/src/
├── app_context/
│   ├── mod.rs            # 类型定义 + trait + provider() 工厂 + 纯函数 + NullProvider + 单元测试
│   ├── macos_ax.rs       # macOS AX 实现（cfg-gated）
│   └── ffi.rs            # AXUIElement C FFI 声明（cfg-gated，macOS only）
├── action_bar_commands.rs  # 修改：ActionBarContext 加字段 + trigger 调 gather + execute 注入 prompt
├── lib.rs                  # 修改：加 `mod app_context;`
└── main.rs                 # 不改（命令注册不变）

crates/desktop/Cargo.toml   # 修改：加 objc2-app-kit (macOS only)
crates/desktop/frontend/src/pages/ActionBar/index.tsx  # 修改：Context 类型 + 来源标签
```

---

## Task 1: app_context 模块骨架——类型 + trait + 纯函数 + 单元测试

**Files:**
- Create: `crates/desktop/src/app_context/mod.rs`
- Modify: `crates/desktop/src/lib.rs`（加 `mod app_context;`）

**Interfaces:**
- Produces: `AppSource`, `AppKind`, `SurroundingText`, `ExtraContext`, `ContextProvider` trait, `NullProvider`, `provider()` 工厂, `classify_app(&str) -> AppKind`, `extract_surrounding(&str, CFRange-like, usize) -> SurroundingText`（后续 Task 依赖这些类型）

- [ ] **Step 1: 创建 app_context/mod.rs——类型 + trait + 纯函数**

```rust
//! Action Bar 应用上下文获取——平台无关的类型定义、trait、纯函数辅助。
//!
//! 各 OS 的实现在子模块（macos_ax.rs 等），通过 `provider()` 工厂 + cfg 分发。

/// 应用语义类别。决定前端/LLM 如何利用上下文。
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    Editor,
    Terminal,
    Browser,
    Chat,
    Unknown,
}

/// 选中文本所在的应用。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    pub name: String,
    pub kind: AppKind,
}

/// 选中文本的周围文本。
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurroundingText {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
}

/// gather 采集到的额外上下文。
pub struct ExtraContext {
    pub source: AppSource,
    pub surrounding: Option<SurroundingText>,
}

/// 平台无关的应用上下文获取接口。
pub trait ContextProvider {
    /// 至少返回 source（前台 app 信息）；surrounding 可能 None。
    fn gather(&self) -> anyhow::Result<ExtraContext>;
}

/// 非 macOS 平台的空实现——永远返回 Err。
pub struct NullProvider;

impl ContextProvider for NullProvider {
    fn gather(&self) -> anyhow::Result<ExtraContext> {
        Err(anyhow::anyhow!("app context: platform not supported"))
    }
}

/// 工厂函数——cfg 分发到各平台实现。
pub fn provider() -> Box<dyn ContextProvider> {
    #[cfg(target_os = "macos")]
    {
        Box::new(self::macos_ax::AxProvider)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NullProvider)
    }
}

// ── 纯函数辅助 ──

/// bundle id → AppKind 映射。
pub fn classify_app(bundle_id: &str) -> AppKind {
    match bundle_id {
        "com.apple.Terminal" | "com.googlecode.iterm2" => AppKind::Terminal,
        "com.microsoft.Word"
        | "com.apple.TextEdit"
        | "com.sublimetext.4"
        | "com.sublimetext.3"
        | "com.microsoft.VSCode"
        | "com.todesktop.230313mzl4w4u92" // Cursor
        | "com.github.atom" => AppKind::Editor,
        "com.apple.Safari"
        | "com.google.Chrome"
        | "org.mozilla.firefox"
        | "com.microsoft.edgemac" => AppKind::Browser,
        "com.tencent.xinWeChat"
        | "com.tinyspeck.slackmacgap" // Slack
        | "com.hnc.Discord" => AppKind::Chat,
        _ => AppKind::Unknown,
    }
}

/// 选区范围（start..end，字符偏移）。
#[derive(Clone, Copy, Debug)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

/// 从全文和选区范围切出 before/after，各裁剪到 limit 字。
pub fn extract_surrounding(full_text: &str, range: TextRange, limit: usize) -> SurroundingText {
    let chars: Vec<char> = full_text.chars().collect();
    let total = chars.len();
    let start = range.start.min(total);
    let end = range.end.min(total);

    // before: 选区起点向前取 limit 字
    let before_start = start.saturating_sub(limit);
    let before: String = chars[before_start..start].iter().collect();
    // after: 选区终点向后取 limit 字
    let after_end = (end + limit).min(total);
    let after: String = chars[end..after_end].iter().collect();

    SurroundingText {
        before: if before.is_empty() { None } else { Some(before) },
        after: if after.is_empty() { None } else { Some(after) },
        window_title: None, // 由 AxProvider 填充
    }
}

/// Terminal scrollback 截断：从选区起点向前取，以 max_lines 或 max_chars 先达到者为准。
pub fn truncate_terminal_scrollback(scrollback: &str, selection_start: usize, max_lines: usize, max_chars: usize) -> String {
    let chars: Vec<char> = scrollback.chars().collect();
    let start = selection_start.min(chars.len());
    let before_part: String = chars[..start].iter().collect();

    // 按行截断
    let lines: Vec<&str> = before_part.lines().collect();
    let start_line = lines.len().saturating_sub(max_lines);
    let by_lines: String = lines[start_line..].join("\n");

    // 按字截断
    if by_lines.chars().count() > max_chars {
        let char_start = by_lines.chars().count().saturating_sub(max_chars);
        by_lines.chars().skip(char_start).collect()
    } else {
        by_lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_terminal() {
        assert_eq!(classify_app("com.apple.Terminal"), AppKind::Terminal);
        assert_eq!(classify_app("com.googlecode.iterm2"), AppKind::Terminal);
    }

    #[test]
    fn test_classify_editor() {
        assert_eq!(classify_app("com.microsoft.Word"), AppKind::Editor);
        assert_eq!(classify_app("com.apple.TextEdit"), AppKind::Editor);
        assert_eq!(classify_app("com.microsoft.VSCode"), AppKind::Editor);
    }

    #[test]
    fn test_classify_unknown() {
        assert_eq!(classify_app("com.some.unknown.app"), AppKind::Unknown);
        assert_eq!(classify_app(""), AppKind::Unknown);
    }

    #[test]
    fn test_extract_surrounding_normal() {
        let full = "Hello world this is a test sentence";
        let range = TextRange { start: 6, end: 11 }; // "world"
        let s = extract_surrounding(full, range, 5);
        assert_eq!(s.before.as_deref(), Some("Hello"));
        assert_eq!(s.after.as_deref(), Some(" this"));
    }

    #[test]
    fn test_extract_surrounding_start_of_text() {
        let full = "Hello world";
        let range = TextRange { start: 0, end: 5 }; // "Hello"
        let s = extract_surrounding(full, range, 100);
        assert_eq!(s.before, None); // 没有上文
        assert_eq!(s.after.as_deref(), Some(" world"));
    }

    #[test]
    fn test_extract_surrounding_cjk() {
        let full = "你好世界这是一段测试文字";
        let range = TextRange { start: 4, end: 6 }; // "这是"
        let s = extract_surrounding(full, range, 2);
        assert_eq!(s.before.as_deref(), Some("世界"));
        assert_eq!(s.after.as_deref(), Some("一段"));
    }

    #[test]
    fn test_truncate_terminal_by_lines() {
        let scrollback = "line1\nline2\nline3\nline4\nline5\nselected";
        // "selected" starts at char 35
        let result = truncate_terminal_scrollback(scrollback, 35, 2, 10000);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "line4");
        assert_eq!(lines[1], "line5");
    }

    #[test]
    fn test_truncate_terminal_by_chars() {
        let scrollback = "abcdefghijklmnopqrstuvwxyz selected";
        let result = truncate_terminal_scrollback(scrollback, 26, 10000, 5);
        assert_eq!(result.chars().count(), 5);
        // 最后 5 个字符是 "vwxyz"
        assert_eq!(result, "vwxyz");
    }

    #[test]
    fn test_null_provider_returns_err() {
        let p = NullProvider;
        assert!(p.gather().is_err());
    }
}
```

- [ ] **Step 2: 在 lib.rs 注册模块**

在 `crates/desktop/src/lib.rs` 的 `mod` 声明区（`mod action_bar_commands;` 附近）加：

```rust
mod app_context;
```

- [ ] **Step 3: 编译验证**

Run: `cargo build -p octopus-desktop 2>&1 | head -30`
Expected: 编译通过（macos_ax 模块被 `provider()` 引用但尚未创建——需先加空 stub）

在 `app_context/mod.rs` 末尾（tests 之前）加 macOS stub 以保证编译：

```rust
#[cfg(target_os = "macos")]
mod macos_ax;
```

创建 `crates/desktop/src/app_context/macos_ax.rs` 空骨架：

```rust
//! macOS Accessibility 实现（Task 2 填充）。

pub struct AxProvider;

impl super::ContextProvider for AxProvider {
    fn gather(&self) -> anyhow::Result<super::ExtraContext> {
        Err(anyhow::anyhow!("not yet implemented"))
    }
}
```

Run: `cargo build -p octopus-desktop 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 4: 运行单元测试**

Run: `cargo test -p octopus-desktop --lib app_context 2>&1`
Expected: 8 个测试全部 PASS

- [ ] **Step 5: 提交**

```bash
git add crates/desktop/src/app_context/ crates/desktop/src/lib.rs
git commit -m "feat(app-context): 类型定义 + ContextProvider trait + 纯函数辅助 + 单元测试"
```

---

## Task 2: macOS AX 实现——FFI 声明 + AxProvider

**Files:**
- Create: `crates/desktop/src/app_context/ffi.rs`
- Modify: `crates/desktop/src/app_context/macos_ax.rs`（填充实现）
- Modify: `crates/desktop/src/app_context/mod.rs`（加 `mod ffi;`）
- Modify: `crates/desktop/Cargo.toml`（加 `objc2-app-kit` macOS 依赖）

**Interfaces:**
- Consumes: `AppSource`, `AppKind`, `SurroundingText`, `ExtraContext`, `classify_app()`, `extract_surrounding()`, `truncate_terminal_scrollback()`（from Task 1）
- Produces: `AxProvider` 实现 `ContextProvider::gather()` 返回真实 macOS 上下文

- [ ] **Step 1: 加 objc2-app-kit 依赖到 Cargo.toml**

在 `crates/desktop/Cargo.toml` 的 `[target.'cfg(target_os = "macos")'.dependencies]` 段加：

```toml
objc2-app-kit = { version = "0.2", features = ["NSWorkspace", "NSRunningApplication"] }
```

Run: `cargo build -p octopus-desktop 2>&1 | tail -5`
Expected: 编译通过（依赖下载）

- [ ] **Step 2: 创建 ffi.rs——AXUIElement C FFI 声明**

```rust
//! macOS Accessibility (AXUIElement) C FFI 声明。
//!
//! AX 函数在 ApplicationServices/HIServices framework，返回 core-foundation 类型。

#![cfg(target_os = "macos")]

use core_foundation::base::{CFTypeRef, OSStatus};
use core_foundation::string::CFStringRef;

/// AXUIElement 不透明指针
pub type AXUIElementRef = *const std::ffi::c_void;
/// AXValue 不透明指针
pub type AXValueRef = *const std::ffi::c_void;

pub type AXError = i32;
pub type AXValueType = u32;

/// AXValue 类型枚举值（AXValue.h）
pub const kAXValueCGPointType: AXValueType = 1;
pub const kAXValueCGSizeType: AXValueType = 2;
pub const kAXValueCFRangeType: AXValueType = 4;

/// CFRange（值类型，用于 AXValueGetValue 解码选区范围）
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CFRange {
    pub location: i64,
    pub length: i64,
}

extern "C" {
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXValueGetValue(
        value: AXValueRef,
        theType: AXValueType,
        valuePtr: *mut std::ffi::c_void,
    ) -> AXError;
    pub fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;

    // CFRelease（core-foundation 已有，但 AX 返回的 CFTypeRef 需手动 release）
    pub fn CFRelease(cf: CFTypeRef);
}

// kAX* 属性字符串是外部符号（CFStringRef），需在运行时获取。
// macOS 上这些是已导出的 CFString 全局变量。
extern "C" {
    pub static kAXFocusedUIElementAttribute: CFStringRef;
    pub static kAXSelectedTextAttribute: CFStringRef;
    pub static kAXSelectedTextRangeAttribute: CFStringRef;
    pub static kAXValueAttribute: CFStringRef;
    pub static kAXTitleAttribute: CFStringRef;
    pub static kAXRoleAttribute: CFStringRef;
}

/// 链接 ApplicationServices framework（含 HIServices / AX）。
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {}
```

- [ ] **Step 3: 在 mod.rs 注册 ffi 模块**

在 `app_context/mod.rs` 的 `#[cfg(target_os = "macos")] mod macos_ax;` 之前加：

```rust
#[cfg(target_os = "macos")]
mod ffi;
```

- [ ] **Step 4: 填充 macos_ax.rs——AxProvider 实现**

```rust
//! macOS Accessibility 实现。
//!
//! 取数路径：
//! 1. NSWorkspace.frontmostApplication → pid + bundleId + name
//! 2. classify_app(bundleId) → AppKind
//! 3. AXUIElementCreateApplication(pid) → app element
//! 4. kAXFocusedUIElementAttribute → focused element
//! 5. kAXSelectedTextRangeAttribute + kAXValueAttribute → 切前后文
//! 6. Terminal 特例：全文作 scrollback 截断
//! 7. kAXTitleAttribute → 窗口标题

#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

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
        let (pid, bundle_id, name) = frontmost_app()
            .ok_or_else(|| anyhow::anyhow!("无法获取前台应用"))?;

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
            match gather_surrounding(pid, kind, deadline) {
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
    use objc2_app_kit::{NSWorkspace, NSRunningApplication};

    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let pid = app.processIdentifier();

    // pid == -1 表示无效
    if pid < 0 {
        return None;
    }

    let bundle_id = app.bundleIdentifier().map(|s| s.to_string());
    let name = app.localizedName()
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("pid:{}", pid));

    Some((pid, bundle_id, name))
}

/// 通过 AX 采集选区周围文本。
fn gather_surrounding(pid: i32, kind: AppKind, deadline: Instant) -> anyhow::Result<SurroundingText> {
    unsafe {
        let app_element = AXUIElementCreateApplication(pid);
        if app_element.is_null() {
            return Err(anyhow::anyhow!("AXUIElementCreateApplication 返回 null"));
        }

        // 获取焦点元素
        let focused = get_attribute_value(app_element, kAXFocusedUIElementAttribute)?;
        let focused_element = focused as AXUIElementRef;

        // 窗口标题
        let window_title = get_attribute_string(focused_element, kAXTitleAttribute)
            .or_else(|| get_attribute_string(app_element, kAXTitleAttribute))
            .ok();

        // 选区范围
        let full_text = get_attribute_string(focused_element, kAXValueAttribute).unwrap_or_default();
        let range = get_selected_range(focused_element).unwrap_or(TextRange { start: 0, end: 0 });

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

        // 超时检查——若已超，返回已采集的部分
        if Instant::now() > deadline {
            log::warn!("[app-context] gather_surrounding 超时，返回部分结果");
        }

        CFRelease(app_element as CFTypeRef);
        Ok(surrounding)
    }
}

/// 安全读取 AX 属性值（返回 CFTypeRef，调用方需根据类型转换）。
unsafe fn get_attribute_value(element: AXUIElementRef, attr: CFStringRef) -> anyhow::Result<CFTypeRef> {
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr, &mut value);
    if err != 0 || value.is_null() {
        return Err(anyhow::anyhow!("AXUIElementCopyAttributeValue error: {}", err));
    }
    Ok(value)
}

/// 读取 AX 字符串属性。
unsafe fn get_attribute_string(element: AXUIElementRef, attr: CFStringRef) -> anyhow::Result<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    let value = get_attribute_value(element, attr)?;
    let cf_string = CFString::wrap_under_create_rule(value as *const _);
    let s = cf_string.to_string();
    Ok(s)
}

/// 读取选区范围 (start, end)，单位为 UTF-16 字符偏移（AX 的 CFRange 单位）。
unsafe fn get_selected_range(element: AXUIElementRef) -> anyhow::Result<TextRange> {
    let value = get_attribute_value(element, kAXSelectedTextRangeAttribute)?;
    let ax_value = value as AXValueRef;

    let mut range = CFRange { location: 0, length: 0 };
    let err = AXValueGetValue(ax_value, kAXValueCFRangeType, &mut range as *mut _ as *mut _);
    CFRelease(value);

    if err == 0 {
        return Err(anyhow::anyhow!("AXValueGetValue failed"));
    }

    // AX range 是 UTF-16 偏移，近似为 Unicode 标量偏移（CJK BMP 内一致）
    Ok(TextRange {
        start: range.location.max(0) as usize,
        end: (range.location + range.length).max(0) as usize,
    })
}
```

- [ ] **Step 5: 编译验证**

Run: `cargo build -p octopus-desktop 2>&1 | tail -20`
Expected: 编译通过。若 `NSWorkspace::sharedWorkspace` / `frontmostApplication` API 不匹配 objc2-app-kit 版本，按编译器提示调整方法名（可能需 `shared` 而非 `sharedWorkspace`，取决于版本）

- [ ] **Step 6: 运行全部测试确认无回归**

Run: `cargo test -p octopus-desktop 2>&1 | tail -10`
Expected: 全部 PASS（Task 1 的纯函数测试不受影响）

- [ ] **Step 7: 提交**

```bash
git add crates/desktop/src/app_context/ffi.rs crates/desktop/src/app_context/macos_ax.rs crates/desktop/src/app_context/mod.rs crates/desktop/Cargo.toml
git commit -m "feat(app-context): macOS AX 实现——NSWorkspace 前台 App + AXUIElement 选区上下文"
```

---

## Task 3: ActionBarContext 升级 + trigger_action_bar 集成 gather

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs:10-17`（ActionBarContext 加字段）
- Modify: `crates/desktop/src/action_bar_commands.rs:100-103`（trigger 暂存上下文处调 gather）

**Interfaces:**
- Consumes: `provider()`, `ExtraContext`, `AppSource`, `SurroundingText`（from Task 1/2）
- Produces: `ActionBarContext { text, source, surrounding }` 序列化到前端

- [ ] **Step 1: 升级 ActionBarContext 结构体**

将 `action_bar_commands.rs:10-15` 的：

```rust
/// 暂存选中文本 + 上下文（trigger 时写入，前端 mount 时 take）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    pub text: String,
}
```

替换为：

```rust
/// 暂存选中文本 + 上下文（trigger 时写入，前端 mount 时 take）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::app_context::AppSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surrounding: Option<crate::app_context::SurroundingText>,
}
```

- [ ] **Step 2: 在 trigger_action_bar 中调 gather**

将 `action_bar_commands.rs` 中暂存上下文的代码（约 100-103 行）：

```rust
        // 5. 暂存上下文
        *PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext { text });
```

替换为：

```rust
        // 5. 暂存上下文——采集来源应用 + 环境上下文（失败降级到仅 text）
        let mut ctx = ActionBarContext { text, source: None, surrounding: None };
        match crate::app_context::provider().gather() {
            Ok(extra) => {
                ctx.source = Some(extra.source);
                ctx.surrounding = extra.surrounding;
            }
            Err(e) => log::warn!("[action-bar] context gather 失败（降级到仅 text）: {}", e),
        }
        *PENDING_CONTEXT.lock().unwrap() = Some(ctx);
```

- [ ] **Step 3: 编译验证**

Run: `cargo build -p octopus-desktop 2>&1 | tail -10`
Expected: 编译通过

- [ ] **Step 4: 运行测试确认无回归**

Run: `cargo test -p octopus-desktop 2>&1 | tail -10`
Expected: 全部 PASS

- [ ] **Step 5: 提交**

```bash
git add crates/desktop/src/action_bar_commands.rs
git commit -m "feat(action-bar): ActionBarContext 加 source/surrounding 字段 + trigger 集成 gather"
```

---

## Task 4: LLM prompt 上下文注入

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`（`execute_action_bar_inner` AI 分支 + 新增 `build_enriched_text` 辅助函数）

**Interfaces:**
- Consumes: `PENDING_CONTEXT`（读取 source/surrounding）, `octopus_llm::chat_text_with_prompt`
- Produces: 增强后的 LLM 输入文本

- [ ] **Step 1: 新增 build_enriched_text 辅助函数**

在 `action_bar_commands.rs` 的辅助函数区（`read_clipboard_text` 附近）加：

```rust
/// 将 ActionBarContext 的 source/surrounding 拼成 LLM 可理解的情境块，
/// 追加到原始选中文本前面。供 AI 动作（润色/摘要/解释/翻译）使用。
fn build_enriched_text(text: &str) -> String {
    let ctx = PENDING_CONTEXT.lock().unwrap();
    let Some(ref ctx) = *ctx else {
        return text.to_string();
    };

    let mut parts = Vec::new();

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
```

- [ ] **Step 2: 在 execute_action_bar_inner 的 AI 分支注入**

将 `execute_action_bar_inner` 中非 auto_translate 的 AI 操作（约 833-839 行）：

```rust
            // 非 auto_translate 的 AI 操作（润色/摘要/解释），仍走 LLM
            let llm_config = crate::config::llm_config_ignore_mode(&config)
                .ok_or("润色模型未配置，请在设置中配置 LLM")?;
            let result = octopus_llm::chat_text_with_prompt(&item.action_data, &text, &llm_config)
                .map_err(|e| e.to_string())?;
            action_bar_show_result(result, String::new(), item.title, app.clone(), true);
            Ok(true)
```

替换为（text → enriched_text）：

```rust
            // 非 auto_translate 的 AI 操作（润色/摘要/解释），仍走 LLM
            let llm_config = crate::config::llm_config_ignore_mode(&config)
                .ok_or("润色模型未配置，请在设置中配置 LLM")?;
            let enriched_text = build_enriched_text(&text);
            let result = octopus_llm::chat_text_with_prompt(&item.action_data, &enriched_text, &llm_config)
                .map_err(|e| e.to_string())?;
            action_bar_show_result(result, String::new(), item.title, app.clone(), true);
            Ok(true)
```

同样，LLM 翻译路径（约 824-827 行）也注入：

```rust
                    TranslateStrategy::Llm => {
                        let llm_config = crate::config::llm_config_ignore_mode(&config)
                            .ok_or("润色模型未配置，请在设置中配置 LLM")?;
                        let enriched_text = build_enriched_text(&text);
                        let prompt = auto_translate_prompt(&enriched_text);
                        let result = octopus_llm::chat_text_with_prompt(prompt, &enriched_text, &llm_config)
                        .map_err(|e| e.to_string())?;
                        action_bar_show_result(result, text, "translate".into(), app.clone(), true);
                        return Ok(true);
                    }
```

> **注意**：本地翻译引擎路径（`TranslateStrategy::Local` / `do_translate_streaming`）**不注入**上下文——本地模型只翻译选中文本本身，上下文反而干扰。`action_bar_show_result` 的 `original_text` / `text` 参数仍传原始 `text`，只有 LLM 输入用 `enriched_text`。

- [ ] **Step 3: 编译验证**

Run: `cargo build -p octopus-desktop 2>&1 | tail -10`
Expected: 编译通过

- [ ] **Step 4: 运行测试**

Run: `cargo test -p octopus-desktop 2>&1 | tail -10`
Expected: 全部 PASS

- [ ] **Step 5: 提交**

```bash
git add crates/desktop/src/action_bar_commands.rs
git commit -m "feat(action-bar): AI 动作注入来源 + 前后文上下文到 LLM prompt"
```

---

## Task 5: 前端类型更新 + 来源标签

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`（Context 类型 + 来源标签展示）

**Interfaces:**
- Consumes: 后端 `ActionBarContext`（含 `source?`, `surrounding?`）

- [ ] **Step 1: 升级 Context 类型定义**

将 `ActionBar/index.tsx:10-12` 的：

```tsx
interface Context {
  text: string;
}
```

替换为：

```tsx
type AppKind = 'editor' | 'terminal' | 'browser' | 'chat' | 'unknown';

interface AppSource {
  bundleId?: string;
  name: string;
  kind: AppKind;
}

interface SurroundingText {
  before?: string;
  after?: string;
  windowTitle?: string;
}

interface Context {
  text: string;
  source?: AppSource;
  surrounding?: SurroundingText;
}
```

- [ ] **Step 2: 添加来源标签 UI**

在浮窗主视图的菜单栏区域（`ScrollRow` 之前或之后），当 `context?.source` 存在时显示一个小标签。找到 JSX 渲染区域的主视图 `view === "main"` 分支，在菜单容器上方加：

```tsx
{context?.source && view === "main" && (
  <div className="flex items-center gap-1 px-2 py-0.5 text-[9px] text-muted-foreground/70 shrink-0">
    <span className="truncate max-w-[120px]">{context.source.name}</span>
  </div>
)}
```

> 位置：放在 `data-action-bar` 容器内的最上方一行，菜单 `ScrollRow` 之前。具体插入点需阅读渲染区 JSX 确定。

- [ ] **Step 3: 前端编译验证**

Run: `cd crates/desktop/frontend && npx tsc --noEmit 2>&1 | tail -10`
Expected: 无类型错误

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/index.tsx
git commit -m "feat(action-bar): 前端 Context 类型升级 + 来源应用标签展示"
```

---

## Task 6: 手动验证 + 文档同步

**Files:**
- Modify: `docs/architecture.md`（action bar 模块描述加 app_context）
- Modify: `docs/superpowers/plans/2026-07-12-actionbar-app-context.md`（review 回写实际偏差）

- [ ] **Step 1: 手动验证清单**

构建并运行桌面应用：

```bash
cargo run --release -p octopus-desktop --features embedded 2>&1 | head -5 &
```

在以下 App 中各选中一段文字，触发 action bar 热键，检查终端日志输出的 `[action-bar]` 和 `[app-context]` 行：

- [ ] TextEdit——验证 Editor：source.kind=editor，before/after 有内容
- [ ] Terminal.app——验证 Terminal：source.kind=terminal，before 为 scrollback 截断
- [ ] Safari 地址栏/文本框——验证 Browser
- [ ] VSCode 编辑器——验证 Electron App 的 AX 覆盖
- [ ] 无辅助功能权限场景——验证降级（日志出现 "context gather 失败"，浮窗照常显示）

> 查看完整上下文日志：在 `gather()` 成功路径加临时 `log::info!("[app-context] source={:?} surrounding={:?}", ...)` 确认数据正确，验证后删除临时日志。

- [ ] **Step 2: 更新 architecture.md**

在 action bar 相关章节补充 app_context 模块说明：

```markdown
### app_context 模块
Action Bar 触发时获取选中文本的来源应用上下文（平台无障碍 API）。
- `ContextProvider` trait + cfg 分发
- macOS：NSWorkspace（前台 App）+ AXUIElement（AX 树读取选区/前后文）
- Windows/Linux：stub（返回 Err，降级到仅选中文本）
- 失败不阻塞浮窗——降级到现有「仅 text」行为
```

- [ ] **Step 3: Plan review 回写**

回顾本 plan，把实际实现中的偏差回写到 plan 文档（objc2-app-kit API 调整、FFI 符号名修正等）。

- [ ] **Step 4: 提交**

```bash
git add docs/architecture.md docs/superpowers/plans/2026-07-12-actionbar-app-context.md
git commit -m "docs: 同步 app_context 模块到 architecture + plan review 回写"
```

---

## 回顾检查项

实现完成后，对照 spec 逐条验证：

1. ☐ `ActionBarContext` 有 `source` + `surrounding` 字段，`skip_serializing_if` 保证向后兼容
2. ☐ macOS 走 NSWorkspace + AXUIElement，Windows/Linux 走 NullProvider
3. ☐ `gather()` 失败/超时降级到「仅 text」，浮窗照常显示
4. ☐ Terminal 特例：before 走 scrollback 截断（50 行 / 2000 字）
5. ☐ Editor：before/after 各 2000 字截断
6. ☐ AX 超时 500ms
7. ☐ AI 动作 prompt 注入上下文；本地翻译不注入
8. ☐ 前端类型升级 + 来源标签
9. ☐ 纯函数单元测试（classify_app、extract_surrounding、truncate_terminal_scrollback）
10. ☐ 手动验证 5 个场景
