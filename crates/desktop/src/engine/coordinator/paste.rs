//! 粘贴 + DB helpers + 通用工具函数（从 coordinator/mod.rs 提取，Task 1.1）。
//!
//! 这里集中了两类函数：
//! - **通用工具**：`now_millis` / `active_asr_engine_name` / `active_llm_name` /
//!   `sync_runtime_fields` / `stage_name`——被 mod.rs 和其他子模块多处调用。
//! - **粘贴/落库**：`do_paste`（粘贴 + 终翻 + 剪贴板写入）/ `update_transcription_raw`
//!   （过程落库 INSERT/UPDATE 节流）。

use crate::core::config::AppConfig;
use crate::core::db_queue::{DbCommand, get_db_sender};
use crate::engine::transcript::Transcript;
use log::{info, warn, error};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use tauri::{Emitter, Manager};
use super::{
    Command, Stage, DB_FLUSH_INTERVAL_MS, FALLBACK_STREAMING_SPEC, TRANSLATION_ACTIVE,
    INSTANT_MODE,
};

/// 当前 Unix 毫秒时间戳（作 Transcript id / DB 主键）。
pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 取激活 ASR 引擎名（裸名）。resolve 失败（含未激活 + 兜底失败）时返回兜底 spec。
///
/// Task 2 后：coordinator 不再依赖 config.asr_engine 字段（已删），统一调此函数。
pub(crate) fn active_asr_engine_name() -> String {
    octopus_asr_local::config::resolve_active_engine("asr")
        .map(|r| r.name)
        .unwrap_or_else(|_| FALLBACK_STREAMING_SPEC.to_string())
}

/// 取激活 LLM 引擎名（裸名）。resolve 失败（未激活）时返回空串。
///
/// Task 2 后：coordinator 不再依赖 config.polish_llm 字段（已删），统一调此函数。
/// 用于 DB 记录的 model 字段——空串表示无润色 LLM 激活。
pub(crate) fn active_llm_name() -> String {
    octopus_asr_local::config::resolve_active_engine("llm")
        .map(|r| r.name)
        .unwrap_or_default()
}

/// 把共享 AppConfig 的运行时可变字段同步到 coordinator 的 config 快照。
///
/// 与 Toggle 时的同步逻辑共用，确保两条路径同步内容一致。
/// 不含 `asr_engine`（需重建引擎实例，只能 Toggle 时切），也不含 `denoise_mode`
/// （音频处理路径有独立 cfg 读取，会话中切换影响降噪器状态）。
pub(crate) fn sync_runtime_fields(config: &mut AppConfig, shared: &AppConfig) {
    config.polish_mode = shared.polish_mode;
    config.asr_correct = shared.asr_correct;
    config.output_simplified = shared.output_simplified;
    config.hide_toolbar = shared.hide_toolbar;
    config.edit_shortcut = shared.edit_shortcut.clone();
}

pub(crate) fn stage_name(stage: &Stage) -> &'static str {
    match stage {
        Stage::Idle => "Idle",
        Stage::Streaming { .. } => "Streaming",
        Stage::VadSegmented { .. } => "VadSegmented",
        Stage::WaitingCompletion { .. } => "WaitingCompletion",
        Stage::StoppingPolish { .. } => "StoppingPolish",
        Stage::Polishing { .. } => "Polishing",
        Stage::Pasting { .. } => "Pasting",
        #[cfg(feature = "cloud")]
        Stage::CloudClosing { .. } => "CloudClosing",
    }
}

/// 执行真正的粘贴落库操作（在主线程进行）
#[allow(clippy::too_many_arguments)]
pub(crate) fn do_paste(
    stage: &mut Stage,
    text_to_paste: &str,
    id: i64,
    raw_text: &str,
    segments: &str,
    polish_status: &str,
    config: &AppConfig,
    app_handle: &tauri::AppHandle,
    tx: &Sender<Command>,
) {
    // 翻译模式：润色完成后（或跳过润色），对最终文本同步翻译，粘贴译文。
    // swap 消费确保只翻译一次（多个 do_paste 调用只有首个触发）。
    let text_to_paste_owned: String;
    let text_to_paste = if TRANSLATION_ACTIVE.swap(false, Ordering::Relaxed) {
        if INSTANT_MODE.load(Ordering::Relaxed) {
            crate::ui::instant_overlay::show_instant_overlay(app_handle, "polishing", "");
        } else {
            crate::ui::result_window::show_result(app_handle, "⏳ 最终翻译中...");
        }
        // catch_unwind 兜底：do_translate 调模型加载（ort/candle）与 LLM 网络，
        // panic 会杀 coordinator 线程导致整个状态机失效（同 start_final_polish_or_paste 的加固）。
        // do_translate 已 async 化（云端引擎走 HTTP）——coordinator 非 tokio 线程，
        // 用 tauri::async_runtime::block_on 进入（cloud_pipeline.rs:122 同模式，不可新建 Runtime）。
        let text_ref = text_to_paste;
        text_to_paste_owned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tauri::async_runtime::block_on(
                crate::action_bar::action_bar_commands::do_translate(text_ref, config)
            )
        }))
        .unwrap_or_else(|p| {
            let msg = if let Some(s) = p.downcast_ref::<&str>() { (*s).to_string() }
                else if let Some(s) = p.downcast_ref::<String>() { s.clone() }
                else { "unknown panic".to_string() };
            warn!("终翻 panic: {}", msg);
            Err(msg)
        })
        .unwrap_or_else(|e| {
            warn!("最终翻译失败，回退润色/原文: {}", e);
            text_ref.to_string()
        });
        info!("Translation finalize: {} chars", text_to_paste_owned.chars().count());
        &text_to_paste_owned
    } else {
        text_to_paste
    };

    // instant 模式：用 instant 浮窗 "done" 态展示最终文本（PasteDone 后 hide）。
    // 非 instant：正常 show_result（用户可编辑结果窗）。
    if INSTANT_MODE.load(Ordering::Relaxed) {
        crate::ui::instant_overlay::show_instant_overlay(app_handle, "done", text_to_paste);
    } else {
        crate::ui::result_window::show_result(app_handle, text_to_paste);
    }

    *stage = Stage::Pasting {
        id,
        raw_text: raw_text.to_string(),
        segments: segments.to_string(),
        polished_text: if polish_status == "done" {
            text_to_paste.to_string()
        } else {
            String::new()
        },
        polish_status: polish_status.to_string(),
    };

    let config = config.clone();
    let tx_inner = tx.clone();
    let clipboard_handle = app_handle
        .state::<std::sync::Arc<octopus_clipboard::ClipboardHandle>>()
        .inner()
        .clone();
    let text_to_paste = text_to_paste.to_string();

    let app_handle_emit = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let res = tokio::task::spawn_blocking(move || {
            // 录音过程中 insert_transcription_at_id 已在 clipboard_history 创建了 voice 条目（id=tid）。
            // paste 时只需 touch_created_at 把它顶到列表顶部，不重复创建。
            // tid=0（sentinel，无有效会话）时跳过——cancel 后 paste 的边界场景。
            let touched = if id > 0 {
                octopus_infra::db::with_db(|conn| {
                    octopus_clipboard::store::touch_created_at(conn, id)
                })
            } else {
                Ok(())
            };
            if let Err(e) = &touched {
                warn!("Clipboard history touch_created_at failed: {}", e);
            }

            // ASR 记录已入库：主动广播 clipboard://changed。paste 路径写剪贴板时
            // 会设 suppress_flag，watcher 的 on_clipboard_change 命中
            // check_and_clear_suppress 后直接 return（不调 on_change 闭包），
            // emit 不会自然触发——前端浮窗/设置面板收不到通知，ASR 记录无法即时渲染。
            if touched.is_ok() {
                let _ = app_handle_emit.emit("clipboard://changed", ());
            }

            crate::platform::paste::paste(&text_to_paste, &clipboard_handle, &config, &app_handle_emit)
        }).await;

        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => error!("Paste failed: {}", e),
            Err(e) => error!("Paste task panicked: {:?}", e),
        }
        let _ = tx_inner.send(Command::PasteDone);
    });
}

/// 首次有文本 INSERT，否则 UPDATE raw_text。DB 失败返回 Err 供调用方 warn（不阻塞识别）。
/// 用 Transcript.db_inserted() 区分首次与后续（避免「UPDATE 0 行无法判断」歧义）。
pub(crate) fn update_transcription_raw(
    transcript: &mut Transcript,
    engine: &str,
    engine_mode: &str,
) -> Result<(), String> {
    if transcript.full().is_empty() {
        return Ok(());
    }
    let sender = get_db_sender();
    if !transcript.db_inserted() {
        let cmd = DbCommand::Insert {
            id: transcript.id,
            text: transcript.db_text(),
            segments: transcript.segments_json(),
            engine: engine.to_string(),
            engine_mode: Some(engine_mode.to_string()),
        };
        sender.send(cmd).map_err(|e| format!("Queue DB insert failed: {}", e))?;
        transcript.mark_db_inserted();
        transcript.mark_db_written();
    } else {
        // 落库节流：距上次落库 < DB_FLUSH_INTERVAL_MS 则跳过本次 UPDATE（Finalize 兜底完整写入，
        // 最坏落后一帧文本；避免长录音每 changed≈每 tick 一次 UPDATE 的写放大）。
        if !transcript.db_flush_due(std::time::Duration::from_millis(DB_FLUSH_INTERVAL_MS)) {
            return Ok(());
        }
        let cmd = DbCommand::UpdateTextSegments {
            id: transcript.id,
            text: transcript.db_text(),
            segments: transcript.segments_json(),
        };
        sender
            .send(cmd)
            .map_err(|e| format!("Queue DB update_text_segments failed: {}", e))?;
        transcript.mark_db_written();
    }
    Ok(())
}
