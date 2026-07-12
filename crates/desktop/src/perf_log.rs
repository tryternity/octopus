//! 临时性能打点工具：写 `~/.octopus/logs/asr.log`，供 ASR Result 窗卡顿取证。
//!
//! 设计要点：
//! - 轻量 append + 单 Mutex，IO 错误静默吞（打点不得影响业务或反成卡顿源）。
//! - 时间戳本地时区毫秒，便于人眼对账（不稳定复现 → 事后翻日志）。
//! - 调用方自行做阈值过滤（仅超阈值才记），避免每帧写盘放大开销。
//!
//! 根因定位 + 修复完成后可整体移除。

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

use chrono::Local;

static LOCK: Mutex<()> = Mutex::new(());

fn log_path() -> std::path::PathBuf {
    let dir = dirs::home_dir()
        .map(|h| h.join(".octopus").join("logs"))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/octopus-logs"));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("asr.log")
}

/// 写一行性能日志：`<本地时间毫秒> <msg>`。任何 IO 错误静默吞。
pub fn log(msg: &str) {
    let ts = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let line = format!("{} {}\n", ts, msg);
    let _g = LOCK.lock();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 前端 invoke 入口（fire-and-forget）：前端打点经此写盘。
#[tauri::command]
pub fn perf_log_cmd(msg: String) {
    log(&msg);
}
