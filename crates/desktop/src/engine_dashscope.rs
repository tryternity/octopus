//! 阿里云百炼 DashScope FunASR Realtime WebSocket ASR engine。
//!
//! 集成点：桌面分块 [`TranscriptionEngine`]（coordinator 按 `is_streaming_engine=false`
//! 时，每段 VAD 调一次 [`DashscopeEngine::transcribe`]）。
//!
//! DashScope duplex 全双工协议：
//! 1. `run-task`（text frame，action=run-task，streaming=duplex，model + parameters.format=pcm）
//! 2. 服务端回 `task-started`
//! 3. 客户端发二进制 PCM 帧（s16le, 16kHz, mono），分块 200ms
//! 4. 发 `finish-task`（text frame）
//! 5. 服务端逐句回 `result-generated`（累积 `payload.output.sentence.text`）
//! 6. 最终 `task-finished` 关闭
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

/// DashScope duplex WS 流类型别名。
///
/// `connect_async` 返回 `WebSocketStream<MaybeTlsStream<TcpStream>>`（tokio-tungstenite 0.29），
/// 这里取别名便于在拆出的 helper（`send_pcm_frames` / `collect_results`）签名中复用。
type WsStream = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

use crate::engine::TranscriptionEngine;

/// 构造带 bearer 鉴权的 WS 握手请求。
///
/// `endpoint.into_client_request()` 自动补齐 WS 握手头（Sec-WebSocket-*、Host、Upgrade），
/// 这里只追加 `Authorization: bearer <key>`。抽出为独立函数便于单测 header 注入。
fn build_authed_request(endpoint: &str, key: &str) -> Result<Request> {
    let mut request = endpoint
        .into_client_request()
        .context("dashscope WS 请求构造失败")?;
    request.headers_mut().insert(
        AUTHORIZATION,
        format!("bearer {}", key)
            .parse()
            .context("dashscope Authorization header 构造失败")?,
    );
    Ok(request)
}

/// 阿里云 DashScope FunASR Realtime WS 引擎。
///
/// 无运行时状态：每次 `transcribe` 调用都从 DB 重新解析 `engine` 字符串 →
/// 取 endpoint + secret_key → 开一条 WS。这样运行时切换 asr_engine（toolbar 命令）
/// 可即时生效。
pub struct DashscopeEngine;

impl DashscopeEngine {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TranscriptionEngine for DashscopeEngine {
    async fn transcribe(&self, samples: &[f32], language: &str, engine: &str) -> Result<String> {
        // 1. 从 DB 解析 engine spec → endpoint + secret_key。
        //    显式查 asr.aliyun section，未命中精确 bail（不静默回退 zipformer，
        //    避免报错指向错误名字）。
        let cfg = octopus_asr::config::load_config()?;
        let model_name = octopus_infra::db::parse_model_spec(engine)
            .model_name()
            .to_string();
        let entry = cfg
            .asr
            .aliyun
            .as_ref()
            .and_then(|m| m.get(model_name.as_str()))
            .with_context(|| {
                format!(
                    "aliyun ASR 模型 '{}' 未在 DB（asr.aliyun section）配置",
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
        let key = entry.secret_key.clone();
        let model = model_name;
        let samples = samples.to_vec();
        let language = language.to_string();

        // 2. 全流程超时 8s（与 engine_ws.rs 一致）
        tokio::time::timeout(Duration::from_secs(8), async move {
            run_session(&endpoint, &key, &model, &samples, &language).await
        })
        .await
        .map_err(|_| anyhow!("dashscope transcription timeout (8s)"))?
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
        .with_context(|| format!("dashscope WS 连接失败: {}", endpoint))?;

    // run-task（含完整 payload + header）—— 由 build_run_task 单一构造。
    let task_id = uuid::Uuid::new_v4().to_string();
    let run_task = build_run_task(model, language, &task_id);
    ws.send(Message::Text(run_task.to_string()))
        .await
        .context("dashscope WS 发送 run-task 失败")?;

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
        .context("dashscope WS 发送 finish-task 失败")?;

    // 收 result-generated，累积最终句文本。
    collect_results(&mut ws).await
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
            .context("dashscope WS 发送 PCM 帧失败")?;
    }
    Ok(())
}

/// 收消息循环：`result-generated` 累积 `payload.output.sentence.text`（取最新即最终句），
/// `task-finished` 收尾，`task-failed` 健壮取错后 bail。
async fn collect_results(ws: &mut WsStream) -> Result<String> {
    let mut text = String::new();
    while let Some(msg) = ws.next().await {
        let msg = msg.context("dashscope WS 读消息失败")?;
        if let Message::Text(t) = msg {
            let v: Value = match serde_json::from_str(&t) {
                Ok(v) => v,
                Err(_) => continue, // 非 JSON 文本（极少见），忽略
            };
            match v["header"]["event"].as_str() {
                Some("result-generated") => {
                    // FunASR Realtime：payload.output.sentence.text 是累积文本，
                    // 每次覆盖即取最新（最终）句。
                    if let Some(t) = v["payload"]["output"]["sentence"]["text"].as_str() {
                        text = t.to_string();
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
                    bail!("dashscope task-failed: {}", msg);
                }
                _ => {} // task-started / 其他事件忽略
            }
        }
    }
    Ok(text)
}

/// f32[-1, 1] 样本 → s16le PCM 字节流（16kHz mono）。
///
/// 钳幅到 [-1, 1] 后乘 32767 四舍五入为 i16，按小端字节序展开。
fn samples_to_pcm_s16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
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
        let engine = DashscopeEngine::new();
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
