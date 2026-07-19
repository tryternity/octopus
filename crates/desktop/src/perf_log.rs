//! ASR Result 窗可观测性日志工具：写 `~/.octopus/logs/asr.log`。
//!
//! 承载两类调用方（共用同一文件，按 msg prefix 区分）：
//!
//! 1. **阈值性能日志**（不稳定卡顿取证）：调用方自行做阈值过滤，避免每帧写盘放大开销。
//!    - `[BE tick]`：`pipeline.rs` tick 总耗时 > 30ms 时记
//!    - `[FE writeDoc]`：`AsrEditor.tsx` writeDoc dispatch > 8ms 或文本 > 800 字时记
//!    - 这部分是临时的，根因定位 + 修复完成后可移除。
//!
//! 2. **状态机诊断日志**（无阈值过滤，每事件必记）：让"绿条为何不亮""识别为何停"
//!    这类不稳定复现问题可事后翻日志复盘（详见 spec 2026-07-19-asr-edit-stall-observability）。
//!    - `[STATE]`：editing 状态翻转（enter / commit / toggle / cancel / discard）
//!    - `[HEARTBEAT]`：tick 线程心跳（1Hz 节流，证明线程在跑 + 当前 stage/editing）
//!    - `[SPEAKING]`：VAD 说话状态翻转 + emit
//!    - `[FE]`：前端关键事件（enter/commit/isSpeaking/isRecording 翻转）
//!    - `[WARN]`：异常路径（dispatch_tick stage 不匹配等）
//!    - 这部分是长期保留的可观测性，与阈值日志区分。
//!
//! 设计要点：
//! - 轻量 append + 单 Mutex，IO 错误静默吞（打点不得影响业务或反成卡顿源）。
//! - 时间戳本地时区毫秒，便于人眼对账（不稳定复现 → 事后翻日志）。
//! - 调用方决定是否阈值过滤（性能日志过滤、诊断日志必记），本模块不强制。

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
