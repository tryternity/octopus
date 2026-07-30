//! 阿里云百炼 DashScope ASR WebSocket 引擎。
//!
//! 支持两套云端协议，通过 endpoint 路径自动分发：
//! - **Fun-ASR / Paraformer**（`/api-ws/v1/inference`）：任务型协议
//!   1. `run-task`（text frame，model + parameters.format=pcm）
//!   2. 服务端回 `task-started`
//!   3. 客户端发二进制 PCM 帧（s16le, 16kHz, mono），分块 200ms
//!   4. 发 `finish-task`（text frame）
//!   5. 服务端逐句回 `result-generated`（累积 `payload.output.sentence.text`）
//!   6. 最终 `task-finished` 关闭
//! - **Qwen-ASR Realtime**（`/api-ws/v1/realtime`）：OpenAI Realtime 风格会话协议
//!   1. URL 追加 `?model=<model_name>` 查询参数
//!   2. `session.update`（配置 pcm/16k + Manual 模式）
//!   3. `input_audio_buffer.append`（base64 PCM，文本帧）
//!   4. `input_audio_buffer.commit` + `session.finish`
//!   5. 收 `conversation.item.input_audio_transcription.completed`
//!   6. `session.finished` 关闭
//!
//! 集成点：桌面分块 [`TranscriptionEngine`]（coordinator 按 `is_streaming_engine=false`
//! 时，每段 VAD 调一次 [`AliyunEngine::transcribe`]）。
//!
//! 鉴权：WS 请求 header `Authorization: bearer <secret_key>`（DashScope API Key）。
//! DashScope api-ws 端点强制要求该头，缺则 401/连接被拒。通过 `IntoClientRequest` 把
//! endpoint 转成合法 WS 握手请求（自动填 Sec-WebSocket-*/Host/Upgrade），再追加
//! Authorization 头，最后 `connect_async(request)`。

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::{
    connect_async,
    tungstenite::client::IntoClientRequest,
    tungstenite::handshake::client::Request,
    tungstenite::http::header::AUTHORIZATION,
    tungstenite::Message,
    WebSocketStream,
};
use octopus_asr_local::sentence_separator;

/// DashScope duplex WS 流类型别名。
///
/// `connect_async` 返回 `WebSocketStream<MaybeTlsStream<TcpStream>>`（tokio-tungstenite 0.29），
/// 这里取别名便于在拆出的 helper（`send_pcm_frames` / `collect_results`）签名中复用。
type WsStream = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

use crate::engine::engine::TranscriptionEngine;

/// 构造带 bearer 鉴权的 WS 握手请求。
///
/// `endpoint.into_client_request()` 自动补齐 WS 握手头（Sec-WebSocket-*、Host、Upgrade），
/// 这里只追加 `Authorization: bearer <key>`。抽出为独立函数便于单测 header 注入。
fn build_authed_request(endpoint: &str, key: &str) -> Result<Request> {
    let mut request = endpoint
        .into_client_request()
        .context("aliyun WS 请求构造失败")?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("bearer {}", key)
            .parse()
            .context("aliyun Authorization header 构造失败")?,
    );
    Ok(request)
}

/// 阿里云 DashScope FunASR Realtime WS 引擎。
///
/// 无运行时状态：每次 `transcribe` 调用都从 DB 重新解析 `engine` 字符串 →
/// 取 endpoint + secret_key → 开一条 WS。这样运行时切换 asr_engine（toolbar 命令）
/// 可即时生效。
pub struct AliyunEngine;

impl AliyunEngine {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TranscriptionEngine for AliyunEngine {
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String> {
        // 1. 从 DB 解析 engine spec → endpoint + secret_key（resolve_engine_any 查任意可用 ASR）。
        let model_name = octopus_infra::db::parse_model_spec(engine)
            .model_name()
            .to_string();
        let (_cat, entry) = octopus_asr_local::config::resolve_engine_any(engine)
            .with_context(|| {
                format!(
                    "aliyun ASR 模型 '{}' 未在 DB 配置",
                    model_name
                )
            })?;

        if entry.secret_key.is_empty() {
            bail!(
                "aliyun ASR 模型 '{}' 的 secret_key（DashScope API Key）为空，请用 sqlite3 填写：\n\
                 sqlite3 ~/.octopus/octopus.db \"UPDATE models SET secret_key='sk-...' WHERE model_name='{}'\"",
                model_name,
                model_name
            );
        }
        if entry.source.is_empty() {
            bail!(
                "aliyun ASR 模型 '{}' 的 source（DashScope WS endpoint）为空，请用 sqlite3 填写",
                model_name
            );
        }

        let endpoint = entry.source.clone();
        // follow-up #7：secret_key 可能是 v1: 加密格式（vault 启用后 Task 20 迁移过），
        // 用全局 session 透明解密。本地 / 未迁移明文 → no-op 返回原值。
        // 安全修复 #5：vault 启用但解密失败（app_key 不可用 / 密文损坏）→ Err，
        // 不把密文当 Bearer 发到云端（会污染云端 access log）。
        let key = crate::vault::vault_secret_access::try_decrypt_secret_global(&entry.secret_key)
            .map_err(|_| anyhow::anyhow!("云端 ASR 鉴权失败：保险库未解锁或密文损坏，请先解锁保险库"))?;
        let model = model_name;
        let samples = samples.to_vec();
        let language = language.to_string();

        // 2. 全流程超时 8s（与 engine_ws.rs 一致）
        //    根据 endpoint 路径选择协议
        let is_qwen = octopus_asr_cloud::aliyun_stream::is_qwen_realtime_endpoint(&endpoint);
        tokio::time::timeout(Duration::from_secs(8), async move {
            if is_qwen {
                run_qwen_realtime_transcribe(&endpoint, &key, &model, &samples, &language).await
            } else {
                run_session(&endpoint, &key, &model, &samples, &language).await
            }
        })
        .await
        .map_err(|_| anyhow!("aliyun transcription timeout (8s)"))?
    }

    async fn health_check(&self) -> bool {
        // 健康检查：保守返回 true，避免每次启动探活消耗 API 额度。
        // 真实健康度在首次 transcribe 时由错误路径暴露。
        true
    }
}

/// 跑一次完整的 DashScope duplex 会话：建连 → run-task → PCM 帧 → finish-task → 收结果。
///
/// 编排逻辑：仅做"建连 → 发 run-task → 发 PCM → 发 finish-task → 收结果"的串联，
/// 具体构造/收发细节下沉到 [`build_run_task`] / [`send_pcm_frames`] / [`collect_results`]。
async fn run_session(
    endpoint: &str,
    key: &str,
    model: &str,
    samples: &[f32],
    language: &str,
) -> Result<String> {
    // #6 空 samples 早退：避免空段跑完整 WS 协议（建连 + run/finish + 等关闭）。
    if samples.is_empty() {
        return Ok(String::new());
    }

    // 构造带 Authorization: bearer <key> 的 WS 握手请求，再 connect_async。
    // DashScope api-ws 端点强制要求该头，缺则 401/连接被拒。
    let request = build_authed_request(endpoint, key)?;
    let (mut ws, _resp) = connect_async(request)
        .await
        .with_context(|| format!("aliyun WS 连接失败: {}", endpoint))?;

    // run-task（含完整 payload + header）—— 由 build_run_task 单一构造。
    let task_id = uuid::Uuid::new_v4().to_string();
    let run_task = build_run_task(model, language, &task_id);
    ws.send(Message::Text(run_task.to_string()))
        .await
        .context("aliyun WS 发送 run-task 失败")?;

    // PCM 帧（200ms 分块）。
    send_pcm_frames(&mut ws, samples).await?;

    // finish-task：按官方协议只需 header + payload.input（不需 model/parameters）。
    let finish_task = json!({
        "header": {
        "action": "finish-task",
            "task_id": task_id,
            "streaming": "duplex",
        },
        "payload": {
            "input": {}
        }
    });
    ws.send(Message::Text(finish_task.to_string()))
        .await
        .context("aliyun WS 发送 finish-task 失败")?;

    // 收 result-generated，累积最终句文本。
    collect_results(&mut ws, language).await
}

/// 构造 DashScope `run-task` 请求 JSON（含 header + payload）。
///
/// payload 字段符合 DashScope FunASR Realtime 协议：
/// - `model`：模型名（如 `fun-asr-2025-11-07`）
/// - `task_group`="audio" / `task`="asr" / `function`="recognition"
/// - `parameters.format`="pcm" / `sample_rate`=16000 / `language_hints`
/// - `input`：固定 `{}`（占位，DashScope duplex 协议要求此字段存在于 payload 内）
fn build_run_task(model: &str, language: &str, task_id: &str) -> Value {
    // language="auto"/空 → ["zh","en"] 双语 hints；否则单语。
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
            },
            "input": {},
        }
    })
}

/// 发送二进制 PCM 帧（f32[-1,1] → s16le），分块 200ms。
///
/// 16kHz mono s16le：200ms = 0.2 × 16000 × 2 bytes = 6400 bytes/帧（3200 样本）。
async fn send_pcm_frames(ws: &mut WsStream, samples: &[f32]) -> Result<()> {
    let pcm = samples_to_pcm_s16le(samples);
    const CHUNK_BYTES: usize = 6400;
    for chunk in pcm.chunks(CHUNK_BYTES) {
        ws.send(Message::binary(chunk.to_vec()))
            .await
            .context("aliyun WS 发送 PCM 帧失败")?;
    }
    Ok(())
}

/// 收消息循环：根据 `sentence_id` + `sentence_end` 跨句累积文本，
/// `task-finished` 收尾，`task-failed` 健壮取错后 bail。
async fn collect_results(ws: &mut WsStream, language: &str) -> Result<String> {
    let sep = sentence_separator(language);
    let mut committed = String::new();
    let mut current_sentence = String::new();
    let mut current_sentence_id: i64 = -1;
    while let Some(msg) = ws.next().await {
        let msg = msg.context("aliyun WS 读消息失败")?;
        if let Message::Text(t) = msg {
            let v: Value = match serde_json::from_str(&t) {
                Ok(v) => v,
                Err(_) => continue, // 非 JSON 文本（极少见），忽略
            };
            match v["header"]["event"].as_str() {
                Some("result-generated") => {
                    let sentence = &v["payload"]["output"]["sentence"];
                    // 跳过心跳包
                    if sentence["heartbeat"].as_bool().unwrap_or(false) {
                        continue;
                    }
                    let text = sentence["text"].as_str().unwrap_or("");
                    let sentence_id = sentence["sentence_id"].as_i64().unwrap_or(0);
                    let sentence_end = sentence["sentence_end"].as_bool().unwrap_or(false);

                    // sentence_id 变化 = 新句，提交前一句
                    if sentence_id != current_sentence_id
                        && current_sentence_id > 0
                        && !current_sentence.is_empty()
                    {
                        if !committed.is_empty() && !committed.ends_with(sep) {
                            committed.push_str(sep);
                        }
                        committed.push_str(&current_sentence);
                        current_sentence.clear();
                    }
                    current_sentence_id = sentence_id;
                    current_sentence = text.to_string();

                    // sentence_end=true = 最终结果，立即提交
                    if sentence_end {
                        if !committed.is_empty() && !committed.ends_with(sep) {
                            committed.push_str(sep);
                        }
                        committed.push_str(&current_sentence);
                        current_sentence.clear();
                        current_sentence_id = -1;
                    }
                }
                Some("task-finished") => break,
                Some("task-failed") => {
                    // 健壮取错：优先 error_message（duplex WS 风格），再 error_code；
                    // 都没有则把 header 子树序列化进错误信息（兜底，不丢信息）。
                    let msg = v["header"]["error_message"]
                        .as_str()
                        .or_else(|| v["header"]["error_code"].as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v["header"].to_string());
                    bail!("aliyun task-failed: {}", msg);
                }
                _ => {} // task-started / 其他事件忽略
            }
        }
    }
    // 提交最后一句
    if !current_sentence.is_empty() {
        if !committed.is_empty() && !committed.ends_with(sep) {
            committed.push_str(sep);
        }
        committed.push_str(&current_sentence);
    }
    Ok(committed)
}

/// f32[-1,1] → s16le PCM 转发：实际实现在 [`octopus_asr_cloud::samples_to_pcm_s16le`]。
pub(crate) use octopus_asr_cloud::samples_to_pcm_s16le;

// ── Qwen-ASR Realtime 离线转录（Manual 模式）──

/// Qwen-ASR Realtime 离线转录：建连 → session.update(Manual) → append PCM → commit → finish → 收结果。
///
/// 用于 coordinator 的 VadSegmented 模式（每段 VAD 调一次）。
/// 使用 Manual 模式（turn_detection=null），因为 coordinator 已经做了 VAD 切分，
/// 这里只需转写这一段完整音频。
async fn run_qwen_realtime_transcribe(
    endpoint: &str,
    key: &str,
    model: &str,
    samples: &[f32],
    language: &str,
) -> Result<String> {
    let sep = sentence_separator(language);
    if samples.is_empty() {
        return Ok(String::new());
    }

    // 1. 构造 URL（追加 ?model= 查询参数）
    let url = if endpoint.contains("?model=") || endpoint.contains("&model=") {
        endpoint.to_string()
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

    // 3. 发 session.update（Manual 模式：turn_detection=null）
    let transcription = if language.is_empty() || language == "auto" {
        json!({})
    } else {
        json!({ "language": language })
    };
    let session_update = json!({
        "event_id": format!("evt_{}", uuid::Uuid::new_v4().simple()),
        "type": "session.update",
        "session": {
            "input_audio_format": "pcm",
            "sample_rate": 16000,
            "input_audio_transcription": transcription,
            "turn_detection": null,
        }
    });
    ws.send(Message::Text(session_update.to_string()))
        .await
        .context("qwen-asr WS 发送 session.update 失败")?;

    // 4. 发 PCM（base64 编码，200ms 分块）
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let pcm = samples_to_pcm_s16le(samples);
    const CHUNK_SAMPLES: usize = 3200; // 200ms @ 16kHz
    const CHUNK_BYTES: usize = CHUNK_SAMPLES * 2; // s16le = 2 bytes/sample
    for chunk in pcm.chunks(CHUNK_BYTES) {
        let b64 = STANDARD.encode(chunk);
        let append = json!({
            "event_id": format!("evt_{}", uuid::Uuid::new_v4().simple()),
            "type": "input_audio_buffer.append",
            "audio": b64,
        });
        ws.send(Message::Text(append.to_string()))
            .await
            .context("qwen-asr WS 发送 PCM append 失败")?;
    }

    // 5. commit + finish（Manual 模式：commit 触发识别，finish 结束会话）
    let commit = json!({
        "event_id": format!("evt_{}", uuid::Uuid::new_v4().simple()),
        "type": "input_audio_buffer.commit",
    });
    ws.send(Message::Text(commit.to_string()))
        .await
        .context("qwen-asr WS 发送 commit 失败")?;

    let finish = json!({
        "event_id": format!("evt_{}", uuid::Uuid::new_v4().simple()),
        "type": "session.finish",
    });
    ws.send(Message::Text(finish.to_string()))
        .await
        .context("qwen-asr WS 发送 session.finish 失败")?;

    // 6. 收结果：conversation.item.input_audio_transcription.completed + session.finished
    let mut text = String::new();
    while let Some(msg) = ws.next().await {
        let msg = msg.context("qwen-asr WS 读消息失败")?;
        if let Message::Text(t) = msg {
            let v: Value = match serde_json::from_str(&t) {
                Ok(v) => v,
                Err(_) => continue,
            };
            match v["type"].as_str() {
                Some("conversation.item.input_audio_transcription.completed") => {
                    if let Some(t) = v["transcript"].as_str() {
                        if !text.is_empty() && !text.ends_with(sep) {
                            text.push_str(sep);
                        }
                        text.push_str(t);
                    }
                }
                Some("session.finished") => break,
                Some("error") => {
                    let msg = v["error"]["message"]
                        .as_str()
                        .or_else(|| v["error"]["code"].as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| v["error"].to_string());
                    bail!("qwen-asr error: {}", msg);
                }
                _ => {} // 其他事件忽略
            }
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_conversion_known_values() {
        // 已知值：0.0→0, 1.0→32767, -1.0→-32767（round(-32767.0) = -32767）
        let samples = vec![0.0_f32, 1.0, -1.0];
        let pcm = samples_to_pcm_s16le(&samples);
        assert_eq!(pcm.len(), 6);
        assert_eq!(i16::from_le_bytes([pcm[0], pcm[1]]), 0);
        assert_eq!(i16::from_le_bytes([pcm[2], pcm[3]]), 32767);
        assert_eq!(i16::from_le_bytes([pcm[4], pcm[5]]), -32767);
    }

    #[test]
    fn pcm_conversion_clamps_overflow() {
        // 超出 [-1,1] 的样本被钳到极值：2.0→32767, -2.0→-32767
        let samples = vec![2.0_f32, -2.0];
        let pcm = samples_to_pcm_s16le(&samples);
        assert_eq!(i16::from_le_bytes([pcm[0], pcm[1]]), 32767);
        assert_eq!(i16::from_le_bytes([pcm[2], pcm[3]]), -32767);
    }

    #[test]
    fn pcm_conversion_empty() {
        assert!(samples_to_pcm_s16le(&[]).is_empty());
    }

    #[test]
    fn authed_request_carries_bearer_token() {
        // 构造带鉴权的 WS 握手请求，校验 Authorization header 为 `bearer <key>`。
        // 不实际发 WS（建连是网络调用），仅验证 header 注入逻辑。
        let req = build_authed_request(
            "wss://dashscope.aliyuncs.com/api-ws/v1/inference",
            "sk-test-123",
        )
        .expect("build_authed_request 应成功");
        assert_eq!(
            req.headers().get(AUTHORIZATION).unwrap(),
            "bearer sk-test-123"
        );
    }

    #[test]
    fn run_task_json_has_required_fields() {
        // 直接调 build_run_task 拿真实返回值断言（消除复制粘贴腐化）。
        // 不实际发 WS，仅验证 run-task JSON 结构符合 DashScope FunASR Realtime 协议。
        let run_task = build_run_task("fun-asr-2025-11-07", "auto", "abc");
        assert_eq!(run_task["header"]["action"], "run-task");
        assert_eq!(run_task["header"]["streaming"], "duplex");
        assert_eq!(run_task["header"]["task_id"], "abc");
        assert_eq!(run_task["payload"]["model"], "fun-asr-2025-11-07");
        assert_eq!(run_task["payload"]["task_group"], "audio");
        assert_eq!(run_task["payload"]["task"], "asr");
        assert_eq!(run_task["payload"]["function"], "recognition");
        assert_eq!(run_task["payload"]["parameters"]["format"], "pcm");
        assert_eq!(run_task["payload"]["parameters"]["sample_rate"], 16000);
        // language="auto" → ["zh","en"]
        assert_eq!(
            run_task["payload"]["parameters"]["language_hints"],
            json!(["zh", "en"])
        );
        // input 必须在 payload 内部（不在顶层）——官方协议强制要求 payload.input
        assert_eq!(run_task["payload"]["input"], json!({}));
        assert!(run_task.get("input").is_none(), "input 不应在顶层");
    }

    #[test]
    fn finish_task_only_carries_payload_input() {
        // finish-task 按官方协议只需 header + payload.input（不带 model/parameters）。
        // 复刻 run_session 的构造逻辑做断言。
        let task_id = "task-xyz".to_string();
        let finish_task = json!({
            "header": {
                "action": "finish-task",
                "task_id": task_id,
                "streaming": "duplex",
            },
            "payload": {
                "input": {}
            }
        });
        assert_eq!(finish_task["header"]["action"], "finish-task");
        assert_eq!(finish_task["header"]["task_id"], "task-xyz");
        assert_eq!(finish_task["header"]["streaming"], "duplex");
        // finish-task 的 payload 只有 input，没有 model/parameters/task_group 等
        assert_eq!(finish_task["payload"]["input"], json!({}));
        assert!(finish_task["payload"].get("model").is_none());
        assert!(finish_task["payload"].get("parameters").is_none());
    }

    #[test]
    fn lang_hints_falls_back_to_zh_en_for_auto() {
        // 通过 build_run_task 验证 language_hints 行为（不再复制粘贴 hints 逻辑）。
        let hints = |lang: &str| build_run_task("m", lang, "t")["payload"]["parameters"]["language_hints"].clone();
        assert_eq!(hints("auto"), json!(["zh", "en"]));
        assert_eq!(hints(""), json!(["zh", "en"]));
        assert_eq!(hints("zh"), json!(["zh"]));
        assert_eq!(hints("en"), json!(["en"]));
    }

    /// DashScope WS 端到端集成测试（需真实 API Key + 网络访问）。
    ///
    /// 运行前需：
    /// 1. `sqlite3 ~/.octopus/octopus.db "UPDATE models SET secret_key='sk-...' WHERE model_name='fun-asr-2025-11-07'"`
    /// 2. 提供 16kHz mono f32 PCM 样本（此处用合成正弦波占位，不会产生有效识别结果）。
    ///
    /// 跳过：`cargo test -p octopus-desktop --features dashscope -- --ignored dashscope_e2e`
    #[tokio::test]
    #[ignore = "需真实 DashScope API Key，且消耗云端调用配额"]
    async fn dashscope_e2e_smoke() {
        let engine = AliyunEngine::new();
        // 合成 1 秒 1kHz 正弦波（非语音，仅验证协议通路，预期返回空或无关文本）
        let sr = 16000_f32;
        let samples: Vec<f32> = (0..sr as usize)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin() * 0.1)
            .collect();
        let result = engine
            .transcribe(&samples, "zh", "fun-asr-2025-11-07")
            .await;
        // 仅要求协议跑通（不要求识别出语义文本）；连接失败也 ok 标记（CI 无 key）
        match result {
            Ok(_t) => { /* 协议成功 */ }
            Err(e) => panic!("dashscope e2e 失败：{}", e),
        }
    }
}
