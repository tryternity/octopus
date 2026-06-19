//! DashScope 云端 ASR Realtime 流式会话（VAD-gated per-utterance streaming）。
//!
//! 支持两套阿里云端协议，通过 endpoint 路径自动分发：
//! - **Fun-ASR / Paraformer**（`/api-ws/v1/inference`）：任务型协议
//!   （`run-task` → 二进制 PCM → `finish-task` → `result-generated`）
//! - **Qwen-ASR Realtime**（`/api-ws/v1/realtime`）：OpenAI Realtime 风格会话协议
//!   （`session.update` → base64 PCM via `input_audio_buffer.append` → `session.finish`）
//!
//! 与 `engine_dashscope.rs` 的 chunk 模式（每段 VAD 开一条新 WS）不同，本模块维护
//! 一条长连接 WS，由 coordinator 的 VAD 逻辑管理连接生命周期：
//! - 语音 onset → [`DashScopeStreamSession::open`]：建连 + 初始化 + 推 ~100ms pre-roll
//! - 持续语音 → [`DashScopeStreamSession::push_pcm`]：推 PCM 帧
//! - 静音 ≥ `pause_polish_threshold_ms` → [`DashScopeStreamSession::close`]：结束 + 收最终结果
//!
//! ## 异步模型
//!
//! coordinator 运行在 `std::thread`（非 tokio runtime）。WS 是 async（tokio-tungstenite），
//! 故本会话在 `tauri::async_runtime`（tokio handle）上 spawn 一条 tokio task 跑
//! `tokio::select!` 双向循环：收 PCM → send / 收 WS text → 发 result event。
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
    /// 建连 + 初始化 + 推 pre-roll PCM + 启动后台 WS task。
    ///
    /// 根据 `endpoint` 路径自动选择协议：
    /// - 含 `/v1/realtime` → Qwen-ASR Realtime 会话协议（OpenAI Realtime 风格）
    /// - 否则 → Fun-ASR/Paraformer 任务型协议（run-task/finish-task）
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

        let is_qwen = is_qwen_realtime_endpoint(&endpoint);
        rt.spawn(async move {
            let result = if is_qwen {
                run_qwen_realtime_session(
                    pcm_rx, result_tx, endpoint, key, model, language, pre_roll_samples,
                ).await
            } else {
                run_ws_session(
                    pcm_rx, result_tx, endpoint, key, model, language, pre_roll_samples,
                ).await
            };
            if let Err(e) = result {
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

    /// 非阻塞发送 Finish 信号（finish-task / session.finish），不等待结果。
    ///
    /// 后续 WS task 收到服务端最终结果后通过 `try_recv_text()` 返回
    /// `StreamEvent::Text`（最终文本）+ `StreamEvent::Finished`。
    /// coordinator 在后续 tick 中 drain 这些事件。
    pub fn finish(&self) -> Result<()> {
        self.pcm_tx
            .send(PcmFrame::Finish)
            .map_err(|_| anyhow!("dashscope PCM channel closed"))
    }

    /// 非阻塞取 partial 文本（如果有新的）。
    pub fn try_recv_text(&mut self) -> Option<StreamEvent> {
        self.result_rx.try_recv().ok()
    }

    /// 结束会话 + 阻塞等最终结果（仅用于 Toggle/stop 路径）。
    ///
    /// **不要在 tick handler 中调用**——`block_on` 会阻塞 coordinator 线程。
    /// tick handler 应使用 `finish()`（非阻塞），结果通过 `try_recv_text()` 异步获取。
    ///
    /// - Fun-ASR 协议：发 `finish-task`
    /// - Qwen-ASR 协议：发 `session.finish`
    ///
    /// 返回最终累积文本（可能为空）。
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
    // Fun-ASR 在一个 task 内可能发多句 result-generated。根据文档：
    // - sentence_id 从 1 递增，标识当前句子
    // - sentence_end=true 表示该句最终结果（之后 sentence_id 会递增）
    // - heartbeat=true 时 sentence_id=0，应跳过
    // - text 是该句的累积文本（中间结果可能被修订，最终结果在 sentence_end=true 时确定）
    let mut committed: String = String::new(); // 已完成的句子
    let mut current_sentence: String = String::new(); // 当前句子的累积文本
    let mut current_sentence_id: i64 = -1; // 当前句子 ID（-1 = 尚未收到）
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
                                let sentence = &v["payload"]["output"]["sentence"];
                                // 跳过心跳包（heartbeat=true, sentence_id=0）
                                let heartbeat = sentence["heartbeat"].as_bool().unwrap_or(false);
                                if heartbeat {
                                    continue;
                                }
                                let text = sentence["text"].as_str().unwrap_or("");
                                let sentence_id = sentence["sentence_id"].as_i64().unwrap_or(0);
                                let sentence_end = sentence["sentence_end"].as_bool().unwrap_or(false);

                                // sentence_id 变化 = 新句开始，提交前一句
                                if sentence_id != current_sentence_id && current_sentence_id > 0 {
                                    if !current_sentence.is_empty() {
                                        if !committed.is_empty() && !committed.ends_with('，') {
                                            committed.push('，');
                                        }
                                        committed.push_str(&current_sentence);
                                        current_sentence.clear();
                                    }
                                }
                                current_sentence_id = sentence_id;
                                current_sentence = text.to_string();

                                // sentence_end=true = 该句最终结果，立即提交
                                if sentence_end {
                                    if !committed.is_empty() && !committed.ends_with('，') {
                                        committed.push('，');
                                    }
                                    committed.push_str(&current_sentence);
                                    current_sentence.clear();
                                    current_sentence_id = -1; // 等下一个新句
                                }

                                let combined = format!("{}{}", committed, current_sentence);
                                log::info!(
                                    "[FunASR-Stream] sid={} end={} text={:?} combined={:?}",
                                    sentence_id, sentence_end, text, combined
                                );
                                let _ = result_tx.send(StreamEvent::Text(combined));
                            }
                            Some("task-finished") => {
                                // 提交未提交的最后一句
                                if !current_sentence.is_empty() {
                                    if !committed.is_empty() && !committed.ends_with('，') {
                                        committed.push('，');
                                    }
                                    committed.push_str(&current_sentence);
                                    current_sentence.clear();
                                }
                                log::info!("[FunASR-Stream] task-finished total={:?}", committed);
                                if !committed.is_empty() {
                                    let _ = result_tx.send(StreamEvent::Text(committed.clone()));
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

// ── Qwen-ASR Realtime 协议（OpenAI Realtime 风格）──

/// 判断 endpoint 是否为 Qwen-ASR Realtime 端点。
///
/// Qwen-ASR Realtime 使用 `/api-ws/v1/realtime` 路径，
/// 而 Fun-ASR/Paraformer 使用 `/api-ws/v1/inference` 路径。
pub(crate) fn is_qwen_realtime_endpoint(endpoint: &str) -> bool {
    endpoint.contains("/v1/realtime") || endpoint.contains("/realtime?")
}

/// 构造 Qwen-ASR Realtime 的 `session.update` 事件 JSON。
///
/// 配置：
/// - 音频格式 `pcm`，采样率 16000
/// - 语言：`auto`/空 → 不指定（服务端自动检测）；否则指定 `language`
/// - 服务端 VAD（`server_vad`）：threshold=0.5（默认灵敏度），silence_duration_ms=600
///   （略短于 coordinator 的 pause_polish_threshold，让服务端先出完整句）
fn build_qwen_session_update(language: &str, event_id: &str) -> Value {
    let transcription = if language.is_empty() || language == "auto" {
        json!({})
    } else {
        json!({ "language": language })
    };
    json!({
        "event_id": event_id,
        "type": "session.update",
        "session": {
            "input_audio_format": "pcm",
            "sample_rate": 16000,
            "input_audio_transcription": transcription,
            "turn_detection": {
                "type": "server_vad",
                "threshold": 0.5,
                "silence_duration_ms": 600,
            }
        }
    })
}

/// 生成 Qwen-ASR event_id（`evt_` + UUIDv4 简写）。
fn qwen_event_id() -> String {
    format!("evt_{}", uuid::Uuid::new_v4().simple())
}

/// s16le PCM 字节 → base64 编码字符串（Qwen-ASR `input_audio_buffer.append` 要求）。
fn pcm_s16le_to_base64(pcm: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(pcm)
}

/// Qwen-ASR Realtime 会话主逻辑：建连 → session.update → pre-roll → 双向循环 → session.finish → 收结果。
///
/// 与 `run_ws_session`（Fun-ASR 任务型协议）的关键区别：
/// - URL：模型名通过查询参数 `?model=<name>` 传递，而非 payload
/// - 鉴权：`Authorization: Bearer <key>`（注意大写 B）
/// - 音频传输：base64 编码的 PCM 封装在 JSON `input_audio_buffer.append` 事件中（文本帧），
///   而非二进制 WS 帧
/// - 结束：发 `session.finish`（而非 `finish-task`），等服务端回 `session.finished`
/// - 结果提取：`conversation.item.input_audio_transcription.text`（partial：text+stash）
///   和 `.completed`（final：transcript）
async fn run_qwen_realtime_session(
    mut pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    endpoint: String,
    key: String,
    model: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<()> {
    // 1. 构造完整 URL（追加 ?model= 查询参数）
    let url = if endpoint.contains("?model=") || endpoint.contains("&model=") {
        endpoint.clone()
    } else if endpoint.contains('?') {
        format!("{}&model={}", endpoint, model)
    } else {
        format!("{}?model={}", endpoint, model)
    };

    // 2. 建连 + Authorization: Bearer <key>
    let mut request = url
        .as_str()
        .into_client_request()
        .with_context(|| format!("qwen-asr WS 请求构造失败: {}", url))?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {}", key)
            .parse()
            .context("qwen-asr Authorization header 构造失败")?,
    );
    let (mut ws, _resp) = connect_async(request)
        .await
        .with_context(|| format!("qwen-asr WS 连接失败: {}", url))?;

    // 3. 发 session.update（配置音频格式 + VAD）
    let session_update = build_qwen_session_update(&language, &qwen_event_id());
    ws.send(Message::Text(session_update.to_string()))
        .await
        .context("qwen-asr WS 发送 session.update 失败")?;

    // 4. 推 pre-roll PCM（base64 编码）
    if !pre_roll_samples.is_empty() {
        let pcm = crate::engine_dashscope::samples_to_pcm_s16le(&pre_roll_samples);
        let b64 = pcm_s16le_to_base64(&pcm);
        let append = json!({
            "event_id": qwen_event_id(),
            "type": "input_audio_buffer.append",
            "audio": b64,
        });
        ws.send(Message::Text(append.to_string()))
            .await
            .context("qwen-asr WS 发送 pre-roll PCM 失败")?;
    }

    // 5. 双向循环
    let mut accumulated_text = String::new();
    loop {
        tokio::select! {
            // 收 PCM 指令
            frame = pcm_rx.recv() => {
                match frame {
                    Some(PcmFrame::Samples(pcm)) => {
                        let b64 = pcm_s16le_to_base64(&pcm);
                        let append = json!({
                            "event_id": qwen_event_id(),
                            "type": "input_audio_buffer.append",
                            "audio": b64,
                        });
                        ws.send(Message::Text(append.to_string()))
                            .await
                            .context("qwen-asr WS 发送 PCM append 失败")?;
                    }
                    Some(PcmFrame::Finish) => {
                        let finish = json!({
                            "event_id": qwen_event_id(),
                            "type": "session.finish",
                        });
                        ws.send(Message::Text(finish.to_string()))
                            .await
                            .context("qwen-asr WS 发送 session.finish 失败")?;
                    }
                    None => break,
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
                        let event_type = v["type"].as_str().unwrap_or("");
                        match event_type {
                            // partial 结果（高频流式）：text（已确认前缀）+ stash（预测后缀）
                            // partial 只含当前句，需拼上已完成句的 accumulated_text
                            "conversation.item.input_audio_transcription.text" => {
                                let text = v["text"].as_str().unwrap_or("");
                                let stash = v["stash"].as_str().unwrap_or("");
                                let partial = format!("{}{}", text, stash);
                                let combined = format!("{}{}", accumulated_text, partial);
                                log::info!(
                                    "[Qwen-Stream] partial text={:?} stash={:?} combined={:?}",
                                    text, stash, combined
                                );
                                if !combined.is_empty() {
                                    let _ = result_tx.send(StreamEvent::Text(combined));
                                }
                            }
                            // 最终结果（per-utterance）：累积 transcript
                            "conversation.item.input_audio_transcription.completed" => {
                                if let Some(t) = v["transcript"].as_str() {
                                    log::info!(
                                        "[Qwen-Stream] completed transcript={:?} prev_accumulated={:?}",
                                        t, accumulated_text
                                    );
                                    if !t.is_empty() {
                                        if !accumulated_text.is_empty()
                                            && !accumulated_text.ends_with('，')
                                        {
                                            accumulated_text.push('，');
                                        }
                                        accumulated_text.push_str(t);
                                        let _ = result_tx.send(
                                            StreamEvent::Text(accumulated_text.clone()),
                                        );
                                    }
                                }
                            }
                            // 会话结束：服务端已完成所有识别
                            "session.finished" => {
                                if !accumulated_text.is_empty() {
                                    let _ = result_tx.send(StreamEvent::Text(
                                        accumulated_text.clone(),
                                    ));
                                }
                                let _ = result_tx.send(StreamEvent::Finished);
                                break;
                            }
                            // 错误事件
                            "error" => {
                                let msg = v["error"]["message"]
                                    .as_str()
                                    .or_else(|| v["error"]["code"].as_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| v["error"].to_string());
                                let _ = result_tx.send(StreamEvent::Failed(msg));
                                break;
                            }
                            _ => {} // session.created/updated, speech_started/stopped 等忽略
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwen_endpoint_detection() {
        assert!(is_qwen_realtime_endpoint(
            "wss://dashscope.aliyuncs.com/api-ws/v1/realtime"
        ));
        assert!(is_qwen_realtime_endpoint(
            "wss://dashscope.aliyuncs.com/api-ws/v1/realtime?model=test"
        ));
        assert!(!is_qwen_realtime_endpoint(
            "wss://dashscope.aliyuncs.com/api-ws/v1/inference"
        ));
    }

    #[test]
    fn qwen_session_update_structure() {
        let update = build_qwen_session_update("zh", "evt_test");
        assert_eq!(update["type"], "session.update");
        assert_eq!(update["event_id"], "evt_test");
        assert_eq!(update["session"]["input_audio_format"], "pcm");
        assert_eq!(update["session"]["sample_rate"], 16000);
        assert_eq!(
            update["session"]["input_audio_transcription"]["language"],
            "zh"
        );
        assert_eq!(update["session"]["turn_detection"]["type"], "server_vad");
        assert_eq!(update["session"]["turn_detection"]["threshold"], 0.5);
        assert_eq!(
            update["session"]["turn_detection"]["silence_duration_ms"],
            600
        );
    }

    #[test]
    fn qwen_session_update_auto_language_omits_language_field() {
        let update = build_qwen_session_update("auto", "evt_1");
        assert!(
            update["session"]["input_audio_transcription"]
                .get("language")
                .is_none(),
            "auto 语言不应指定 language 字段"
        );
    }

    #[test]
    fn pcm_base64_roundtrip() {
        let pcm = vec![0x01u8, 0x00, 0xFF, 0x7F, 0x80, 0x00];
        let b64 = pcm_s16le_to_base64(&pcm);
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let decoded = STANDARD.decode(&b64).unwrap();
        assert_eq!(decoded, pcm);
    }

    #[test]
    fn qwen_event_id_has_prefix() {
        let id = qwen_event_id();
        assert!(id.starts_with("evt_"), "event_id 应以 evt_ 开头: {}", id);
        assert!(id.len() > 10, "event_id 应有足够长度: {}", id);
    }
}
