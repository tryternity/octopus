//! Mock helper：测试用的假 helper 二进制。
//! 按 argv[1] 解析 RecordingRequest，按 stdin 命令回放事件流。
//! 真实 helper 是 Swift 写的，无法在 Rust 测试里用，这个 mock 验证主进程的协议处理。

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

    // 1. emit Ready
    emit(&[("event", "ready"), ("schema_version", "1")]);

    // 2. emit RecordingStarted（用请求里的 width/height）
    let req: serde_json::Value = serde_json::from_str(&req_json).unwrap();
    let width = req["video"]["width"].as_u64().unwrap_or(1920);
    let height = req["video"]["height"].as_u64().unwrap_or(1080);
    emit(&[
        ("event", "recording-started"),
        ("timestamp_ms", "1000"),
        ("width", &width.to_string()),
        ("height", &height.to_string()),
    ]);

    // 3. 读 stdin 命令流
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
