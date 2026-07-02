//! macOS 原生 compact editor：NSWindow + 工具栏(NSView/NSButton) + NSScrollView/NSTextView。
//! 无 webview。objc2 写法复用 spike/pin_window 验证模式。非 macOS 不编译本文件内容，
//! 回退 webview(见 compact_editor_window.rs 分流)。

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{define_class, msg_send, sel, ClassType, MainThreadMarker};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSControl, NSFont, NSScrollView,
        NSTextField, NSTextView, NSView, NSWindow,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use tauri::Manager;

    use crate::compact_editor_window::{HEIGHT, MIN_HEIGHT, MIN_WIDTH, WINDOW_LABEL, WIDTH};

    const TOOLBAR_H: f64 = 36.0;

    // ── 字号持久化(app_config，仿 window_position)──
    const FONT_KEY: &str = "compact_editor.font_size";
    const FONT_MIN: f64 = 12.0;
    const FONT_MAX: f64 = 24.0;
    const FONT_DEFAULT: f64 = 15.0;

    fn load_font_size() -> f64 {
        octopus_infra::db::load_config_key(FONT_KEY)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|&s| (FONT_MIN..=FONT_MAX).contains(&s))
            .unwrap_or(FONT_DEFAULT)
    }
    fn save_font_size(s: f64) {
        let _ = octopus_infra::db::save_config_key(FONT_KEY, &s.to_string());
    }

    // ── 按钮 tag（onClick: 据 tag 分发）──
    const TAG_UNDO: isize = 1;
    const TAG_REDO: isize = 2;
    const TAG_FONT_DEC: isize = 3;
    const TAG_FONT_INC: isize = 4;
    const TAG_FIND: isize = 5;
    const TAG_CLEAR: isize = 6;
    const TAG_CANCEL: isize = 7;
    const TAG_SAVE: isize = 8;

    // ── 清空二次确认态（复刻前端 clearPending）──
    static CLEAR_PENDING: AtomicBool = AtomicBool::new(false);

    /// 当前编辑窗的全部主线程控件引用 + AppHandle。
    /// attach 时一次性塞入；全部 Retained 对象为主线程专属，故 unsafe Send/Sync（仅在主线程
    /// run_on_main_thread / AppKit Action 回调内访问）。reopen 时整体覆盖（旧的在主线程闭包内 drop，安全）。
    struct SendState {
        #[allow(dead_code)]
        app: tauri::AppHandle,
        text_view: Retained<NSTextView>,
        font_label: Retained<NSTextField>,
        clear_button: Retained<NSButton>,
        #[allow(dead_code)]
        target: Retained<CompactEditorButtonTarget>,
    }
    unsafe impl Send for SendState {}
    unsafe impl Sync for SendState {}
    static STATE: Mutex<Option<SendState>> = Mutex::new(None);

    /// 主线程闭包内对当前 textview 做事（undo/redo/find 等同步操作）。
    fn with_tv<F: FnOnce(&NSTextView)>(f: F) {
        let guard = STATE.lock().unwrap();
        if let Some(s) = guard.as_ref() {
            f(&s.text_view);
        }
    }

    // ── 按钮 target：无 ivar，onClick: 读 sender.tag() 分发到 Rust ──
    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "CompactEditorButtonTarget"]
        struct CompactEditorButtonTarget;

        impl CompactEditorButtonTarget {
            #[unsafe(method(onClick:))]
            fn on_click(&self, sender: &NSControl) {
                let tag = sender.tag();
                let app = STATE.lock().unwrap().as_ref().map(|s| s.app.clone());
                if let Some(app) = app {
                    on_button_clicked(tag, &app);
                }
            }
        }
    );

    /// tag → 行为分发（AppKit Action 回调，主线程）。
    fn on_button_clicked(tag: isize, app: &tauri::AppHandle) {
        match tag {
            TAG_SAVE => crate::compact_editor_commands::do_save(app),
            TAG_CANCEL => crate::compact_editor_commands::do_cancel(app),
            TAG_UNDO => with_tv(|tv| {
                if let Some(u) = tv.undoManager() {
                    if u.canUndo() {
                        u.undo();
                    }
                }
            }),
            TAG_REDO => with_tv(|tv| {
                if let Some(u) = tv.undoManager() {
                    if u.canRedo() {
                        u.redo();
                    }
                }
            }),
            TAG_FIND => with_tv(|tv| unsafe {
                let _ = tv.performFindPanelAction(None);
            }),
            TAG_FONT_DEC => set_font_size(app, -1.0),
            TAG_FONT_INC => set_font_size(app, 1.0),
            TAG_CLEAR => on_clear_clicked(app),
            _ => {}
        }
    }

    /// 字号 ±：load→clamp→save→主线程重设 textview font + 更新字号 label。
    fn set_font_size(app: &tauri::AppHandle, delta: f64) {
        let cur = load_font_size();
        let new = (cur + delta).clamp(FONT_MIN, FONT_MAX);
        if (new - cur).abs() < f64::EPSILON {
            return; // 钳制后无变化（已到上下限）
        }
        save_font_size(new);
        let Some(w) = app.get_window(WINDOW_LABEL) else {
            return;
        };
        let _ = w.run_on_main_thread(move || {
            let guard = STATE.lock().unwrap();
            let Some(s) = guard.as_ref() else {
                return;
            };
            s.text_view.setFont(Some(&NSFont::systemFontOfSize(new)));
            s.font_label
                .setStringValue(&NSString::from_str(&format!("{}", new as i64)));
        });
    }

    /// 清空二次确认：首次点→切「确认清空」+ 2s 复位；2s 内再点→真正清空。
    fn on_clear_clicked(app: &tauri::AppHandle) {
        if !CLEAR_PENDING.swap(true, Ordering::Relaxed) {
            // 首次：切确认态（已在主线程，直接改 title）
            {
                let guard = STATE.lock().unwrap();
                if let Some(s) = guard.as_ref() {
                    s.clear_button.setTitle(&NSString::from_str("确认清空?"));
                }
            }
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                CLEAR_PENDING.store(false, Ordering::Relaxed);
                if let Some(w) = app2.get_window(WINDOW_LABEL) {
                    let _ = w.run_on_main_thread(move || {
                        let guard = STATE.lock().unwrap();
                        if let Some(s) = guard.as_ref() {
                            s.clear_button.setTitle(&NSString::from_str("清空"));
                        }
                    });
                }
            });
            return;
        }
        // 二次：清空文本 + 复位按钮
        CLEAR_PENDING.store(false, Ordering::Relaxed);
        {
            let guard = STATE.lock().unwrap();
            if let Some(s) = guard.as_ref() {
                s.text_view.setString(&NSString::from_str(""));
                s.clear_button.setTitle(&NSString::from_str("清空"));
            }
        }
    }

    /// 建一个工具栏按钮（标题/tag/frame/绑定 target-action）。
    fn make_button(
        mtm: MainThreadMarker,
        title: &str,
        tag: isize,
        target: &CompactEditorButtonTarget,
        frame: NSRect,
    ) -> Retained<NSButton> {
        let btn = NSButton::new(mtm);
        btn.setTitle(&NSString::from_str(title));
        btn.setFrame(frame);
        btn.setTag(tag);
        btn.setBezelStyle(NSBezelStyle::Push);
        unsafe {
            let _: () = msg_send![&*btn, setTarget: target];
            let _: () = msg_send![&*btn, setAction: sel!(onClick:)];
        }
        btn
    }

    /// 建无 webview 原生窗(`WindowBuilder`)+ 挂工具栏/NSTextView。返回后窗口已显示。
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
                attach_textview(&w, app);
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

    /// 在原生窗上挂工具栏(NSView+NSButton 横排)+ NSScrollView/NSTextView，并存入 STATE。
    fn attach_textview(window: &tauri::window::Window, app: &tauri::AppHandle) {
        let win = window.clone();
        let app = app.clone();
        let _ = window.run_on_main_thread(move || {
            let Ok(ptr) = win.ns_window() else {
                return;
            };
            if ptr.is_null() {
                return;
            }
            let mtm = MainThreadMarker::new().expect("run_on_main_thread 在主线程执行");
            let ns_win: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
            let content = ns_win
                .contentView()
                .expect("window must have content view");
            let (cw, ch) = {
                let sz = content.frame().size;
                (sz.width, sz.height)
            };

            // 工具栏容器（顶部 TOOLBAR_H，宽度跟窗口，顶部钉住）
            let toolbar = NSView::new(mtm);
            toolbar.setFrame(NSRect::new(
                NSPoint::new(0., ch - TOOLBAR_H),
                NSSize::new(cw, TOOLBAR_H),
            ));
            toolbar.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
            );

            // target（单例，所有按钮共用；STATE 持有其 Retained 保活——NSControl target 是弱引用）
            let target: Retained<CompactEditorButtonTarget> =
                unsafe { msg_send![CompactEditorButtonTarget::class(), new] };

            let font_size = load_font_size();
            let y = 4.0;
            let bh = TOOLBAR_H - 8.0;
            let mut x = 6.0;
            let mut step = |w: f64, gap: f64| {
                let cur = x;
                x += w + gap;
                cur
            };

            let undo = make_button(
                mtm,
                "撤销",
                TAG_UNDO,
                &target,
                NSRect::new(NSPoint::new(step(38., 2.), y), NSSize::new(38., bh)),
            );
            let redo = make_button(
                mtm,
                "重做",
                TAG_REDO,
                &target,
                NSRect::new(NSPoint::new(step(38., 8.), y), NSSize::new(38., bh)),
            );
            let fdec = make_button(
                mtm,
                "−",
                TAG_FONT_DEC,
                &target,
                NSRect::new(NSPoint::new(step(22., 0.), y), NSSize::new(22., bh)),
            );
            // 字号显示 label
            let font_label = NSTextField::new(mtm);
            font_label.setBezeled(false);
            font_label.setDrawsBackground(false);
            font_label.setEditable(false);
            font_label.setSelectable(false);
            font_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            font_label.setStringValue(&NSString::from_str(&format!("{}", font_size as i64)));
            font_label.setFrame(NSRect::new(NSPoint::new(step(26., 0.), y), NSSize::new(26., bh)));
            let finc = make_button(
                mtm,
                "+",
                TAG_FONT_INC,
                &target,
                NSRect::new(NSPoint::new(step(22., 8.), y), NSSize::new(22., bh)),
            );
            let find = make_button(
                mtm,
                "查找",
                TAG_FIND,
                &target,
                NSRect::new(NSPoint::new(step(40., 6.), y), NSSize::new(40., bh)),
            );
            let clear = make_button(
                mtm,
                "清空",
                TAG_CLEAR,
                &target,
                NSRect::new(NSPoint::new(step(44., 6.), y), NSSize::new(44., bh)),
            );
            // 右侧组：保存(右贴边) / 取消
            let save = make_button(
                mtm,
                "保存",
                TAG_SAVE,
                &target,
                NSRect::new(NSPoint::new(cw - 6. - 56., y), NSSize::new(56., bh)),
            );
            let cancel = make_button(
                mtm,
                "取消",
                TAG_CANCEL,
                &target,
                NSRect::new(NSPoint::new(cw - 6. - 56. - 6. - 52., y), NSSize::new(52., bh)),
            );

            for btn in [&undo, &redo, &fdec, &finc, &find, &clear, &cancel, &save] {
                toolbar.addSubview(btn);
            }
            toolbar.addSubview(&font_label);

            // 文本区（占 toolbar 以下）
            let text_view = NSTextView::new(mtm);
            text_view.setRichText(false);
            text_view.setFont(Some(&NSFont::systemFontOfSize(font_size)));
            text_view.setEditable(true);
            text_view.setSelectable(true);
            text_view.setUsesFindBar(true);

            let scroll = NSScrollView::new(mtm);
            scroll.setFrame(NSRect::new(
                NSPoint::new(0., 0.),
                NSSize::new(cw, ch - TOOLBAR_H),
            ));
            scroll.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            scroll.setDocumentView(Some(&text_view));
            scroll.setHasVerticalScroller(true);
            scroll.setAutoresizesSubviews(true);

            content.addSubview(&toolbar);
            content.addSubview(&scroll);

            // 存 STATE（reopen 时覆盖，旧的在主线程闭包内 drop，安全）
            let clear_btn = clear.clone();
            *STATE.lock().unwrap() = Some(SendState {
                app: app.clone(),
                text_view,
                font_label,
                clear_button: clear_btn,
                target,
            });
            log::info!("[native] compact editor toolbar + textview attached");
        });
    }

    /// 把 text 塞进当前窗的 NSTextView(首次塞文本 / 并发再开换文本共用)。
    /// attach 先于 set_text 排队(run_on_main_thread)，故 set_text 跑到时 STATE 已设。
    pub fn set_text(app: &tauri::AppHandle, text: &str) {
        let Some(window) = app.get_window(WINDOW_LABEL) else {
            log::warn!("[native] set_text: window {WINDOW_LABEL} not found");
            return;
        };
        let text = text.to_string();
        let _ = window.run_on_main_thread(move || {
            let guard = STATE.lock().unwrap();
            let Some(s) = guard.as_ref() else {
                log::warn!("[native] set_text: STATE not set");
                return;
            };
            s.text_view.setString(&NSString::from_str(&text));
            log::info!("[native] compact editor text set ({} 字节)", text.len());
        });
    }

    /// 主线程读 NSTextView 全文并回调 `f(text)`。
    ///
    /// run_on_main_thread 异步排队(非阻塞)，无法把文本同步回传给调用方，故 do_save
    /// 须把「emit result / mark_saved / 关窗」全放进 `f` 内，在主线程闭包里一并完成
    /// (plan Task 6 标注的「排队(异步)」分支)。`f` 收到 owned String，无生命周期耦合。
    pub fn with_text<F>(app: &tauri::AppHandle, f: F)
    where
        F: FnOnce(String) + Send + 'static,
    {
        let Some(window) = app.get_window(WINDOW_LABEL) else {
            log::warn!("[native] with_text: window {WINDOW_LABEL} not found");
            return;
        };
        let _ = window.run_on_main_thread(move || {
            let guard = STATE.lock().unwrap();
            let Some(s) = guard.as_ref() else {
                log::warn!("[native] with_text: STATE not set");
                return;
            };
            let text = s.text_view.string().to_string(); // Retained<NSString> → String(impl Display)
            f(text);
        });
    }

}

#[cfg(target_os = "macos")]
pub use imp::*;
