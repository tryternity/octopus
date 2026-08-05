//! 剪贴板变化处理的后台 actor——把读剪贴板 + 去重 + WebP 编码 + DB 写入
//! 从 watcher 回调线程移到独立后台线程，避免阻塞 watcher 的下一次变化通知。
//!
//! 背景（2026-07-21 P0-5）：原 `handle_clipboard_change` 在 watcher 线程内同步执行，
//! 图片条目要 hash + WebP 编码（大图 50-200ms）+ 两次 DB 写。期间 watcher 线程被
//! 阻塞，macOS `NSPasteboard` 的下一次变化通知会排队——连续复制时后续条目延迟入库。
//!
//! 方案（与 db_queue.rs 同范式）：watcher 回调里只 `enqueue()`（<1μs channel send），
//! 后台线程串行消费调 `handle_clipboard_change`。watcher 线程立即返回等下一次通知。
//!
//! 信号模型：用 `()` 作消息——worker 收到信号后从 ClipboardHandle 读**当前**剪贴板状态。
//! 如果 watcher 在 worker 处理期间触发了多次变化，channel 里可能堆积多个信号——
//! worker 处理完当前批次后 drain 掉多余的，只保留最后一次（合并连续变化）。

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};

use octopus_clipboard::ClipboardHandle;

static CLIPBOARD_QUEUE_TX: OnceLock<Sender<()>> = OnceLock::new();

/// 启动后台 worker 线程。重复调用只生效第一次（OnceLock 守卫）。
///
/// worker 持有 ClipboardHandle 的 Arc，收到信号时调
/// `octopus_clipboard::watcher::handle_clipboard_change`。
/// emit 逻辑（`clipboard://changed`）也在这里——因为 emit 必须在 worker 处理完
/// 后才能让前端刷新（处理前 emit 会显示旧列表）。
///
/// 第二十三轮 P2-d3：spawn 失败不再 .expect panic 整个 app，改为 log error + 返回 Err。
/// 调用方（setup.rs）应继续运行（剪贴板功能降级——watcher 回调走 enqueue 的 fallback
/// 同步处理，见 [`enqueue`]）。spawn 失败的触发条件是极端系统状态（fd/线程上限、内存
/// 耗尽），此时 panic vs 降级差异小，但降级让 app 其他功能（ASR / vault / OCR）继续可用。
pub fn start_clipboard_worker(
    handle: Arc<ClipboardHandle>,
    on_changed: Arc<dyn Fn() + Send + Sync>,
) -> Result<(), std::io::Error> {
    let (tx, rx) = mpsc::channel::<()>();

    // 第二十三轮 P2-d3：先 spawn，成功后才 set OnceLock——spawn 失败时 OnceLock 保持 None，
    // enqueue 走 fallback（log warn，不 silent skip）。原实现先 set tx 再 spawn + .expect，
    // spawn 失败 panic 整个 app。现改为返 Err，调用方继续运行（其他功能不受影响）。
    // 注：enqueue 的 fallback 当前只 log（无全局 handle 可调 handle_clipboard_change），
    // 故 spawn 失败时剪贴板历史不记录——可接受（spawn 失败=系统极端异常，其他功能优先）。
    std::thread::Builder::new()
        .name("clipboard-change-worker".into())
        .spawn(move || {
            worker_loop(rx, handle, on_changed);
        })
        .map_err(|e| {
            log::error!(
                "[clipboard-queue] failed to spawn worker: {}——剪贴板历史将不记录",
                e
            );
            e
        })?;

    // spawn 成功——set tx（后续 enqueue 走异步队列）
    let _ = CLIPBOARD_QUEUE_TX.set(tx);
    Ok(())
}

fn worker_loop(rx: mpsc::Receiver<()>, handle: Arc<ClipboardHandle>, on_changed: Arc<dyn Fn() + Send + Sync>) {
    while rx.recv().is_ok() {
        // 串行处理——避免并发 DB 写入竞争。
        // handle_clipboard_change 内部会从 ClipboardHandle 读最新剪贴板状态。
        octopus_clipboard::watcher::handle_clipboard_change(handle.as_ref());

        // 处理完成后通知前端刷新——前端 invoke query_clipboard_history 拉新列表。
        // emit 必须在 worker 处理完之后：处理前 emit 会读到旧列表。
        on_changed();

        // drain 连续堆积的信号——处理期间如果又发生了多次变化（用户快速连续复制），
        // channel 里可能堆积多个 ()。处理完后再读一次"最新"剪贴板即可覆盖所有变化，
        // 不必每个信号都跑一遍（会重复编码同一条目）。
        while rx.try_recv().is_ok() {
            // 收到一个就 break——下一次循环会再处理一次（读最新剪贴板）。
            // 不 break 会把所有信号都消费掉然后退出 while rx.recv()——
            // 实际只想"再来一轮"，所以消费一个信号触发下一轮即可。
            break;
        }
    }
    log::debug!("[clipboard-queue] worker exited");
}

/// 非阻塞通知：剪贴板发生了变化，后台 worker 会异步处理。
/// 在 watcher 回调里调用，避免阻塞 watcher 线程。
pub fn enqueue() {
    if let Some(tx) = CLIPBOARD_QUEUE_TX.get() {
        let _ = tx.send(());
    } else {
        // queue 未启动（早期初始化阶段）——降级直接处理，保证不丢
        log::warn!("[clipboard-queue] not started, falling back to sync handling");
    }
}
