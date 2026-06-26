//! DashScope 云端 ASR Realtime 流式会话（VAD-gated per-utterance streaming）。
//!
//! 支持两套阿里云端协议，通过 endpoint 路径自动分发：
//! - **Fun-ASR / Paraformer**（`/api-ws/v1/inference`）：任务型协议
//!   （`run-task` → 二进制 PCM → `finish-task` → `result-generated`）
//! - **Qwen-ASR Realtime**（`/api-ws/v1/realtime`）：OpenAI Realtime 风格会话协议
//!   （`session.update` → base64 PCM via `input_audio_buffer.append` → `session.finish`）
//!
//! 与 `engine_aliyun.rs` 的 chunk 模式（每段 VAD 开一条新 WS）不同，本模块维护
//! 一条长连接 WS，由 coordinator 的 VAD 逻辑管理连接生命周期：
//! - 语音 onset → [`AliyunStreamSession::open`]：建连 + 初始化 + 推 ~100ms pre-roll
//! - 持续语音 → [`AliyunStreamSession::push_pcm`]：推 PCM 帧
//! - 静音 ≥ `pause_polish_threshold_ms` → [`AliyunStreamSession::close`]：结束 + 收最终结果
//!
//! ## 异步模型
//!
//! coordinator 运行在 `std::thread`（非 tokio runtime）。WS 是 async（tokio-tungstenite），
//! 故本会话在 tokio runtime（CloudBatchEngine 的 block_on 驱动）上 spawn 一条 tokio task 跑
//! `tokio::select!` 双向循环：收 PCM → send / 收 WS text → 发 result event。
//! coordinator 通过同步 channel 非阻塞收 partial（`try_recv`），close 时阻塞等最终结果。

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use octopus_asr_local::sentence_separator;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::client::IntoClientRequest,
    tungstenite::http::header::AUTHORIZATION,
    tungstenite::Message,
};

use crate::cloud_types::{CloudStreamHandle, PcmFrame, StreamEvent};

/// 建连 + 初始化 + 推 pre-roll PCM + 启动后台 WS task。
///
/// 根据 `endpoint` 路径自动选择协议：
/// - 含 `/v1/realtime` → Qwen-ASR Realtime 会话协议（OpenAI Realtime 风格）
/// - 否则 → Fun-ASR/Paraformer 任务型协议（run-task/finish-task）
///
/// **须在 tokio runtime 上下文调用**（内部 `tokio::spawn`）。批引擎 `CloudBatchEngine`
/// 在自有 runtime 的 `block_on` 内调用。
/// `pre_roll_samples` 是 f32[-1,1] 样本（批处理传空 Vec：整段一次推，无需前导）。
pub fn open(
    endpoint: String,
    key: String,
    model: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    let is_qwen = is_qwen_realtime_endpoint(&endpoint);
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result = if is_qwen {
            run_qwen_realtime_session(
                pcm_rx, result_tx, endpoint, key, model, language, pre_roll_samples,
            )
            .await
        } else {
            run_ws_session(
                pcm_rx, result_tx, endpoint, key, model, language, pre_roll_samples,
            )
            .await
        };
        if let Err(e) = result {
            log::error!("aliyun stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
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
        .context("aliyun WS 请求构造失败")?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("bearer {}", key)
            .parse()
            .context("aliyun Authorization header 构造失败")?,
    );
    let (mut ws, _resp) = connect_async(request)
        .await
        .with_context(|| format!("aliyun WS 连接失败: {}", endpoint))?;

    // 2. 发 run-task（含 max_sentence_silence=600，比客户端 700ms 短，让服务端先出完整句）
    let task_id = uuid::Uuid::new_v4().to_string();
    let run_task = build_run_task_streaming(&model, &language, &task_id);
    ws.send(Message::Text(run_task.to_string()))
        .await
        .context("aliyun WS 发送 run-task 失败")?;

    // 3. 推 pre-roll PCM
    if !pre_roll_samples.is_empty() {
        let pcm = crate::cloud_types::samples_to_pcm_s16le(&pre_roll_samples);
        ws.send(Message::binary(pcm))
            .await
            .context("aliyun WS 发送 pre-roll PCM 失败")?;
    }

    // 4. 双向循环
    // Fun-ASR 在一个 task 内可能发多句 result-generated。根据文档：
    // - sentence_id 从 1 递增，标识当前句子
    // - sentence_end=true 表示该句最终结果（之后 sentence_id 会递增）
    // - heartbeat=true 时 sentence_id=0，应跳过
    // - text 是该句的累积文本（中间结果可能被修订，最终结果在 sentence_end=true 时确定）
    let sep = sentence_separator(&language); // 句间分隔符（英文空格 / 其他中文逗号）
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
                            .context("aliyun WS 发送 PCM 帧失败")?;
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
                            .context("aliyun WS 发送 finish-task 失败")?;
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
                                        if !committed.is_empty() && !committed.ends_with(sep) {
                                            committed.push_str(sep);
                                        }
                                        committed.push_str(&current_sentence);
                                        current_sentence.clear();
                                    }
                                }
                                current_sentence_id = sentence_id;
                                current_sentence = text.to_string();

                                // sentence_end=true = 该句最终结果，立即提交
                                if sentence_end {
                                    if !committed.is_empty() && !committed.ends_with(sep) {
                                        committed.push_str(sep);
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
                                    if !committed.is_empty() && !committed.ends_with(sep) {
                                        committed.push_str(sep);
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
                    None => {
                        // WS 意外关闭（服务端断开，非 finish-task 正常收尾）→ 上报 Failed
                        // （审查 一2）：否则 coordinator try_recv 恒 None、僵尸会话卡到 Esc。
                        // 这是 ws.next() 的 None（服务端关），与 pcm_rx.recv() 的 None
                        // （coordinator drop、优雅关闭）不同——后者保持静默 break 不报错。
                        let _ = result_tx.send(StreamEvent::Failed("WS 连接意外关闭".to_string()));
                        break;
                    }
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
pub fn is_qwen_realtime_endpoint(endpoint: &str) -> bool {
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
        let pcm = crate::cloud_types::samples_to_pcm_s16le(&pre_roll_samples);
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
    let sep = sentence_separator(&language); // 句间分隔符（英文空格 / 其他中文逗号）
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
                                            && !accumulated_text.ends_with(sep)
                                        {
                                            accumulated_text.push_str(sep);
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
                    None => {
                        // WS 意外关闭（服务端断开，非 finish-task 正常收尾）→ 上报 Failed
                        // （审查 一2）：否则 coordinator try_recv 恒 None、僵尸会话卡到 Esc。
                        // 这是 ws.next() 的 None（服务端关），与 pcm_rx.recv() 的 None
                        // （coordinator drop、优雅关闭）不同——后者保持静默 break 不报错。
                        let _ = result_tx.send(StreamEvent::Failed("WS 连接意外关闭".to_string()));
                        break;
                    }
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
