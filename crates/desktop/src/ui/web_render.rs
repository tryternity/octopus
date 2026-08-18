//! URL 渲染 fallback（spec 2026-08-18-url-to-markdown §4）：离屏 WKWebView 加载
//! SPA 页面 → readyState 轮询 → settle → outerHTML。仅 macOS。
//!
//! 线程模型（零 NavigationDelegate、零跨线程属性读）：
//! - 调用方（spawn_blocking 线程）持 mpsc channel + 20s deadline 循环；
//! - 主线程经 run_on_main_thread 创建/加载/evaluate；
//! - 重试与 settle 的延时由本函数所在线程 sleep 后回投主线程实现
//!   （spec §4 的 dispatch_after 等效替代，零 GCD 依赖）。
//!
//! unsafe 契约（slot 不变式）：`slot: Arc<AtomicUsize>` 仅存
//! `Retained<WKWebView>::into_raw` 的裸指针（0 = 无 webview）。WKWebView 非
//! Send/Sync，跨线程仅以 usize 形态传递；**裸指针只在主线程**经
//! `Retained::from_raw` 取回、同步使用后立即 `into_raw` 放回（见
//! `with_webview` / `cleanup_on_main`）。所有指针解引用都发生在
//! run_on_main_thread 派发的闭包内（主线程串行，无交错）；监控线程只持有
//! Arc 本体、从不解引用指针值。completion block 的 `*mut AnyObject` /
//! `*mut NSError` 参数按 ObjC block 惯例为 +0 借用（block 执行期间由调用方
//! WKWebView 保活），**禁止 `Retained::from_raw` 接管**（会过度释放）——
//! 仅以 `&*` 引用形态在 block 内使用。

#![cfg(target_os = "macos")]

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_foundation::{NSError, NSString, NSURL, NSURLRequest};
use objc2_web_kit::WKWebView;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// SPA 渲染总预算（秒）。
pub const RENDER_TIMEOUT_SECS: u64 = 20;
/// readyState 完成后的 settle（毫秒）——懒加载/字体。
pub const RENDER_SETTLE_MS: u64 = 2000;
/// readyState 轮询间隔（毫秒）——同时是 recv 超时节拍（探针重投 cadence）。
const READY_POLL_MS: u64 = 250;

/// readyState 探针：complete 时返回 outerHTML（非 null JS 结果），否则 null。
/// HTML 不经探针通道回传——settle 后的 final evaluate 是唯一成功出口。
const READY_PROBE_JS: &str =
    "(document.readyState === 'complete') ? document.documentElement.outerHTML : null";
/// settle 后的最终取值（成功出口的 JS）。
const OUTER_HTML_JS: &str = "document.documentElement.outerHTML";

/// 主线程 → 监控线程的一次性信号。
enum Signal {
    /// readyState 探针命中（complete）——仅节拍，HTML 由 final evaluate 送。
    Ready,
    /// 最终 outerHTML（唯一成功出口）。
    Html(String),
    /// JS 执行错误 / 主线程初始化失败。
    Failed(String),
}

/// 离屏 WKWebView 渲染 URL → outerHTML（spec §2 决策树的渲染 fallback）。
/// 阻塞调用（≤ `RENDER_TIMEOUT_SECS` + settle 余量），供 spawn_blocking 线程使用。
pub fn render_html(app: &tauri::AppHandle, url: &str) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<Signal>();
    let done = Arc::new(AtomicBool::new(false));
    // slot 在 render_html 层创建：create 闭包 / 探针重投 / final evaluate / cleanup
    // 各自 clone 捕获（brief「执行者注意」：slot 提升到本层共享）。
    let slot = Arc::new(AtomicUsize::new(0));
    let url_owned = url.to_string();

    // ── 创建 + 加载 + 首次探针（主线程）──
    let tx_create = tx.clone();
    let done_create = done.clone();
    let slot_create = slot.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            let _ = tx_create.send(Signal::Failed("主线程标记获取失败".into()));
            return;
        };
        // SAFETY: mtm 证明在主线程（WKWebView 必须主线程创建）；离屏（不 attach
        // window），加载与 JS 执行由 Tauri 的 NSApplication 主 runloop 驱动。
        let webview = unsafe { WKWebView::new(mtm) };
        let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(&url_owned)) else {
            // webview 随闭包结束 drop（Retained 引用计数归零，主线程释放）
            let _ = tx_create.send(Signal::Failed("非法 URL".into()));
            return;
        };
        let request = NSURLRequest::requestWithURL(&nsurl);
        // SAFETY: request 为有效引用；返回的 Retained<WKNavigation> 立即 drop
        //（一次 release）不影响已发起的导航。
        let _ = unsafe { webview.loadRequest(&request) };
        // SAFETY: into_raw 裸指针经 slot 传递——见模块 unsafe 契约。
        slot_create.store(Retained::into_raw(webview) as usize, Ordering::SeqCst);
        probe_once(&slot_create, tx_create, done_create);
    });

    // ── 监控循环（本线程）：recv 超时 = 探针重投节拍；deadline 兜底退出 ──
    let deadline = Instant::now() + Duration::from_secs(RENDER_TIMEOUT_SECS);
    let mut settled = false; // 首次 Ready 后不再探针——final evaluate 已在途
    loop {
        if Instant::now() >= deadline {
            cleanup_on_main(app, &slot, &done);
            return Err("渲染超时（SPA 页面）".into());
        }
        match rx.recv_timeout(Duration::from_millis(READY_POLL_MS)) {
            Ok(Signal::Html(h)) => {
                cleanup_on_main(app, &slot, &done);
                return Ok(h);
            }
            Ok(Signal::Failed(e)) => {
                cleanup_on_main(app, &slot, &done);
                return Err(format!("渲染失败: {}", e));
            }
            Ok(Signal::Ready) => {
                if settled {
                    continue; // 在途探针重复命中——final evaluate 已在途，不重投
                }
                settled = true;
                // settle（懒加载/字体）后回投主线程取最终 outerHTML（唯一成功出口）
                std::thread::sleep(Duration::from_millis(RENDER_SETTLE_MS));
                let tx_final = tx.clone();
                let done_final = done.clone();
                let slot_final = slot.clone();
                let _ = app.run_on_main_thread(move || {
                    evaluate_final_html(&slot_final, tx_final, done_final);
                });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !settled {
                    // 未 ready：回投主线程再探针（监控方节拍驱动重试——等效 dispatch_after）
                    let tx_probe = tx.clone();
                    let done_probe = done.clone();
                    let slot_probe = slot.clone();
                    let _ = app.run_on_main_thread(move || {
                        probe_once(&slot_probe, tx_probe, done_probe);
                    });
                }
                // settled：final evaluate 在途，继续等 Html/Failed 或 deadline
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                cleanup_on_main(app, &slot, &done);
                return Err("渲染失败: 通道关闭".into());
            }
        }
    }
}

/// 从 slot 取回 webview 执行 `f` 后立即放回（unsafe 契约：仅主线程调用）。
/// slot 为 0（create 未跑 / cleanup 已回收）时跳过返回 None。
fn with_webview<R>(slot: &Arc<AtomicUsize>, f: impl FnOnce(&WKWebView) -> R) -> Option<R> {
    let raw = slot.swap(0, Ordering::SeqCst);
    if raw == 0 {
        return None;
    }
    // SAFETY: raw 只能来自 create 闭包的 Retained::into_raw（+1 所有权）；
    // swap 取出后主线程独占（本函数仅由 run_on_main_thread 派发闭包调用，
    // 主线程串行无交错），f 同步执行期间无其他 slot 写者，用毕立即 into_raw
    // 放回；若 f panic 则 Retained 随 unwind drop（不泄漏、不二次释放）。
    let webview = unsafe { Retained::from_raw(raw as *mut WKWebView) }
        .expect("slot 指针非空（raw != 0 已检查，from_raw 仅对 null 返回 None）");
    let out = f(&webview);
    slot.store(Retained::into_raw(webview) as usize, Ordering::SeqCst);
    Some(out)
}

/// 单次 readyState 探针（仅主线程）。completion 语义：
/// - JS 结果非 null（readyState complete）→ `Signal::Ready`（HTML 不经探针回传）；
/// - JS 错误 → `Signal::Failed`；
/// - 结果 null（未 complete）→ 不发信号，监控方超时节拍重投。
/// completion block 不做延时——延时由监控方 sleep 后回投（brief 契约）。
fn probe_once(slot: &Arc<AtomicUsize>, tx: mpsc::Sender<Signal>, done: Arc<AtomicBool>) {
    let block: RcBlock<dyn Fn(*mut AnyObject, *mut NSError)> =
        RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
            if done.load(Ordering::SeqCst) {
                return; // 监控方已退出——不再发信号
            }
            if !error.is_null() {
                // SAFETY: block 参数为 +0 借用——error 在 block 执行期间由调用方
                //（WKWebView）保活；仅在本 block 内引用，不越界持有、不接管所有权。
                let err: &NSError = unsafe { &*error };
                let _ = tx.send(Signal::Failed(err.localizedDescription().to_string()));
                return;
            }
            if result.is_null() {
                return; // readyState 未 complete——等监控方下个节拍重投
            }
            let _ = tx.send(Signal::Ready);
        });
    let _ = with_webview(slot, |webview| {
        // SAFETY: completion block 为合法堆分配 ObjC block（WKWebView 按惯例 copy
        // 持有至回调后释放）；JS 字符串为有效 NSString 引用。
        unsafe {
            webview.evaluateJavaScript_completionHandler(
                &NSString::from_str(READY_PROBE_JS),
                Some(&block),
            );
        }
    });
}

/// settle 后的最终取值（仅主线程）：completion 直发 `Signal::Html`（唯一成功出口）
/// 或 `Signal::Failed`。与 `probe_once` 同构，仅 JS 与成功语义不同。
fn evaluate_final_html(slot: &Arc<AtomicUsize>, tx: mpsc::Sender<Signal>, done: Arc<AtomicBool>) {
    let block: RcBlock<dyn Fn(*mut AnyObject, *mut NSError)> =
        RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
            if done.load(Ordering::SeqCst) {
                return; // 监控方已退出——不再发信号
            }
            if !error.is_null() {
                // SAFETY: 同 probe_once——+0 借用，block 执行期间调用方保活。
                let err: &NSError = unsafe { &*error };
                let _ = tx.send(Signal::Failed(err.localizedDescription().to_string()));
                return;
            }
            if result.is_null() {
                let _ = tx.send(Signal::Failed("outerHTML 返回空".into()));
                return;
            }
            // SAFETY: 同上——+0 借用；JS 字符串结果为 NSString，引用期间 to_string
            // 完成深拷贝即弃引用。
            let html: &NSString = unsafe { &*result.cast::<NSString>() };
            let _ = tx.send(Signal::Html(html.to_string()));
        });
    let _ = with_webview(slot, |webview| {
        // SAFETY: 同 probe_once。
        unsafe {
            webview.evaluateJavaScript_completionHandler(
                &NSString::from_str(OUTER_HTML_JS),
                Some(&block),
            );
        }
    });
}

/// 回收 webview（主线程 drop——WKWebView 主线程 dealloc）+ 置 done 让在途
/// completion block 早退。run_on_main_thread 失败（app 退出中）时 webview
/// 随进程终止回收，监控方仍正常返回。
fn cleanup_on_main(app: &tauri::AppHandle, slot: &Arc<AtomicUsize>, done: &AtomicBool) {
    done.store(true, Ordering::SeqCst);
    let slot = slot.clone();
    let _ = app.run_on_main_thread(move || {
        let raw = slot.swap(0, Ordering::SeqCst);
        if raw != 0 {
            // SAFETY: raw 只能来自 create 闭包的 Retained::into_raw（+1 所有权）；
            // swap 独占取出后 drop（主线程，符合 WKWebView dealloc 线程要求），
            // slot 归 0 后不再有解引用者（with_webview 见 0 即跳过）。
            drop(unsafe { Retained::from_raw(raw as *mut WKWebView) });
        }
    });
}
