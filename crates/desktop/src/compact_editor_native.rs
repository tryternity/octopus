//! macOS 原生 compact editor:NSWindow + NSScrollView + NSTextView(无 webview)。
//! 正式实现(Task 5+)。objc2 写法复用 spike 验证的模式。
//! 非 macOS 不编译本文件内容,回退 webview(见 compact_editor_window.rs 分流)。

#[cfg(target_os = "macos")]
mod imp {
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSFont, NSScrollView, NSTextView, NSWindow};
    use objc2_foundation::NSString;
    use tauri::Manager;

    use crate::compact_editor_window::{HEIGHT, MIN_HEIGHT, MIN_WIDTH, WIDTH, WINDOW_LABEL};

    /// 建无 webview 原生窗(`WindowBuilder`)+ 挂 NSScrollView/NSTextView。返回后窗口已显示。
    /// 文本由 `set_text` 塞(open 时首次塞 / 并发再开换文本)。
    pub fn create_native_window(app: &tauri::AppHandle) {
        use tauri::WindowBuilder;
        match WindowBuilder::new(app, WINDOW_LABEL)
            .title("编辑")
            .inner_size(WIDTH, HEIGHT)
            .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
            .decorations(true)
            .resizable(true)
            .center()
            .visible(true)
            .build()
        {
            Ok(w) => {
                attach_textview(&w);
                // 关窗兜底：Destroyed 时补 cancel（未 saved）+ 切回 Accessory。
                let app_clone = app.clone();
                let _ = w.on_window_event(move |event| {
                    if let tauri::WindowEvent::Destroyed = event {
                        crate::compact_editor_commands::on_window_destroyed(&app_clone);
                    }
                });
            }
            Err(e) => log::warn!("native compact editor build failed: {e}"),
        }
    }

    /// 在原生窗上挂 NSScrollView+NSTextView(初始为空)。objc2 写法复用 spike 验证模式。
    fn attach_textview(window: &tauri::window::Window) {
        let win = window.clone();
        let _ = window.run_on_main_thread(move || {
            let Ok(ptr) = win.ns_window() else {
                return;
            };
            if ptr.is_null() {
                return;
            }
            let mtm = MainThreadMarker::new().expect("run_on_main_thread 在主线程执行");
            let ns_win: &NSWindow = unsafe { &*(ptr as *const NSWindow) };

            let text_view = NSTextView::new(mtm);
            text_view.setRichText(false);
            let font = NSFont::systemFontOfSize(15.0);
            text_view.setFont(Some(&font));
            text_view.setEditable(true);
            text_view.setSelectable(true);

            let scroll = NSScrollView::new(mtm);
            if let Some(cv) = ns_win.contentView() {
                scroll.setFrame(cv.frame());
            }
            // 子类 → NSView 靠 objc2 多步 deref coercion(spike 验证)
            scroll.setDocumentView(Some(&text_view));
            scroll.setHasVerticalScroller(true);
            scroll.setAutoresizesSubviews(true);
            ns_win.setContentView(Some(&scroll));

            log::info!("[native] compact editor NSTextView attached");
        });
    }

    /// 主线程上取当前窗的 NSTextView(contentView=ScrollView → documentView=TextView)。
    /// 仅在 run_on_main_thread 闭包内调用(NSView 系主线程对象)。
    /// 返回 owned Retained(对 textview 额外 retain,scroll 已持有一份,无妨)。
    fn current_text_view(window: &tauri::window::Window) -> Option<Retained<NSTextView>> {
        let Ok(ptr) = window.ns_window() else {
            return None;
        };
        if ptr.is_null() {
            return None;
        }
        let ns_win: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
        // contentView → NSScrollView(attach_textview 设的)
        let content = ns_win.contentView()?;
        let scroll = content.downcast::<NSScrollView>().ok()?;
        // documentView → NSTextView
        let doc = scroll.documentView()?;
        doc.downcast::<NSTextView>().ok()
    }

    /// 把 text 塞进当前窗的 NSTextView(首次塞文本 / 并发再开换文本共用)。
    /// 取窗走 Manager::get_window(原生 Window,非 webview)。
    /// 与 attach_textview 都经 run_on_main_thread 排队,attach 先于 set_text 执行,
    /// 故 set_text 跑到时 textview 已挂好。
    pub fn set_text(app: &tauri::AppHandle, text: &str) {
        let Some(window) = app.get_window(WINDOW_LABEL) else {
            log::warn!("[native] set_text: window {WINDOW_LABEL} not found");
            return;
        };
        let win = window.clone();
        let text = text.to_string();
        let _ = window.run_on_main_thread(move || {
            let Some(tv) = current_text_view(&win) else {
                log::warn!("[native] set_text: textview not attached yet");
                return;
            };
            tv.setString(&NSString::from_str(&text));
            log::info!("[native] compact editor text set ({} 字节)", text.len());
        });
    }

    /// 主线程读 NSTextView 全文并回调 `f(text)`。
    ///
    /// run_on_main_thread 异步排队(非阻塞),无法把文本同步回传给调用方,故 do_save
    /// 须把「emit result / mark_saved / 关窗」全放进 `f` 内,在主线程闭包里一并完成
    /// (plan Task 6 标注的「排队(异步)」分支)。`f` 收到 owned String,无生命周期耦合。
    pub fn with_text<F>(app: &tauri::AppHandle, f: F)
    where
        F: FnOnce(String) + Send + 'static,
    {
        let Some(window) = app.get_window(WINDOW_LABEL) else {
            log::warn!("[native] with_text: window {WINDOW_LABEL} not found");
            return;
        };
        let win = window.clone();
        let _ = window.run_on_main_thread(move || {
            let Some(tv) = current_text_view(&win) else {
                log::warn!("[native] with_text: textview not attached");
                return;
            };
            let text = tv.string().to_string(); // Retained<NSString> → String(impl Display)
            f(text);
        });
    }
}

#[cfg(target_os = "macos")]
pub use imp::*;
