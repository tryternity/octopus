//! macOS 原生 compact editor:NSWindow + NSScrollView + NSTextView。
//! 试水 spike:验证原生控件能挂、中文 IME/滚动/取文本可行。
//! 非 macOS 不编译本文件内容,回退 webview(见 compact_editor_window.rs 分流)。

#[cfg(target_os = "macos")]
mod imp {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSFont, NSScrollView, NSTextView, NSWindow};
    use objc2_foundation::NSString;
    use tauri::WebviewWindow;

    /// 临时 spike 入口:在给定 webview 窗口上(借用其底层 NSWindow)挂一个 NSTextView
    /// 显示静态中文。spike 验证完即删,正式实现见 Task 5+(WindowBuilder 原生窗)。
    ///
    /// spike 阶段先复用一个普通 webview 窗的 NSWindow 来挂 NSTextView——绕开
    /// WindowBuilder 尚未接入,先验证「挂控件 + IME + 滚动 + 取文本」这条链
    /// (NSWindow 路径与 result_window/pin_window 已验证的用法一致)。
    pub fn spike_attach_textview(window: &WebviewWindow) {
        let win = window.clone();
        let _ = window.run_on_main_thread(move || {
            let Ok(ptr) = win.ns_window() else {
                return;
            };
            if ptr.is_null() {
                return;
            }
            // run_on_main_thread 保证主线程,可安全取 MainThreadMarker
            let mtm = MainThreadMarker::new().expect("run_on_main_thread 在主线程执行");
            let ns_win: &NSWindow = unsafe { &*(ptr as *const NSWindow) };

            // 建 NSTextView(纯文本)——objc2 0.6 的 new() 需 MainThreadMarker
            let text_view = NSTextView::new(mtm);
            text_view.setRichText(false);
            text_view.setString(&NSString::from_str(
                "这是一段用于 spike 的中文文本。\n你可以用输入法编辑我。\n",
            ));
            let font = NSFont::systemFontOfSize(15.0);
            text_view.setFont(Some(&font));
            text_view.setEditable(true);
            text_view.setSelectable(true);

            // 包进 NSScrollView,frame 取自窗口 contentView
            let scroll = NSScrollView::new(mtm);
            if let Some(cv) = ns_win.contentView() {
                scroll.setFrame(cv.frame());
            }
            // 子类(NSTextView/NSScrollView)→ NSView 靠 objc2 多步 deref coercion
            // (同 pin_window.rs `content.addSubview(&image_view)` 已验证模式)
            scroll.setDocumentView(Some(&text_view));
            scroll.setHasVerticalScroller(true);
            scroll.setAutoresizesSubviews(true);
            ns_win.setContentView(Some(&scroll));

            // 取文本验证:打印字符数到日志,证明能读回
            let s = text_view.string();
            log::info!(
                "[spike] NSTextView attached, chars={}",
                s.to_string().chars().count()
            );
        });
    }
}

#[cfg(target_os = "macos")]
pub use imp::*;
