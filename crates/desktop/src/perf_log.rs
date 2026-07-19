//! ASR Result 窗可观测性日志工具：写 `~/.octopus/logs/asr.log`。
//!
//! 承载两类调用方（共用同一文件，按 msg prefix 区分）：
//!
//! 1. **阈值性能日志**（不稳定卡顿取证）：调用方自行做阈值过滤，避免每帧写盘放大开销。
//!    - `[BE tick]`：`pipeline.rs` tick 总耗时 > 30ms 时记
//!    - `[FE writeDoc]`：`AsrEditor.tsx` writeDoc dispatch > 8ms 或文本 > 800 字时记
//!    - 这部分是临时的，根因定位 + 修复完成后可移除。
//!
//! 2. **状态机诊断日志**（无阈值过滤，每事件必记；tick 详情 1Hz 节流）：让"绿条为何不亮"
//!    "识别为何停""commit 后为何不恢复"这类不稳定复现问题可事后翻日志复盘
//!    （详见 spec 2026-07-19-asr-edit-stall-observability）。
//!    - `[STATE]`：editing 状态翻转（enter / commit / toggle / cancel / discard）
//!    - `[HEARTBEAT]`：tick 线程心跳（1Hz 节流，证明线程在跑 + 当前 stage/editing）
//!    - `[SPEAKING]`：VAD 说话状态翻转 + emit（pipeline 触发 + coordinator emit）
//!    - `[FE]`：前端关键事件（enter/commit/isSpeaking/isRecording 翻转）
//!    - `[WARN]`：异常路径（dispatch_tick stage 不匹配等）
//!    - `[POLISH]`：润色状态机（take_polish_input / polish_apply / on_polish_failed /
//!      PolishDone 各分支 / PolishNow / auto-trigger），验证"polish_pending 残留 / 编辑被
//!      polish_apply 覆盖"等假设
//!    - `[TICK-DETAIL]`：tick 详情（1Hz 节流，pipeline-local / pipeline-vad-seg 两路径），
//!      含 silence/has_speech/speaking/changed/events/samples——验证"VAD 冻结 + 灌 5 秒静音"
//!      导致绿条延迟亮
//!    - `[APPLY]`：`transcript.rs::apply_engine_full` 关键分支（is_prefix / diverted /
//!      polish_pending / delta_len / cum_len / shown_len），验证"engine_cumulative 与 segments
//!      失配"假设
//!    - `[CARET]`：`caret_gap` 落点变化（set_caret / set_selection / commit_edit /
//!      push_delta_at_caret / polish_apply），验证"新文本插到中插但前端看不见"假设
//!    - 这部分是长期保留的可观测性，与阈值日志区分。
//!
//! **crate 边界**：asr-local 不依赖 desktop（架构反向），故 `streaming_runner.rs` 的 runner
//! 内部状态（seen_speech / flushed）用 `log::debug!` 写 stderr；desktop 层 pipeline.rs 的
//! `[TICK-DETAIL]` 写文件，二者互补。如未来需要 asr-local 也写文件，把 perf_log 提升到 infra。
//!
//! 设计要点：
//! - 轻量 append + 单 Mutex，IO 错误静默吞（打点不得影响业务或反成卡顿源）。
//! - 时间戳本地时区毫秒，便于人眼对账（不稳定复现 → 事后翻日志）。
//! - 调用方决定是否阈值过滤（性能日志过滤、诊断日志必记、tick 详情节流），本模块不强制。

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
