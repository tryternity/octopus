//! DashScope FunASR Realtime 流式会话（VAD-gated per-utterance streaming）。
//!
//! 与 `engine_dashscope.rs` 的 chunk 模式（每段 VAD 开一条新 WS）不同，本模块维护
//! 一条长连接 WS，由 coordinator 的 VAD 逻辑管理连接生命周期：
//! - 语音 onset → [`DashScopeStreamSession::open`]：建连 + run-task + 推 ~100ms pre-roll
//! - 持续语音 → [`DashScopeStreamSession::push_pcm`]：推 PCM 帧
//! - 静音 ≥ `pause_polish_threshold_ms` → [`DashScopeStreamSession::close`]：finish-task + 收最终结果
//!
//! ## 异步模型
//!
//! coordinator 运行在 `std::thread`（非 tokio runtime）。WS 是 async（tokio-tungstenite），
//! 故本会话在 `tauri::async_runtime`（tokio handle）上 spawn 一条 tokio task 跑
//! `tokio::select!` 双向循环：收 PCM → send binary / 收 WS text → 发 result event。
//! coordinator 通过同步 channel 非阻塞收 partial（`try_recv`），close 时阻塞等最终结果。

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::client::IntoClientRequest,
    tungstenite::http::header::AUTHORIZATION,
    tungstenite::Message,
};

/// PCM 帧指令：coordinator → 后台 WS task
enum PcmFrame {
    /// 推 PCM 样本（s16le bytes）
    Samples(Vec<u8>),
    /// 发 finish-task + 关闭发送端
    Finish,
}

/// 后台 reader 发给 coordinator 的事件。
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// partial / final 识别文本（累积句文本，每次覆盖取最新）
    Text(String),
    /// 服务端 task-finished（最终结果已到位，连接可关闭）
    Finished,
    /// task-failed（错误信息）
    Failed(String),
}

/// DashScope 流式会话句柄。
///
/// 持有 PCM sender（供 coordinator 推音频）和 result receiver（取识别文本）。
/// 后台一条 tokio task 管理 WS 连接的双向收发。
pub struct DashScopeStreamSession {
    pcm_tx: mpsc::UnboundedSender<PcmFrame>,
    result_rx: mpsc::UnboundedReceiver<StreamEvent>,
}

impl DashScopeStreamSession {
    /// 建连 + 发 run-task + 推 pre-roll PCM + 启动后台 WS task。
    ///
    /// `rt` 是 tauri 全局 runtime handle，用于 spawn async task。
    /// `pre_roll_samples` 是 f32[-1,1] 样本（~100ms 前导音频），为 ASR 提供声学上下文。
    pub fn open(
        rt: &tauri::async_runtime::RuntimeHandle,
        endpoint: String,
        key: String,
        model: String,
        language: String,
        pre_roll_samples: Vec<f32>,
    ) -> Result<Self> {
        let (pcm_tx, pcm_rx) = mpsc::unbounded_channel::<PcmFrame>();
        let (result_tx, result_rx) = mpsc::unbounded_channel::<StreamEvent>();

        rt.spawn(async move {
            if let Err(e) = run_ws_session(pcm_rx, result_tx, endpoint, key, model, language, pre_roll_samples).await {
                log::error!("dashscope stream session error: {}", e);
            }
        });

        Ok(Self { pcm_tx, result_rx })
    }

    /// 推 PCM 样本（f32[-1,1] → s16le），非阻塞。
    pub fn push_pcm(&self, samples: &[f32]) -> Result<()> {
        let pcm = crate::engine_dashscope::samples_to_pcm_s16le(samples);
        self.pcm_tx
            .send(PcmFrame::Samples(pcm))
            .map_err(|_| anyhow!("dashscope PCM channel closed"))
    }

    /// 非阻塞取 partial 文本（如果有新的）。
    pub fn try_recv_text(&mut self) -> Option<StreamEvent> {
        self.result_rx.try_recv().ok()
    }

    /// 发 finish-task + 阻塞等最终结果。
    ///
    /// 返回最终累积文本（可能为空）。在 coordinator 线程（非 tokio）上调用，
    /// 用 tokio `block_on` 阻塞等 result channel 直到 Finished / Failed。
    pub fn close(self, rt: &tauri::async_runtime::RuntimeHandle) -> Result<String> {
        let _ = self.pcm_tx.send(PcmFrame::Finish);
        let mut rx = self.result_rx;
        rt.block_on(async move {
            let mut text = String::new();
            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::Text(t) => text = t,
                    StreamEvent::Finished => break,
                    StreamEvent::Failed(msg) => bail!("dashscope task-failed: {}", msg),
                }
            }
            Ok(text)
        })
    }
}

/// 后台 WS 会话主逻辑：建连 → run-task → pre-roll → 双向循环 → finish-task → 收结果。
async fn run_ws_session(
    mut pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    endpoint: String,
    key: String,
    model: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<()> {
    // 1. 建连
    let mut request = endpoint
        .as_str()
        .into_client_request()
        .context("dashscope WS 请求构造失败")?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("bearer {}", key)
            .parse()
            .context("dashscope Authorization header 构造失败")?,
    );
    let (mut ws, _resp) = connect_async(request)
        .await
        .with_context(|| format!("dashscope WS 连接失败: {}", endpoint))?;

    // 2. 发 run-task（含 max_sentence_silence=600，比客户端 700ms 短，让服务端先出完整句）
    let task_id = uuid::Uuid::new_v4().to_string();
    let run_task = build_run_task_streaming(&model, &language, &task_id);
    ws.send(Message::Text(run_task.to_string()))
        .await
        .context("dashscope WS 发送 run-task 失败")?;

    // 3. 推 pre-roll PCM
    if !pre_roll_samples.is_empty() {
        let pcm = crate::engine_dashscope::samples_to_pcm_s16le(&pre_roll_samples);
        ws.send(Message::binary(pcm))
            .await
            .context("dashscope WS 发送 pre-roll PCM 失败")?;
    }

    // 4. 双向循环
    let mut accumulated_text = String::new();
    loop {
        tokio::select! {
            // 收 PCM 指令
            frame = pcm_rx.recv() => {
                match frame {
                    Some(PcmFrame::Samples(pcm)) => {
                        ws.send(Message::binary(pcm))
                            .await
                            .context("dashscope WS 发送 PCM 帧失败")?;
                    }
                    Some(PcmFrame::Finish) => {
                        let finish_task = json!({
                            "header": {
                                "action": "finish-task",
                                "task_id": task_id,
                                "streaming": "duplex",
                            },
                            "payload": { "input": {} }
                        });
                        ws.send(Message::Text(finish_task.to_string()))
                            .await
                            .context("dashscope WS 发送 finish-task 失败")?;
                    }
                    None => break, // coordinator drop → 关闭
                }
            }
            // 收 WS 消息
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        let v: Value = match serde_json::from_str(&t) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        match v["header"]["event"].as_str() {
                            Some("result-generated") => {
                                if let Some(text) = v["payload"]["output"]["sentence"]["text"].as_str() {
                                    accumulated_text = text.to_string();
                                    let _ = result_tx.send(StreamEvent::Text(accumulated_text.clone()));
                                }
                            }
                            Some("task-finished") => {
                                if !accumulated_text.is_empty() {
                                    let _ = result_tx.send(StreamEvent::Text(accumulated_text.clone()));
                                }
                                let _ = result_tx.send(StreamEvent::Finished);
                                break;
                            }
                            Some("task-failed") => {
                                let msg = v["header"]["error_message"]
                                    .as_str()
                                    .or_else(|| v["header"]["error_code"].as_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| v["header"].to_string());
                                let _ = result_tx.send(StreamEvent::Failed(msg));
                                break;
                            }
                            _ => {} // task-started 等忽略
                        }
                    }
                    Some(Ok(_)) => {} // binary 等忽略
                    Some(Err(e)) => {
                        let _ = result_tx.send(StreamEvent::Failed(format!("WS 读错误: {}", e)));
                        break;
                    }
                    None => break, // WS 关闭
                }
            }
        }
    }
    Ok(())
}

/// 构造 streaming 模式的 run-task（含 max_sentence_silence=600）。
fn build_run_task_streaming(model: &str, language: &str, task_id: &str) -> Value {
    let lang_hints = if language.is_empty() || language == "auto" {
        json!(["zh", "en"])
    } else {
        json!([language])
    };
    json!({
        "header": {
            "action": "run-task",
            "task_id": task_id,
            "streaming": "duplex",
        },
        "payload": {
            "model": model,
            "task_group": "audio",
            "task": "asr",
            "function": "recognition",
            "parameters": {
                "format": "pcm",
                "sample_rate": 16000,
                "language_hints": lang_hints,
                "max_sentence_silence": 600,
            },
            "input": {},
        }
    })
}
