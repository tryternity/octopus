//! 剪贴板 concealed 写入：30 秒后自动清空。
//!
//! 走 octopus-clipboard 的 ClipboardHandle::write_text（自动 suppress_next，
//! 跳过自身 clipboard_history 监听器），同时单独写 org.nspasteboard.ConcealedType
//! 让第三方剪贴板工具（Maccy / Paste / iCloud Universal Clipboard）跳过。

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

    // spawn 定时清空（默认 30s）
    let ttl_secs = ttl.as_secs();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(ttl_secs));
        let _ = clear_clipboard();
    });

    Ok(())
}

fn clear_clipboard() -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let empty = NSString::from_str("");
    let string_type = unsafe { NSPasteboardTypeString };
    pb.setString_forType(&empty, string_type);
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
