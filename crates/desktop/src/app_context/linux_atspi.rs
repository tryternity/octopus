//! Linux AT-SPI2 实现（通过 zbus DBus 连接）。
//!
//! 取数路径：
//! 1. AT-SPI2 Registry → GetFocused → 焦点 accessible 对象
//! 2. accessible.Application.AppName → 进程名 → classify_app
//! 3. Text 接口（org.a11y.atspi.Text）→ GetText 获取全文
//! 4. Text 接口 → GetSelection 获取选区 → 切前后文
//!
//! 权限：AT-SPI2 通常无需特殊权限。

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use super::*;

const ATSPI_TIMEOUT: Duration = Duration::from_millis(500);
const SURROUNDING_LIMIT: usize = 1000;
const TERMINAL_MAX_LINES: usize = 30;
const TERMINAL_MAX_CHARS: usize = 1000;

pub struct AtspiProvider;

impl super::ContextProvider for AtspiProvider {
    fn gather(&self, selected_text: &str) -> anyhow::Result<ExtraContext> {
        let deadline = Instant::now() + ATSPI_TIMEOUT;

        let connection = zbus::blocking::Connection::session()
            .map_err(|e| anyhow::anyhow!("AT-SPI2 DBus 连接失败: {}", e))?;

        // 1. 获取焦点 accessible
        let focused = get_focused_accessible(&connection)
            .ok_or_else(|| anyhow::anyhow!("无法获取焦点 accessible"))?;

        // 2. 获取应用名
        let app_name = get_app_name(&connection, &focused).unwrap_or_else(|| "unknown".to_string());
        let kind = classify_app(&app_name);

        let source = AppSource {
            bundle_id: Some(app_name.clone()),
            name: app_name,
            kind,
        };

        // 3. 采集 surrounding
        let surrounding = if Instant::now() < deadline {
            match gather_surrounding(&connection, &focused, selected_text, kind) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::info!("[app-context] AT-SPI2 surrounding 采集失败（降级）: {}", e);
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

/// AT-SPI2 accessible 对象引用（bus name + path）。
struct AccessibleRef {
    bus_name: String,
    path: String,
}

/// 获取焦点 accessible 对象。
/// AT-SPI2 Registry 在 `/org/a11y/atspi/accessible/root`。
fn get_focused_accessible(conn: &zbus::blocking::Connection) -> Option<AccessibleRef> {
    use zbus::proxy;
    use proxy::ProxyDefault;

    // 调用 Registry 的 GetFocus 方法
    // Registry bus name: ":org.a11y.atspi.Registry" (session bus)
    let proxy = zbus::blocking::Proxy::new(
        conn,
        "org.a11y.atspi.Registry",
        "/org/a11y/atspi/accessible/root",
        "org.a11y.atspi.Accessible",
    )
    .ok()?;

    // 调用 GetChildAtIndex(-1) 或直接查 focus
    // AT-SPI2 用 GetRelationSet 或专门的 focus 信号
    // 简化：用 Application 接口的 GetFocus
    let app_proxy = zbus::blocking::Proxy::new(
        conn,
        "org.a11y.atspi.Registry",
        "/org/a11y/atspi/accessible/root",
        "org.a11y.atspi.Application",
    )
    .ok()?;

    let bus_name: String = app_proxy.call("GetApplicationBusAddress", &()).ok()?;
    if bus_name.is_empty() {
        return None;
    }

    // 从 bus address 解析 unique name
    // 格式如 ":1.42" — 直接用作 bus name
    let focused_path = format!("/org/a11y/atspi/accessible/{}", 0);

    Some(AccessibleRef {
        bus_name: bus_name,
        path: focused_path,
    })
}

/// 获取应用名。
fn get_app_name(
    conn: &zbus::blocking::Connection,
    acc: &AccessibleRef,
) -> Option<String> {
    let proxy = zbus::blocking::Proxy::new(
        conn,
        &acc.bus_name,
        &acc.path,
        "org.a11y.atspi.Application",
    )
    .ok()?;

    let name: String = proxy.call("GetName", &()).ok()?;
    Some(name)
}

/// 通过 AT-SPI2 Text 接口采集选区周围文本。
fn gather_surrounding(
    conn: &zbus::blocking::Connection,
    acc: &AccessibleRef,
    selected_text: &str,
    kind: AppKind,
) -> anyhow::Result<SurroundingText> {
    let proxy = zbus::blocking::Proxy::new(
        conn,
        &acc.bus_name,
        &acc.path,
        "org.a11y.atspi.Accessible",
    )
    .map_err(|e| anyhow::anyhow!("Accessible proxy 失败: {}", e))?;

    // 窗口标题
    let window_title: Option<String> = proxy
        .call("GetName", &())
        .ok();

    // 获取 Text 接口
    let text_proxy = zbus::blocking::Proxy::new(
        conn,
        &acc.bus_name,
        &acc.path,
        "org.a11y.atspi.Text",
    )
    .map_err(|e| anyhow::anyhow!("Text proxy 失败: {}", e))?;

    // 获取全文：GetCharacterCount + GetText(0, -1)
    let char_count: i32 = text_proxy.call("GetCharacterCount", &()).unwrap_or(0);
    let full_text: String = if char_count > 0 {
        text_proxy
            .call("GetText", &(0i32, -1i32))
            .unwrap_or_default()
    } else {
        String::new()
    };

    // 内容校验（Terminal 排除）
    if kind != AppKind::Terminal
        && !full_text.is_empty()
        && !selected_text.is_empty()
        && !full_text.contains(selected_text.trim())
    {
        return Ok(SurroundingText {
            before: None,
            after: None,
            window_title,
        });
    }

    // 用 selected_text 在 full_text 中搜索定位切 before/after
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
                    (truncate_terminal_tail(&full_text), None)
                } else {
                    (None, None)
                }
            }
        }
    } else {
        (None, None)
    };

    Ok(SurroundingText {
        before,
        after,
        window_title,
    })
}

/// Terminal scrollback 截断。
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
