//! 字节跳动豆包大模型 ASR 双向流式会话（bigmodel_async 优化版本）。
//!
//! 与 `aliyun_stream.rs` 的接口完全一致（`push_pcm` / `try_recv_text` / `finish` / `close_async`），
//! 但内部实现火山的二进制帧协议（4B header + payload），而非 DashScope 的 JSON 文本协议。
//!
//! ## 协议
//!
//! Endpoint 固定：`wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`
//!
//! WS 握手 Headers：
//! - `X-Api-Key: <api_key>`（DB secret_key 字段）
//! - `X-Api-Resource-Id: <resource_id>`（DB source 字段，如 `volc.bigasr.sauc.duration`）
//! - `X-Api-Request-Id: <UUID>`
//! - `X-Api-Sequence: -1`
//!
//! 二进制帧（所有整数大端序）：
//! ```text
//! Byte 0: [Ver 4b=1] [Hdr Size 4b=1]  → 0x11
//! Byte 1: [Msg Type 4b] [Flags 4b]
//! Byte 2: [Ser 4b] [Comp 4b]
//! Byte 3: [Reserved 8b=0]
//! Bytes 4-7: payload_size (uint32 BE)
//! Bytes 8+: payload
//! ```
//!
//! 消息类型：
//! - 0x1 FULL_CLIENT_REQUEST（初始 JSON config，flags=0x0）
//! - 0x2 AUDIO_ONLY_REQUEST（音频帧，flags=0x0 正常 / 0x2 末帧）
//! - 0x9 FULL_SERVER_RESPONSE（JSON 结果，flags=0x1 正常 / 0x3 末帧）
//! - 0xF ERROR_RESPONSE（错误码 + 消息）
//!
//! Serialization：0x0=NONE / 0x1=JSON
//! Compression：0x0=NONE / 0x1=GZIP

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use std::io::{Read, Write};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    tungstenite::client::IntoClientRequest,
    tungstenite::http::HeaderName,
    tungstenite::http::HeaderValue,
    tungstenite::Message,
};

use crate::cloud_types::{CloudStreamHandle, PcmFrame, StreamEvent};
use crate::session_loop::{HandleOutcome, WsSessionHandler, run_ws_session_loop};

/// 固定 endpoint（火山引擎大模型 ASR 双向流式优化版）。
const ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";

// ── 二进制协议常量 ──
const PROTOCOL_VERSION: u8 = 0x1;
const HEADER_SIZE: u8 = 0x1;

const MSG_FULL_CLIENT_REQUEST: u8 = 0x1;
const MSG_AUDIO_ONLY_REQUEST: u8 = 0x2;
const MSG_FULL_SERVER_RESPONSE: u8 = 0x9;
const MSG_ERROR_RESPONSE: u8 = 0xF;

const FLAG_NO_SEQUENCE: u8 = 0x0;
const FLAG_NEG_SEQUENCE: u8 = 0x2; // 末帧（负包）

const SER_NONE: u8 = 0x0;
const SER_JSON: u8 = 0x1;

const COMP_GZIP: u8 = 0x1;
const COMP_NONE: u8 = 0x0;

/// 建连 + 发初始 config + 推 pre-roll PCM + 启动后台 WS task。
///
/// **须在 tokio runtime 上下文调用**（内部 `tokio::spawn`）。
pub fn open(
    api_key: String,
    resource_id: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result =
            run_bytedance_session(pcm_rx, result_tx, ENDPOINT, api_key, resource_id, language, pre_roll_samples)
                .await;
        // session 契约：Ok = 已通过 result_tx 通知最终结果（Finished/运行期 Failed，
        // 见 run_bytedance_session 内 WS 错误分支 return Ok 处）；仅 Err（签名/建连等启动期失败，
        // 未及经 channel 通知）在此补发一次 Failed——避免与 session 内部已发的 Failed 重复。
        if let Err(e) = result {
            log::error!("bytedance stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}

/// 后台 WS 会话主逻辑：建连 → 发初始 config → pre-roll → 双向循环 → 末帧 → 收结果。
///
/// `endpoint` 参数化（P2-1 WS mock）：prod 调用传真 `const ENDPOINT`（`wss://...`），
/// 测试用 in-process server 时传 `ws://127.0.0.1:{port}`（见 `test_ws_server`）。
async fn run_bytedance_session(
    pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    endpoint: &str,
    api_key: String,
    resource_id: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<()> {
    let handler = BytedanceHandler {
        api_key,
        resource_id,
        request_id: uuid::Uuid::new_v4().to_string(),
        lang: map_language(&language),
        last_text: None,
    };
    run_ws_session_loop(pcm_rx, result_tx, endpoint, &pre_roll_samples, handler).await
}

/// bytedance 语言映射（`zh`/`auto`/空 → `zh-CN`、`en` → `en-US`、其余 `<lang>-CN`）。
fn map_language(language: &str) -> String {
    if language.is_empty() || language == "auto" || language == "zh" {
        "zh-CN".to_string()
    } else if language == "en" {
        "en-US".to_string()
    } else {
        format!("{}-CN", language)
    }
}

/// bytedance WS session 协议 hook 实现（见 [`crate::session_loop::run_ws_session_loop`]）。
///
/// 持有 bytedance 特定状态：鉴权参数（api_key/resource_id/request_id）、语言映射、
/// 结果累积（last_text——每帧 result.text 直接发，Close 时用 best-effort 判定）。
struct BytedanceHandler {
    api_key: String,
    resource_id: String,
    request_id: String,
    lang: String,
    // bytedance 无累加器——每帧 result.text 直接发 Text 事件。Close 时若有 text 则当
    // best-effort Finished，否则 Failed。仅非空 text 记入 last_text（G1/H1：空 text 不污染）。
    last_text: Option<String>,
}

impl WsSessionHandler for BytedanceHandler {
    const LABEL: &'static str = "bytedance";

    fn build_connect_request(
        &self,
        endpoint: &str,
    ) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
        let mut request = endpoint.into_client_request()?;
        let headers = request.headers_mut();
        headers.insert(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_str(&self.api_key)
                .context("bytedance X-Api-Key 构造失败")?,
        );
        headers.insert(
            HeaderName::from_static("x-api-resource-id"),
            HeaderValue::from_str(&self.resource_id)
                .context("bytedance X-Api-Resource-Id 构造失败")?,
        );
        headers.insert(
            HeaderName::from_static("x-api-request-id"),
            HeaderValue::from_str(&self.request_id)
                .context("bytedance X-Api-Request-Id 构造失败")?,
        );
        headers.insert(
            HeaderName::from_static("x-api-sequence"),
            HeaderValue::from_static("-1"),
        );
        Ok(request)
    }

    fn build_init_message(&self) -> Result<Option<Message>> {
        // FULL_CLIENT_REQUEST（初始 JSON config，gzip 压缩）
        let config = json!({
            "user": { "uid": self.request_id },
            "audio": {
                "format": "pcm",
                "codec": "raw",
                "rate": 16000,
                "bits": 16,
                "channel": 1,
                "language": self.lang,
            },
            "request": {
                "model_name": "bigmodel",
                "enable_itn": true,
                "enable_punc": true,
                "enable_ddc": false,
                "show_utterances": true,
            }
        });
        let init_frame = build_client_frame(
            MSG_FULL_CLIENT_REQUEST,
            FLAG_NO_SEQUENCE,
            SER_JSON,
            COMP_GZIP,
            config.to_string().as_bytes(),
        )?;
        Ok(Some(Message::binary(init_frame)))
    }

    fn build_pcm_message(&self, pcm_s16le: &[u8]) -> Result<Message> {
        let audio_frame = build_client_frame(
            MSG_AUDIO_ONLY_REQUEST,
            FLAG_NO_SEQUENCE,
            SER_NONE,
            COMP_NONE,
            pcm_s16le,
        )?;
        Ok(Message::binary(audio_frame))
    }

    fn build_finish_message(&self) -> Result<Message> {
        // 末帧（负包 = NEG_SEQUENCE）——告诉服务端音频结束
        let last_frame = build_client_frame(
            MSG_AUDIO_ONLY_REQUEST,
            FLAG_NEG_SEQUENCE,
            SER_NONE,
            COMP_NONE,
            &[],
        )?;
        Ok(Message::binary(last_frame))
    }

    fn handle_message(
        &mut self,
        msg: Message,
        result_tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> HandleOutcome {
        if let Message::Binary(data) = msg {
            let parsed = match parse_server_frame(&data) {
                Ok(p) => p,
                Err(e) => {
                    // 原实现 .context("bytedance WS 响应解析失败")? 向上传播。
                    return HandleOutcome::TerminalFailed(format!(
                        "bytedance WS 响应解析失败: {}",
                        e
                    ));
                }
            };
            match parsed.msg_type {
                MSG_FULL_SERVER_RESPONSE => {
                    let json_str = match decompress_or_raw(
                        &parsed.payload,
                        parsed.compression == COMP_GZIP,
                    ) {
                        Ok(s) => s,
                        Err(e) => {
                            return HandleOutcome::TerminalFailed(format!(
                                "bytedance 响应解压失败: {}",
                                e
                            ));
                        }
                    };
                    let json_val: Value = match serde_json::from_str(&json_str) {
                        Ok(v) => v,
                        Err(e) => {
                            return HandleOutcome::TerminalFailed(format!(
                                "bytedance 响应 JSON 解析失败: {}",
                                e
                            ));
                        }
                    };
                    if let Some(text) = json_val["result"]["text"].as_str() {
                        // 仅非空 text 处理（G1/G2/H1：空 text 不应记入 last_text，
                        // 也不应发 Text 事件——close_async 的 text = t 会用空串覆盖
                        // 之前累积的非空文本，导致有效结果丢失。其他 3 家 provider
                        // 都有 !display.is_empty() 保护，bytedance 对齐）。
                        if !text.is_empty() {
                            self.last_text = Some(text.to_string());
                            let _ = result_tx.send(StreamEvent::Text(text.to_string()));
                        }
                    }
                    if parsed.flags == 0x3 {
                        // 末帧响应（NEG_WITH_SEQUENCE）——全部结束。
                        // 稳态判据：有非空 last_text（收到过实质识别内容）才发 Finished，
                        // 否则发 Failed 暴露异常（对齐其他 provider 的 !stable.is_empty() 判据，
                        // 防空 text 当成功吞掉鉴权降级/空音频等异常）。
                        if self.last_text.is_some() {
                            HandleOutcome::TerminalFinished
                        } else {
                            HandleOutcome::TerminalFailed(
                                "bytedance 末帧响应但无有效识别结果".into(),
                            )
                        }
                    } else {
                        HandleOutcome::Continue
                    }
                }
                MSG_ERROR_RESPONSE => {
                    let error_code = parsed.sequence; // 错误帧的 sequence 位存 error code
                    let error_msg = String::from_utf8_lossy(&parsed.payload);
                    HandleOutcome::TerminalFailed(format!(
                        "bytedance 错误 {}: {}",
                        error_code, error_msg
                    ))
                }
                _ => {
                    log::debug!("bytedance: 未知消息类型 0x{:X}", parsed.msg_type);
                    HandleOutcome::Continue
                }
            }
        } else if let Message::Close(_) = msg {
            // 服务端主动 Close（鉴权失败/超时/限流等）。旧实现注释「text/close/ping
            // 等忽略」未处理 Close → 随后 ws.next() 返 Ok(None) → break →
            // return Ok(()) 无终态事件，close_async 把 partial 当成功（#3）。
            // 现显式处理：有非空 last_text（best-effort，未到末帧但收到过实质内容）
            // 发 Finished 让用户拿到已识别内容；否则 Failed 暴露异常。
            // 注：last_text 仅在非空时存（见上面 MSG_FULL_SERVER_RESPONSE），
            // 故 Some 必非空，Close 不会发空 text。
            log::debug!("bytedance: WS 连接关闭");
            if let Some(t) = self.last_text.take() {
                let _ = result_tx.send(StreamEvent::Text(t));
                HandleOutcome::TerminalFinished
            } else {
                HandleOutcome::TerminalFailed(
                    "bytedance WS 连接关闭但未收到识别结果".into(),
                )
            }
        } else {
            // text/ping 等忽略
            HandleOutcome::Continue
        }
    }
}

// ── 二进制帧构造/解析 ──

/// 构造客户端帧（header 4B + payload_size 4B + payload）。
fn build_client_frame(
    msg_type: u8,
    flags: u8,
    serialization: u8,
    compression: u8,
    payload_raw: &[u8],
) -> Result<Vec<u8>> {
    // 压缩 payload（如需要）
    let payload: Vec<u8> = if compression == COMP_GZIP {
        gzip_compress(payload_raw)?
    } else {
        payload_raw.to_vec()
    };

    let byte0 = (PROTOCOL_VERSION << 4) | HEADER_SIZE;
    let byte1 = (msg_type << 4) | flags;
    let byte2 = (serialization << 4) | compression;
    let byte3 = 0x00u8;

    let payload_size = payload.len() as u32;

    let mut frame = Vec::with_capacity(4 + 4 + payload.len());
    frame.push(byte0);
    frame.push(byte1);
    frame.push(byte2);
    frame.push(byte3);
    frame.extend_from_slice(&payload_size.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// 解析后的服务端帧。
#[allow(dead_code)]
#[derive(Debug)]
struct ParsedServerFrame {
    msg_type: u8,
    flags: u8,
    compression: u8,
    sequence: u32,
    payload: Vec<u8>,
}

/// 解析服务端帧。
///
/// FULL_SERVER_RESPONSE（0x9）：header(4) + seq(4) + payload_size(4) + payload
/// ERROR_RESPONSE（0xF）：header(4) + error_code(4) + error_msg_size(4) + error_msg
fn parse_server_frame(data: &[u8]) -> Result<ParsedServerFrame> {
    if data.len() < 4 {
        bail!("帧太短（{} bytes < 4）", data.len());
    }
    let byte1 = data[1];
    let byte2 = data[2];

    let msg_type = (byte1 >> 4) & 0x0F;
    let flags = byte1 & 0x0F;
    let compression = byte2 & 0x0F;

    // seq/error_code (4B) + payload/error_msg_size (4B)
    if data.len() < 12 {
        bail!("帧不完整（{} bytes < 12），缺 seq + payload_size", data.len());
    }
    let sequence = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let payload_size = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;

    // data 已是 tungstenite 重组的完整 WS 消息，payload_size 超过实际字节数属应用层
    // 协议异常——直接 bail，否则截断 payload 进下游（gzip/JSON 报"解析失败"、
    // ERROR_RESPONSE 经 from_utf8_lossy 静默截断 error_msg），都掩盖了根因。
    let payload = if data.len() >= 12 + payload_size {
        data[12..12 + payload_size].to_vec()
    } else {
        bail!(
            "payload 不完整：header 声明 {} 字节，但帧仅余 {} 字节",
            payload_size,
            data.len() - 12
        );
    };

    Ok(ParsedServerFrame {
        msg_type,
        flags,
        compression,
        sequence,
        payload,
    })
}

/// gzip 压缩。
///
/// 压缩失败直接返回 `Err` 向上传播，**不**回退到未压缩原始数据——
/// 帧头已固定标记 `COMP_GZIP`，回退 raw 会让服务端按 gzip 解析失败（协议错误帧）。
///
/// `GzEncoder` 底层为 `Vec<u8>`（`Write` infallible），DEFLATE 对任意输入亦不报错，
/// 故实际近乎不可能失败；保留 `Result` 仅为杜绝静默吞错，让真正异常走 `?` 链路。
fn gzip_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .context("bytedance gzip write 失败")?;
    encoder.finish().context("bytedance gzip finish 失败")
}

/// gzip 解压或直接返回。
fn decompress_or_raw(data: &[u8], is_gzip: bool) -> Result<String> {
    if is_gzip {
        let mut decoder = GzDecoder::new(data);
        let mut result = String::new();
        decoder.read_to_string(&mut result)?;
        Ok(result)
    } else {
        Ok(String::from_utf8_lossy(data).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_client_frame_audio_uses_none_compression() {
        // 回归 #6：PCM s16le 高熵，gzip 无效（ratio 0.9-1.1）且双向编解码是热路径 CPU 浪费。
        // 音频帧（含 pre-roll / realtime / 末帧）改 COMP_NONE，协议合法（0x0=NONE）。
        let payload = b"test_audio_pcm_s16le";
        let frame = build_client_frame(
            MSG_AUDIO_ONLY_REQUEST,
            FLAG_NO_SEQUENCE,
            SER_NONE,
            COMP_NONE,
            payload,
        ).unwrap();
        // header 4B + payload_size 4B + raw payload（未压缩，长度等于 payload）
        assert_eq!(frame[0], 0x11); // ver=1, hdr=1
        assert_eq!((frame[1] >> 4) & 0xF, MSG_AUDIO_ONLY_REQUEST);
        assert_eq!(frame[1] & 0xF, FLAG_NO_SEQUENCE);
        assert_eq!((frame[2] >> 4) & 0xF, SER_NONE);
        assert_eq!(frame[2] & 0xF, COMP_NONE, "音频帧 compression 必须是 NONE");
        // payload 原样保留（未 gzip）
        let payload_size = u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]);
        assert_eq!(payload_size as usize, payload.len(), "NONE 帧长度 == 原始 payload");
        assert_eq!(&frame[8..], payload);
    }

    #[test]
    fn test_build_client_frame_last_uses_none_compression() {
        // 末帧（空 payload）也用 COMP_NONE（与音频帧一致）
        let frame = build_client_frame(
            MSG_AUDIO_ONLY_REQUEST,
            FLAG_NEG_SEQUENCE,
            SER_NONE,
            COMP_NONE,
            &[],
        ).unwrap();
        // 末帧 flags = 0x2 (NEG_SEQUENCE)
        assert_eq!(frame[1] & 0xF, FLAG_NEG_SEQUENCE);
        assert_eq!(frame[2] & 0xF, COMP_NONE);
    }

    /// 初始 JSON config 帧仍用 GZIP（文本可压，合理）——确保未误改。
    #[test]
    fn test_build_client_frame_config_uses_gzip() {
        let config = r#"{"user":{"uid":"test"}}"#;
        let frame = build_client_frame(
            MSG_FULL_CLIENT_REQUEST,
            FLAG_NO_SEQUENCE,
            SER_JSON,
            COMP_GZIP,
            config.as_bytes(),
        ).unwrap();
        assert_eq!((frame[2] >> 4) & 0xF, SER_JSON);
        assert_eq!(frame[2] & 0xF, COMP_GZIP, "JSON config 帧保持 GZIP（文本可压）");
    }

    #[test]
    fn test_gzip_roundtrip() {
        let original = r#"{"result":{"text":"测试文本"}}"#;
        let compressed = gzip_compress(original.as_bytes()).unwrap();
        let decompressed = decompress_or_raw(&compressed, true).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_parse_server_frame_response() {
        // 模拟一个 FULL_SERVER_RESPONSE 帧
        let payload = r#"{"result":{"text":"hello"}}"#;
        let payload_bytes = payload.as_bytes();

        let mut frame = vec![0x11, 0x91, 0x11, 0x00];
        frame.extend_from_slice(&1u32.to_be_bytes()); // sequence = 1
        frame.extend_from_slice(&(payload_bytes.len() as u32).to_be_bytes()); // payload_size
        frame.extend_from_slice(payload_bytes); // raw payload (无压缩方便测试)

        let parsed = parse_server_frame(&frame).unwrap();
        assert_eq!(parsed.msg_type, MSG_FULL_SERVER_RESPONSE);
        assert_eq!(parsed.flags, 0x1);
        assert_eq!(parsed.sequence, 1);
        assert_eq!(
            decompress_or_raw(&parsed.payload, false).unwrap(),
            payload
        );
    }

    #[test]
    fn test_parse_server_frame_error() {
        let error_msg = "Invalid request";
        let mut frame = vec![0x11, 0xF1, 0x00, 0x00];
        frame.extend_from_slice(&45000001u32.to_be_bytes()); // error_code
        frame.extend_from_slice(&(error_msg.len() as u32).to_be_bytes());
        frame.extend_from_slice(error_msg.as_bytes());

        let parsed = parse_server_frame(&frame).unwrap();
        assert_eq!(parsed.msg_type, MSG_ERROR_RESPONSE);
        assert_eq!(parsed.sequence, 45000001); // error code 存在 sequence 位
        assert_eq!(String::from_utf8_lossy(&parsed.payload), error_msg);
    }

    #[test]
    fn test_parse_server_frame_truncated_payload() {
        // payload_size 声明 100 但实际只给 5 字节 → 应 bail（不再"尽量取"截断，
        // 避免下游 gzip/JSON 报"解析失败"或 ERROR_RESPONSE lossy 静默截断 error_msg 掩盖根因）。
        let mut frame: Vec<u8> = vec![0x11, 0x91, 0x11, 0x00]; // ver/hdr, FULL_SERVER_RESPONSE+POS_SEQUENCE, JSON+GZIP, reserved
        frame.extend_from_slice(&1u32.to_be_bytes()); // sequence
        frame.extend_from_slice(&100u32.to_be_bytes()); // payload_size=100（远超实际）
        frame.extend_from_slice(b"hello"); // 实际仅 5 字节 payload
        let err = parse_server_frame(&frame).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("payload 不完整") && msg.contains("100"),
            "截断帧应 bail 并点明 payload_size 与实际不符，实际报错: {}",
            msg
        );
    }

    // ── P2-1 WS mock：Close 帧终态测试 + H1 空 text 回归（spec §2.2 + §8 G1/G2/H1）──

    use crate::test_ws_server::WsTestServer;
    use crate::cloud_types::{CloudStreamHandle, StreamEvent};

    /// 辅助：构造 bytedance server 响应二进制帧（FULL_SERVER_RESPONSE）。
    /// flags=0x1 正常帧，0x3 末帧。payload 是 raw JSON（无压缩，方便测试）。
    fn build_server_response(json_payload: &str, flags: u8) -> Vec<u8> {
        let payload_bytes = json_payload.as_bytes();
        let mut frame = vec![0x11u8, (MSG_FULL_SERVER_RESPONSE << 4) | flags, (SER_JSON << 4) | COMP_NONE, 0x00];
        frame.extend_from_slice(&1u32.to_be_bytes()); // sequence
        frame.extend_from_slice(&(payload_bytes.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload_bytes);
        frame
    }

    /// 辅助：spawn run_bytedance_session 连 in-process server，收集事件直到 Finished/Failed。
    async fn spawn_bytedance_and_collect(url: String) -> Vec<StreamEvent> {
        let (mut handle, pcm_rx, result_tx) = CloudStreamHandle::new();
        let tx_clone = result_tx.clone();
        tokio::spawn(async move {
            let result = run_bytedance_session(
                pcm_rx, result_tx, &url,
                "testkey".into(), "volc.test".into(), "zh".into(), Vec::new(),
            ).await;
            if let Err(e) = result { let _ = tx_clone.send(StreamEvent::Failed(e.to_string())); }
        });
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if let Some(ev) = handle.try_recv_text() {
                events.push(ev);
                if matches!(events.last(), Some(StreamEvent::Finished) | Some(StreamEvent::Failed(_))) {
                    break;
                }
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
        events
    }

    /// 回归 spec §2.2：有 text（稳态）+ 末帧 → Finished。
    #[tokio::test]
    async fn last_frame_emits_finished_when_text_present() {
        let resp = build_server_response(r#"{"result":{"text":"你好"}}"#, 0x3); // 末帧 flags=0x3
        let server = WsTestServer::start_script(vec![
            Message::Binary(resp),
        ]).await;
        let events = spawn_bytedance_and_collect(server.ws_url()).await;
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::Finished)),
            "有 text + 末帧应发 Finished，实际 events: {:?}", events
        );
    }

    /// 回归 G1/H1：末帧但无 text（last_text 从未非空）→ Failed（不当成功）。
    #[tokio::test]
    async fn last_frame_emits_failed_when_no_text() {
        // 空 text 响应 + 末帧：last_text 仍 None（G1 修复：空 text 不存）→ Failed（H1：空 text 不发 Text）
        let resp = build_server_response(r#"{"result":{"text":""}}"#, 0x3);
        let server = WsTestServer::start_script(vec![
            Message::Binary(resp),
        ]).await;
        let events = spawn_bytedance_and_collect(server.ws_url()).await;
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::Failed(_))),
            "末帧无 text 应发 Failed，实际 events: {:?}", events
        );
        assert!(
            !events.iter().any(|e| matches!(e, StreamEvent::Finished)),
            "末帧无 text 不应发 Finished"
        );
    }

    /// 回归 H1 副作用：[text("你好"), 空 text, 末帧] → Finished + last_text="你好"（空 text 不污染）。
    #[tokio::test]
    async fn last_frame_with_intermediate_empty_text_keeps_valid() {
        let resp1 = build_server_response(r#"{"result":{"text":"你好"}}"#, 0x1); // 正常帧
        let resp2 = build_server_response(r#"{"result":{"text":""}}"#, 0x1);    // 空 text（H1：不发 Text 事件）
        let resp3 = build_server_response(r#"{"result":{"text":""}}"#, 0x3);    // 末帧（last_text=Some("你好") → Finished）
        let server = WsTestServer::start_script(vec![
            Message::Binary(resp1),
            Message::Binary(resp2),
            Message::Binary(resp3),
        ]).await;
        let events = spawn_bytedance_and_collect(server.ws_url()).await;
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::Finished)),
            "[你好, 空, 末帧] last_text=你好 → 应 Finished，实际 events: {:?}", events
        );
        // 断言有 Text("你好") 事件（H1：空 text 不发 Text，你好保留）
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::Text(t) if t == "你好")),
            "应有 Text(\"你好\")（空 text 不覆盖），实际 events: {:?}", events
        );
    }

    /// 回归 spec §2.2：Close 帧 + 有 last_text → Finished（best-effort）。
    #[tokio::test]
    async fn close_frame_emits_finished_when_text_present() {
        let resp = build_server_response(r#"{"result":{"text":"你好"}}"#, 0x1); // 正常帧（非末帧）
        let server = WsTestServer::start_script(vec![
            Message::Binary(resp),
            // server 发完响应后 close（start_script 自动 close）
        ]).await;
        let events = spawn_bytedance_and_collect(server.ws_url()).await;
        // Close 时 last_text=Some("你好") → Text("你好") + Finished
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::Finished)),
            "Close + 有 text 应 Finished，实际 events: {:?}", events
        );
    }

    /// 回归 spec §2.2：Close 帧 + 无 text → Failed。
    #[tokio::test]
    async fn close_frame_emits_failed_when_no_text() {
        let server = WsTestServer::start_script(vec![
            // 不发任何响应，直接 close（start_script 发完空剧本后 close）
        ]).await;
        let events = spawn_bytedance_and_collect(server.ws_url()).await;
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::Failed(_))),
            "Close 无 text 应 Failed，实际 events: {:?}", events
        );
    }
}
