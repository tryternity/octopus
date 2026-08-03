//! ASR 识别结果的 DB 写入队列（后台 actor 模型）。
//!
//! 调用方通过 `get_db_sender().send(DbCommand::...)` 非阻塞写入，
//! 后台线程排空队列落库。`shutdown_db` 在应用退出前排空 + join。

use log::{info, warn};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) enum DbCommand {
    Insert {
        id: i64,
        text: String,
        segments: String,
        engine: String,
        engine_mode: Option<String>,
    },
    UpdateTextSegments {
        id: i64,
        text: String,
        segments: String,
    },
    UpdatePolished {
        id: i64,
        text: String,
        status: String,
        model: Option<String>,
        /// 润色后段 JSON（修复 I-1：写 segments 列保持「segments 是真相源」一致）。
        segments: String,
    },
    Finalize {
        id: i64,
        raw_text: String,
        segments: String,
        polished_text: Option<String>,
        polish_status: String,
        polish_model: Option<String>,
        duration_ms: Option<i64>,
    },
    UpdateEditedSegments {
        id: i64,
        text: String,
        segments: String,
    },
    /// 取消录音时删除未完成的 DB 记录（审查 Issue 6）
    Delete {
        id: i64,
    },
    /// 增量写 meta_info 单字段（诊断用，审查 #3/#4）。
    /// 走 DB 队列保证 FIFO：cloud_close_error 必须在 Insert 之后执行（异步 Insert 入队后，
    /// 同步 update_meta_field 会命中 0 行——走队列保证顺序）。
    /// 仅 cloud feature 下使用（finalize_cloud 的 enqueue_cloud_close_error），非 cloud 时
    /// 不存在——cfg gate 避免 dead_code warning。
    #[cfg(feature = "cloud")]
    UpdateMetaField {
        id: i64,
        key: String,
        value: String,
    },
}

static DB_SENDER: std::sync::OnceLock<std::sync::mpsc::Sender<DbCommand>> = std::sync::OnceLock::new();

/// 关机标志：置位后后台线程排空队列再退出（避免入队未处理的命令丢失）。
static DB_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// 后台写线程句柄（shutdown_db 用于 join，等待排空完成）。
/// 用 `Mutex<Option<>>` 包裹：`JoinHandle::join` 需要所有权，shutdown 时 take 出来 join。
static DB_HANDLE: std::sync::OnceLock<parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>> =
    std::sync::OnceLock::new();

/// 处理单条 DB 命令（主循环与关机排空共用）。
fn process_db_command(cmd: DbCommand) {
    match cmd {
        DbCommand::Insert { id, text, segments, engine, engine_mode } => {
            if let Err(e) = octopus_asr_local::db::insert_transcription_at_id(
                id,
                &text,
                &segments,
                &engine,
                engine_mode.as_deref(),
            ) {
                warn!("Background DB insert failed: {}", e);
            }
        }
        DbCommand::UpdateTextSegments { id, text, segments } => {
            if let Err(e) =
                octopus_asr_local::db::update_text_segments(id, &text, &segments)
            {
                warn!("Background DB update_text_segments failed: {}", e);
            }
        }
        DbCommand::UpdatePolished { id, text, status, model, segments } => {
            // text = 润色后扁平（落 text 列），与 segments 对应；polished_text 列已随段模型移除。
            if let Err(e) = octopus_asr_local::db::update_polished(
                id,
                &status,
                model.as_deref(),
                &segments,
                &text,
            ) {
                warn!("Background DB update_polished failed: {}", e);
            }
        }
        DbCommand::Finalize {
            id,
            raw_text,
            segments,
            polished_text,
            polish_status,
            polish_model,
            duration_ms,
        } => {
            // 段模型下 DB 只存 text（= finish_text 扁平）：润色 done 用 polished_text，否则 raw_text。
            let text = polished_text.as_deref().unwrap_or(&raw_text);
            if let Err(e) = octopus_asr_local::db::finalize_transcription(
                id,
                text,
                &segments,
                &polish_status,
                polish_model.as_deref(),
                duration_ms,
            ) {
                warn!("Background DB finalize failed: {}", e);
            }
        }
        DbCommand::UpdateEditedSegments { id, text, segments } => {
            if let Err(e) =
                octopus_asr_local::db::update_edited_segments(id, &text, &segments)
            {
                warn!("Background DB update_edited_segments failed: {}", e);
            }
        }
        DbCommand::Delete { id } => {
            if let Err(e) = octopus_infra::db::delete_transcriptions(&[id]) {
                warn!("Background DB delete failed: {}", e);
            }
        }
        #[cfg(feature = "cloud")]
        DbCommand::UpdateMetaField { id, key, value } => {
            if let Err(e) = octopus_infra::db::update_meta_field(id, &key, &value) {
                warn!("Background DB update_meta_field failed: {}", e);
            }
        }
    }
}

/// 排空队列中剩余命令（关机 / 断连后调用）。FIFO 顺序由 channel 保证。
fn drain_db_queue(rx: &std::sync::mpsc::Receiver<DbCommand>) {
    let mut drained = 0u32;
    while let Ok(cmd) = rx.try_recv() {
        process_db_command(cmd);
        drained += 1;
    }
    if drained > 0 {
        info!("DB drain: flushed {} queued command(s)", drained);
    }
}

/// 应用退出前调用：通知后台 DB 线程排空剩余命令并等待退出。
///
/// 背景：DB 写入为非阻塞 actor 模式（调用方 send 后即返回，真实落库在后台线程）。
/// 若不 drain，常见丢失路径为「录音结束 → Finalize 入队 → 用户立即退出 → 后台线程
/// 被进程 kill，队列里 Finalize 未落库」→ 该条记录停留在未 finalize 态。挂到 Tauri
/// `RunEvent::ExitRequested` 后即可保证关机前落库。仅当 actor 已初始化时才需要等待。
pub fn shutdown_db() {
    if DB_SENDER.get().is_some() {
        DB_SHUTDOWN.store(true, Ordering::SeqCst);
        if let Some(cell) = DB_HANDLE.get() {
            if let Some(handle) = cell.lock().take() {
                let _ = handle.join();
            }
        }
        info!("Background DB writer drained and joined");
    }
}

pub(crate) fn get_db_sender() -> &'static std::sync::mpsc::Sender<DbCommand> {
    DB_SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<DbCommand>();
        let handle = std::thread::spawn(move || {
            info!("Background DB writer thread started");
            loop {
                // 关机：先排空队列再退出（保留 FIFO 顺序的剩余命令）
                if DB_SHUTDOWN.load(Ordering::SeqCst) {
                    drain_db_queue(&rx);
                    break;
                }
                // recv_timeout：周期性唤醒以轮询关机标志（最长 200ms 延迟，退出场景可接受）
                match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                    Ok(cmd) => process_db_command(cmd),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // 所有 Sender drop（理论上不会发生，DB_SENDER 为 &'static）；
                        // 防御性排空后退出
                        drain_db_queue(&rx);
                        break;
                    }
                }
            }
            info!("Background DB writer thread exiting");
        });
        let _ = DB_HANDLE.set(parking_lot::Mutex::new(Some(handle)));
        tx
    })
}
