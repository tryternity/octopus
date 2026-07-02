//! macOS 原生 compact editor：NSWindow + 工具栏(NSView/NSButton) + NSScrollView/NSTextView。
//! 无 webview。objc2 写法复用 spike/pin_window 验证模式。非 macOS 不编译本文件内容，
//! 回退 webview(见 compact_editor_window.rs 分流)。

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, NSObject};
    use objc2::{define_class, msg_send, sel, ClassType, MainThreadMarker};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSColor, NSControl, NSEvent,
        NSEventModifierFlags, NSFont, NSScrollView, NSTextField, NSTextView, NSView, NSWindow,
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
        char_label: Retained<NSTextField>,
        clear_button: Retained<NSButton>,
        /// find 按钮也作 Cmd+F 的 sender（tag=1 = NSFindPanelActionShow）。
        find_button: Retained<NSButton>,
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

    /// 字号 label sizeToFit 后在工具栏内垂直居中。
    /// NSTextField 默认文本顶对齐到 cell 顶部，比按钮标题（垂直居中）高出半行。
    fn recenter_label(label: &NSTextField, x: f64) {
        label.sizeToFit();
        let f = label.frame();
        label.setFrame(NSRect::new(
            NSPoint::new(x, (TOOLBAR_H - f.size.height) / 2.),
            f.size,
        ));
    }

    /// 更新工具栏字数统计（读 textview 全文 → char_label「N 字」）。
    fn update_char_count() {
        let guard = STATE.lock().unwrap();
        let Some(s) = guard.as_ref() else {
            return;
        };
        let n = s.text_view.string().to_string().chars().count();
        let x = s.char_label.frame().origin.x;
        s.char_label.setStringValue(&NSString::from_str(&format!("{n} 字")));
        recenter_label(&s.char_label, x);
    }

    // ── container 自定义 NSView：键盘快捷键 + 字数统计 delegate ──
    // container 作 contentView（响应链内），textview 是其子树。textview 原生不处理
    // undo/find 的按键等价，冒泡到 container 时拦截；其余交 super 让 textview 原生
    // 处理 Cmd+C/V/A 等。同时兼 textview 的 delegate（textDidChange: → 字数）。
    define_class!(
        #[unsafe(super(NSView))]
        #[name = "CompactEditorContainerView"]
        struct CompactEditorContainerView;

        impl CompactEditorContainerView {
            #[unsafe(method(performKeyEquivalent:))]
            fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
                let mods = event.modifierFlags();
                let cmd = mods.contains(NSEventModifierFlags::Command);
                let shift = mods.contains(NSEventModifierFlags::Shift);
                let code = event.keyCode();
                let app_of = || STATE.lock().unwrap().as_ref().map(|s| s.app.clone());

                // Cmd+Return(keyCode 36) → 保存
                if cmd && code == 36 {
                    if let Some(app) = app_of() {
                        crate::compact_editor_commands::do_save(&app);
                    }
                    return Bool::YES;
                }
                // Esc(keyCode 53) → 取消
                if code == 53 {
                    if let Some(app) = app_of() {
                        crate::compact_editor_commands::do_cancel(&app);
                    }
                    return Bool::YES;
                }
                if cmd {
                    let key = event
                        .charactersIgnoringModifiers()
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    match key.as_str() {
                        "z" if !shift => {
                            with_tv(|tv| {
                                if let Some(u) = tv.undoManager() {
                                    u.undo();
                                }
                            });
                            return Bool::YES;
                        }
                        "z" if shift => {
                            with_tv(|tv| {
                                if let Some(u) = tv.undoManager() {
                                    u.redo();
                                }
                            });
                            return Bool::YES;
                        }
                        "f" => {
                            // find_button.tag()=1 = Show；作 sender 传给 performFindPanelAction
                            let g = STATE.lock().unwrap();
                            if let Some(s) = g.as_ref() {
                                unsafe {
                                    let _: () = msg_send![
                                        &*s.text_view,
                                        performFindPanelAction: &*s.find_button
                                    ];
                                }
                            }
                            return Bool::YES;
                        }
                        _ => {}
                    }
                }
                // 其余交 super：NSView 默认转发到子树 textview，处理 Cmd+C/V/A 等
                unsafe { msg_send![super(self), performKeyEquivalent: event] }
            }

            #[unsafe(method(textDidChange:))]
            fn text_did_change(&self, _notif: &AnyObject) {
                update_char_count();
            }
        }
    );

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
            s.font_label.setStringValue(&NSString::from_str(&format!("{}", new as i64)));
            let lx = s.font_label.frame().origin.x;
            recenter_label(&s.font_label, lx);
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
    /// bezeled=false：扁平无边框（工具按钮）；true：Push 立体（主/次动作如保存/取消）。
    fn make_button(
        mtm: MainThreadMarker,
        title: &str,
        tag: isize,
        bezeled: bool,
        target: &CompactEditorButtonTarget,
        frame: NSRect,
    ) -> Retained<NSButton> {
        let btn = NSButton::new(mtm);
        btn.setTitle(&NSString::from_str(title));
        btn.setFrame(frame);
        btn.setTag(tag);
        if bezeled {
            btn.setBezelStyle(NSBezelStyle::Push);
        } else {
            btn.setBordered(false);
            btn.setWantsLayer(true);
        }
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
            // container 作为 contentView：setContentView 会把它 resize 到填满 content rect
            // （保证尺寸正确——默认 contentView 在 attach 闭包跑时 frame 可能仍是 0×0，
            // 据它算坐标会让所有子视图 frame 归零/变负而不可见）。
            let container: Retained<CompactEditorContainerView> =
                unsafe { msg_send![CompactEditorContainerView::class(), new] };
            ns_win.setContentView(Some(&container));
            // contentLayoutRect = 标题栏以下的可用区。Tauri 原生窗为 fullSizeContent，
            // contentView 含标题栏区（顶部 ~32px 被标题栏盖住）。工具栏须钉在可用区顶部，
            // 否则落在标题栏遮挡区不可见。
            let lr = ns_win.contentLayoutRect();
            let cw = lr.size.width;
            let uh = lr.size.height; // 可用高度（不含标题栏）
            log::info!("[native] attach: layoutRect {:.0}×{:.0}", cw, uh);

            // 工具栏：可用区顶部 TOOLBAR_H；MaxYMargin 钉住与 contentView 顶（=标题栏底）的间距，
            // resize 时仍贴在标题栏正下方。
            let toolbar = NSView::new(mtm);
            let toolbar_y = uh - TOOLBAR_H;
            toolbar.setFrame(NSRect::new(
                NSPoint::new(0., toolbar_y),
                NSSize::new(cw, TOOLBAR_H),
            ));
            toolbar.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
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
                false,
                &target,
                NSRect::new(NSPoint::new(step(38., 2.), y), NSSize::new(38., bh)),
            );
            let redo = make_button(
                mtm,
                "重做",
                TAG_REDO,
                false,
                &target,
                NSRect::new(NSPoint::new(step(38., 8.), y), NSSize::new(38., bh)),
            );
            let fdec = make_button(
                mtm,
                "−",
                TAG_FONT_DEC,
                false,
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
            recenter_label(&font_label, step(26., 0.));
            let finc = make_button(
                mtm,
                "+",
                TAG_FONT_INC,
                false,
                &target,
                NSRect::new(NSPoint::new(step(22., 8.), y), NSSize::new(22., bh)),
            );
            let find = make_button(
                mtm,
                "查找",
                TAG_FIND,
                false,
                &target,
                NSRect::new(NSPoint::new(step(40., 6.), y), NSSize::new(40., bh)),
            );
            let clear = make_button(
                mtm,
                "清空",
                TAG_CLEAR,
                false,
                &target,
                NSRect::new(NSPoint::new(step(44., 6.), y), NSSize::new(44., bh)),
            );
            // 右侧组：保存(扁平+蓝色 accent 强调主动作，右贴边) / 取消(扁平)
            let save = make_button(
                mtm,
                "保存",
                TAG_SAVE,
                false,
                &target,
                NSRect::new(NSPoint::new(cw - 6. - 56., y), NSSize::new(56., bh)),
            );
            save.setContentTintColor(Some(&NSColor::controlAccentColor()));
            let cancel = make_button(
                mtm,
                "取消",
                TAG_CANCEL,
                false,
                &target,
                NSRect::new(NSPoint::new(cw - 6. - 56. - 6. - 52., y), NSSize::new(52., bh)),
            );

            // 字数统计 label（取消左侧）
            let char_label = NSTextField::new(mtm);
            char_label.setBezeled(false);
            char_label.setDrawsBackground(false);
            char_label.setEditable(false);
            char_label.setSelectable(false);
            char_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            recenter_label(&char_label, cw - 6. - 56. - 6. - 52. - 6. - 44.);

            for btn in [&undo, &redo, &fdec, &finc, &find, &clear, &cancel, &save] {
                toolbar.addSubview(btn);
            }
            toolbar.addSubview(&font_label);
            toolbar.addSubview(&char_label);

            // 文本区（占 toolbar 以下）
            let text_view = NSTextView::new(mtm);
            text_view.setRichText(false);
            text_view.setFont(Some(&NSFont::systemFontOfSize(font_size)));
            text_view.setEditable(true);
            text_view.setSelectable(true);
            text_view.setUsesFindBar(true);
            text_view.setAllowsUndo(true); // 默认 false：不设则 typing 不进 undo 栈，撤销/重做无效

            // 查找按钮改直达 textview：performFindPanelAction: 读 sender.tag，
            // tag=1(NSFindPanelActionShow) 才弹 find bar（nil/tag=0 无动作）。
            find.setTag(1);
            unsafe {
                let _: () = msg_send![&*find, setTarget: &*text_view];
                let _: () = msg_send![&*find, setAction: sel!(performFindPanelAction:)];
            }

            // container 兼 textview delegate：textDidChange: → 字数统计
            unsafe {
                let _: () = msg_send![&*text_view, setDelegate: &*container];
            }

            let scroll = NSScrollView::new(mtm);
            scroll.setFrame(NSRect::new(
                NSPoint::new(0., 0.),
                NSSize::new(cw, toolbar_y),
            ));
            scroll.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            scroll.setDocumentView(Some(&text_view));
            scroll.setHasVerticalScroller(true);
            scroll.setAutoresizesSubviews(true);

            container.addSubview(&toolbar);
            container.addSubview(&scroll);

            // 存 STATE（reopen 时覆盖，旧的在主线程闭包内 drop，安全）
            let clear_btn = clear.clone();
            let find_btn = find.clone();
            *STATE.lock().unwrap() = Some(SendState {
                app: app.clone(),
                text_view,
                font_label,
                char_label,
                clear_button: clear_btn,
                find_button: find_btn,
                target,
            });
            update_char_count(); // 初始字数
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
            // setString 不触发 textDidChange；同一锁内顺手刷字数（避免 update_char_count 重入死锁）
            let clx = s.char_label.frame().origin.x;
            let n = text.chars().count();
            s.char_label.setStringValue(&NSString::from_str(&format!("{n} 字")));
            recenter_label(&s.char_label, clx);
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
