//! 决定性二分：直接调 OCR 引擎（真实 MNN 模型 + 纯色 PNG），绕过 Tauri/前端/DB。
//!
//! - 若此 test hang（不返回）→ 问题在 OCR 引擎层（MNN 编译/链接/初始化死锁）
//! - 若此 test 正常返回 → 问题在 Tauri/前端/DB 调用层
//!
//! 运行：`cargo test -p octopus-ocr --test ocr_smoke -- --ignored --nocapture`

use std::sync::{Arc, Barrier};

use octopus_ocr::engine::OcrEngine;

#[test]
#[ignore]
fn engine_loads_and_recognizes_without_hang() {
    eprintln!("[smoke] before instance()");
    let engine = OcrEngine::instance().expect("OCR instance should load");
    eprintln!("[smoke] after instance() — MNN 加载成功");

    // 纯色 PNG（无文字，recognize 返回空，但触发完整 det+rec 推理路径）
    let img = image::RgbaImage::from_raw(320, 64, vec![255u8; 320 * 64 * 4]).unwrap();
    let dyn_img = image::DynamicImage::ImageRgba8(img);
    let mut buf = Vec::new();
    dyn_img
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    eprintln!("[smoke] before recognize()");
    let text = engine.recognize(&buf).expect("recognize should not hang");
    eprintln!("[smoke] after recognize() — text='{}'", text);
}

/// DCL 正确性：N 线程同时调 `instance()`，确认都成功返回 + 拿到同一引擎（Arc 指针相等）。
/// 证明 double-checked locking 串行化首次加载且模型只加载一次。
/// 运行：`cargo test -p octopus-ocr --test ocr_smoke instance_concurrent -- --ignored --nocapture`
#[test]
#[ignore]
fn instance_concurrent_returns_same_engine() {
    const N: usize = 8;
    let barrier = Arc::new(Barrier::new(N));
    let handles: Vec<_> = (0..N)
        .map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                OcrEngine::instance().map(|e| Arc::as_ptr(&e) as usize)
            })
        })
        .collect();

    let mut ptrs: Vec<usize> = Vec::new();
    for h in handles {
        let ptr = h
            .join()
            .expect("instance 线程 panic")
            .expect("instance() 失败");
        ptrs.push(ptr);
    }
    ptrs.sort();
    ptrs.dedup();
    assert_eq!(
        ptrs.len(),
        1,
        "并发 instance() 应返回同一引擎，实际 {} 个不同指针",
        ptrs.len()
    );
    eprintln!("[smoke-conc] {} 线程并发 instance() 全部返回同一引擎 ✓", N);
}

