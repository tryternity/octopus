//! 判别 OCR 僵死根因：N 线程**独立**调 ocr_rs MNN `OcrEngine::new`（绕过我们的
//! OnceLock 单例），看 MNN C++ 并发首次初始化是否死锁。
//!
//! ⚠️ 必须独占进程跑（MNN 全局未被先加载），否则测的是「并发二次 new」无判别力——
//! 单独的 test bin 即保证独占，别和 ocr_smoke.rs 同进程跑。
//!
//! - 死锁（30s 超时未返回）→ 坐实「MNN 并发初始化」是 OCR 僵死根因；main 用
//!   check-then-set 不 hang 只是运气好没触发并发，DCL 修复对症。
//! - 正常返回 → 并发不是因，worktree 僵死另有原因，DCL 是误打误撞，需回头查
//!   clean-used-feature 到底改了什么让首加载卡住。
//!
//! 运行：`cargo test -p octopus-ocr --test ocr_concurrent_smoke -- --ignored --nocapture`

use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use octopus_ocr::model;

#[test]
#[ignore]
fn concurrent_raw_engine_new_does_not_hang() {
    let dir = model::model_dir(model::DEFAULT_OCR_MODEL);
    let det = dir.join("det.mnn");
    let rec = dir.join("rec.mnn");
    let keys = dir.join("keys.txt");
    assert!(
        model::is_model_ready(model::DEFAULT_OCR_MODEL),
        "默认 OCR 模型未就绪：{}（先下载模型再跑此判别测试）",
        dir.display()
    );

    const N: usize = 4;
    let barrier = Arc::new(Barrier::new(N));
    let (tx, rx) = mpsc::channel();

    for i in 0..N {
        let (barrier, tx) = (barrier.clone(), tx.clone());
        let (det, rec, keys) = (det.clone(), rec.clone(), keys.clone());
        std::thread::spawn(move || {
            // Barrier 同步起跑，最大化「并发首次」——4 线程同时进 MNN 初始化。
            barrier.wait();
            let started = std::time::Instant::now();
            let res = ocr_rs::engine::OcrEngine::new(&det, &rec, &keys, None);
            let _ = tx.send((i, res.is_ok(), started.elapsed()));
        });
    }
    drop(tx);

    for i in 0..N {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok((tid, ok, dur)) => {
                eprintln!("[conc] thread {} done ok={} in {:?}", tid, ok, dur);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!(
                    "[conc] 第 {} 个完成未在 30s 内返回（{} 线程并发 new 超时）→ \
                     MNN 并发初始化死锁坐实，「并发」是 OCR 僵死根因",
                    i, N
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!(
                    "[conc] 第 {} 个完成前 channel 断开（共 {} 线程）：有线程 panic/crash 未 send → \
                     MNN 并发初始化崩溃，「并发」是 OCR 僵死根因",
                    i, N
                );
            }
        }
    }
    eprintln!(
        "[conc] 全部 {} 线程并发 new() 正常返回 → MNN 并发初始化不死锁，\
         「并发」不是 OCR 僵死根因，DCL 是误打误撞，需另查 worktree 改动",
        N
    );
}
