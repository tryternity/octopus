//! 剪贴板 concealed 写入：30 秒后自动清空。
//!
//! **实现**（订正次-2：原文件头注释「走 ClipboardHandle::write_text」与实际代码不符）：
//! 直接走 NSPasteboard 强类型 API（`objc2-app-kit`），**不**经过 `ClipboardHandle`。
//! 因此不会自动 `suppress_next`——suppress 必须由调用方在调本函数前手动调
//! `handle.suppress_next()`，否则 octopus 自身 `clipboard_history` watcher 会把
//! 密码写入 FTS 索引（详见 vault_commands.rs:650 / 732）。
//!
//! 单独写 `org.nspasteboard.ConcealedType` 标记让第三方剪贴板工具
//! （Maccy / Paste / iCloud Universal Clipboard）跳过收集。
//!
//! **定时清空**（订正次-2）：用 `tauri::async_runtime::spawn` + `tokio::time::sleep`，
//! 与项目其他定时任务一致（避免裸 `std::thread::spawn` 的不可取消 + 多复制竞争定时器）。
//! 注意仍存在 suppress flag 被提前消费的竞态（见 vault_commands.rs:732 注释）。

use anyhow::Result;
use std::time::Duration;

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

const DEFAULT_TTL: Duration = Duration::from_secs(30);
const CONCEALED_MARKER: &str = "octopus-vault-concealed";
const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";

/// 复制到剪贴板并 concealed 标记 + 30s 自动清空。
pub fn copy_concealed(text: &str) -> Result<()> {
    copy_concealed_with_ttl(text, DEFAULT_TTL)
}

pub fn copy_concealed_with_ttl(text: &str, ttl: Duration) -> Result<()> {
    // NSPasteboard 强类型 API（objc2-app-kit 0.3）：
    // - generalPasteboard / clearContents / setString_forType 全部为 safe 方法
    // - NSPasteboardTypeString 是 `&'static NSString` 静态量
    // - NSPasteboardType = NSString（type alias），所以 ConcealedType 也是 &NSString
    {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();

        // 写入正文文本（系统剪贴板标准类型）
        let text_ns = NSString::from_str(text);
        // NSPasteboardTypeString 是 extern static，需 unsafe 借用
        let string_type = unsafe { NSPasteboardTypeString };
        pb.setString_forType(&text_ns, string_type);

        // 写入 concealed 标记（第三方剪贴板工具识别：Maccy / Paste / iCloud）
        let marker = NSString::from_str(CONCEALED_MARKER);
        let concealed_type = NSString::from_str(CONCEALED_TYPE);
        pb.setString_forType(&marker, &concealed_type);
    }

    // 让 octopus 自身 clipboard 监听器跳过这次写入
    // 方式：通过 ClipboardHandle::suppress_next（需 AppState 提供 handle 引用）
    // 此处由调用方在调本函数前手动调 handle.suppress_next()

    // spawn 定时清空（默认 30s）——走 tauri::async_runtime + tokio::time（订正次-2）
    //
    // **防误清**（修复 C）：把写入内容 snapshot 到 task，清除前读 pasteboard 比对，
    // 相同才 clearContents——用户在 TTL 期间复制其他内容时不会被误清。
    // 仍存在的窗口：多复制时多个独立 task（未实现单一定时器 + cancellation），
    // 但单次复制场景的误清问题已解决（最常见的失败场景）。
    let snapshot = text.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(ttl).await;
        let _ = clear_clipboard_if_matches(&snapshot);
    });

    Ok(())
}

/// 仅当当前 pasteboard 文本 == `expected` 时清空（修复 C：防误清用户后续复制）。
///
/// 读 NSPasteboard 内容比对：
/// - 相同 → clearContents 清空（这次清空是本次复制的 TTL 责任）
/// - 不同 → 不动（用户在 TTL 期间已复制了别的内容，不能覆盖）
/// - 读失败 → 仍清（保守：防密码残留，宁可错清用户笔记也不留密码）
fn clear_clipboard_if_matches(expected: &str) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    let string_type = unsafe { NSPasteboardTypeString };
    let current = pb.stringForType(string_type);
    let should_clear = match current {
        Some(s) => s.to_string() == expected,
        None => true, // 读失败 → 保守清空（防密码残留）
    };
    if should_clear {
        pb.clearContents();
        let empty = NSString::from_str("");
        pb.setString_forType(&empty, string_type);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_ttl_is_30s() {
        assert_eq!(DEFAULT_TTL, Duration::from_secs(30));
    }

    /// INV-A4 核心不变量：concealed marker 字面量必须固定——
    /// 第三方剪贴板工具（Maccy / Paste / iCloud Universal Clipboard）按这两个
    /// 字符串识别 concealed 内容并跳过收集，改一个字符就失效。
    #[test]
    fn test_concealed_marker_constants_are_fixed() {
        assert_eq!(CONCEALED_MARKER, "octopus-vault-concealed");
        assert_eq!(CONCEALED_TYPE, "org.nspasteboard.ConcealedType");
    }

    /// CONCEALED_TYPE 是 NSPasteboard 跨工具协议约定的字面量——
    /// 形如 "org.nspasteboard.<Type>"，验证它属于该命名空间。
    #[test]
    fn test_concealed_type_in_nspasteboard_namespace() {
        assert!(
            CONCEALED_TYPE.starts_with("org.nspasteboard."),
            "CONCEALED_TYPE 应在 NSPasteboard 协议命名空间下"
        );
    }
}
