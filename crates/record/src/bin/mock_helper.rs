//! Mock helper：测试用的假 helper 二进制。
//! 按 argv[1] 解析 RecordingRequest，按 stdin 命令回放事件流。
//! 真实 helper 是 Swift 写的，无法在 Rust 测试里用，这个 mock 验证主进程的协议处理。
//!
//! **故障场景**（通过 `MOCK_HELPER_MODE` 环境变量切换，测试用 wrapper 注入）：
//! - `no-started`（默认）：emit ready 后**不发** recording-started，模拟 SCK 不出帧
//!   → 父进程等不到事件超时。**验证 start() 超时后 reset_to_idle 把 state 回到 Idle**。
//! - `error`：emit ready + error 后 exit(1)，模拟 permissionDenied
//!   → 验证 HelperEvent::Error 让 wait_for_state 短路返回（不等 10s 超时）。
//! - `stderr-flood`：emit ready 后向 stderr 写 200KB（>64KB 管道缓冲），不发 started
//!   → 验证父进程 stderr reader task 防止 helper 阻塞（曾经没读导致 helper 卡死）。
//! - 不设 / `normal`：正常流程（ready → started → stdin 命令循环）。

use std::io::{BufRead, Write};

fn emit(fields: &[(&str, &str)]) {
    let mut map = serde_json::Map::new();
    for (k, v) in fields {
        // 数值字段直接放，字符串字段加引号
        if v.parse::<i64>().is_ok() || v.parse::<bool>().is_ok() {
            map.insert(k.to_string(), serde_json::Value::from(v.parse::<i64>().unwrap_or_default()));
        } else {
            map.insert(k.to_string(), serde_json::Value::from(*v));
        }
    }
    let line = serde_json::to_string(&map).unwrap();
    println!("{line}");
    std::io::stdout().flush().unwrap();
}

fn main() {
    // argv[1] 是 RecordingRequest JSON，解析它（验证主进程序列化）
    let req_json = std::env::args().nth(1).expect("missing argv[1]");
    let _req: serde_json::Value = serde_json::from_str(&req_json).expect("invalid request JSON");

    let mode = std::env::var("MOCK_HELPER_MODE").unwrap_or_else(|_| "normal".to_string());

    // 1. emit Ready（所有模式都发，让父进程知道 helper 起来了）
    emit(&[("event", "ready"), ("schema_version", "1")]);

    let req: serde_json::Value = serde_json::from_str(&req_json).unwrap();

    match mode.as_str() {
        "no-started" => {
            // emit ready 但不发 recording-started，hang 在 stdin 读
            // 父进程等不到 started → 10s 超时 → reset_to_idle SIGKILL 本进程
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let _ = line; // 阻塞直到被 SIGKILL
            }
        }
        "error" => {
            // emit error 后 exit(1)，模拟 permissionDenied / sourceNotFound
            emit(&[
                ("event", "error"),
                ("code", "permissionDenied"),
                ("message", "test: simulated permission denied"),
            ]);
            std::io::stdout().flush().unwrap();
            std::process::exit(1);
        }
        "stderr-flood" => {
            // 向 stderr 写 200KB（>64KB 管道缓冲），模拟 helper 输出过多日志
            // 若父进程不读 stderr，本进程会阻塞在 write(stderr) → 不发 started → 超时
            // 修复后父进程 spawn stderr reader task，本进程不阻塞，但仍不发 started（测 stderr 读取本身）
            let big = "X".repeat(1024);
            for _ in 0..200 {
                eprintln!("{big}");
            }
            let _ = std::io::stderr().flush();
            // 不发 started，hang 等 SIGKILL（与 no-started 同）
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let _ = line;
            }
        }
        _ => {
            // normal：标准流程
            let width = req["video"]["width"].as_u64().unwrap_or(1920);
            let height = req["video"]["height"].as_u64().unwrap_or(1080);
            emit(&[
                ("event", "recording-started"),
                ("timestamp_ms", "1000"),
                ("width", &width.to_string()),
                ("height", &height.to_string()),
            ]);

            // 读 stdin 命令流
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let cmd = line.unwrap();
                match cmd.trim() {
                    "pause" => emit(&[("event", "recording-paused"), ("timestamp_ms", "2000")]),
                    "resume" => emit(&[("event", "recording-resumed"), ("timestamp_ms", "3000")]),
                    "stop" => {
                        let path = req["outputs"]["screen_path"].as_str().unwrap_or("/tmp/x.mp4");
                        emit(&[
                            ("event", "recording-stopped"),
                            ("screen_path", path),
                            ("duration_ms", "30000"),
                            ("file_size", "1048576"),
                        ]);
                        std::process::exit(0);
                    }
                    _ => {}
                }
            }
        }
    }
}
