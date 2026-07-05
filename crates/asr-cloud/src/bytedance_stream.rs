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
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::io::{Read, Write};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::client::IntoClientRequest,
    tungstenite::http::HeaderName,
    tungstenite::http::HeaderValue,
    tungstenite::Message,
};

use crate::cloud_types::{CloudStreamHandle, PcmFrame, StreamEvent};

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

#[allow(dead_code)]
const COMP_NONE: u8 = 0x0;
const COMP_GZIP: u8 = 0x1;

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
            run_bytedance_session(pcm_rx, result_tx, api_key, resource_id, language, pre_roll_samples)
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
async fn run_bytedance_session(
    mut pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    api_key: String,
    resource_id: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<()> {
    // 1. 建连（带火山引擎特有的握手 headers）
    let mut request = ENDPOINT.into_client_request()
        .context("bytedance WS 请求构造失败")?;
    let headers = request.headers_mut();
    headers.insert(
        HeaderName::from_static("x-api-key"),
        HeaderValue::from_str(&api_key).context("bytedance X-Api-Key 构造失败")?,
    );
    headers.insert(
        HeaderName::from_static("x-api-resource-id"),
        HeaderValue::from_str(&resource_id)
            .context("bytedance X-Api-Resource-Id 构造失败")?,
    );
    let request_id = uuid::Uuid::new_v4().to_string();
    headers.insert(
        HeaderName::from_static("x-api-request-id"),
        HeaderValue::from_str(&request_id)
            .context("bytedance X-Api-Request-Id 构造失败")?,
    );
    headers.insert(
        HeaderName::from_static("x-api-sequence"),
        HeaderValue::from_static("-1"),
    );

    let (mut ws, _resp) = tokio::time::timeout(
        std::time::Duration::from_secs(octopus_infra::net::WS_CONNECT_TIMEOUT_SECS),
        connect_async(request),
    )
    .await
    .map_err(|_| anyhow::anyhow!("bytedance WS connect timeout"))?
    .with_context(|| format!("bytedance WS 连接失败: {}", ENDPOINT))?;

    // 2. 发 FULL_CLIENT_REQUEST（初始 JSON config，gzip 压缩）
    let lang = if language.is_empty() || language == "auto" {
        "zh-CN".to_string()
    } else if language == "zh" {
        "zh-CN".to_string()
    } else if language == "en" {
        "en-US".to_string()
    } else {
        format!("{}-CN", language)
    };
    let config = json!({
        "user": { "uid": request_id },
        "audio": {
            "format": "pcm",
            "codec": "raw",
            "rate": 16000,
            "bits": 16,
            "channel": 1,
            "language": lang,
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
    ws.send(Message::binary(init_frame))
        .await
        .context("bytedance WS 发送初始 config 失败")?;

    // 3. 推 pre-roll PCM
    if !pre_roll_samples.is_empty() {
        let pcm = crate::cloud_types::samples_to_pcm_s16le(&pre_roll_samples);
        let audio_frame = build_client_frame(
            MSG_AUDIO_ONLY_REQUEST,
            FLAG_NO_SEQUENCE,
            SER_NONE,
            COMP_GZIP,
            &pcm,
        )?;
        ws.send(Message::binary(audio_frame))
            .await
            .context("bytedance WS 发送 pre-roll PCM 失败")?;
    }

    // 4. 双向循环
    loop {
        tokio::select! {
            // 收 PCM 指令
            frame = pcm_rx.recv() => {
                match frame {
                    Some(PcmFrame::Samples(pcm)) => {
                        let audio_frame = build_client_frame(
                            MSG_AUDIO_ONLY_REQUEST,
                            FLAG_NO_SEQUENCE,
                            SER_NONE,
                            COMP_GZIP,
                            &pcm,
                        )?;
                        ws.send(Message::binary(audio_frame))
                            .await
                            .context("bytedance WS 发送音频帧失败")?;
                    }
                    Some(PcmFrame::Finish) => {
                        // 末帧（负包 = NEG_SEQUENCE）——告诉服务端音频结束
                        let last_frame = build_client_frame(
                            MSG_AUDIO_ONLY_REQUEST,
                            FLAG_NEG_SEQUENCE,
                            SER_NONE,
                            COMP_GZIP,
                            &[],
                        )?;
                        ws.send(Message::binary(last_frame))
                            .await
                            .context("bytedance WS 发送末帧失败")?;
                    }
                    None => break,
                }
            }
            // 收 WS 响应（加读取超时）
            msg = tokio::time::timeout(
                std::time::Duration::from_secs(octopus_infra::net::WS_READ_TIMEOUT_SECS),
                ws.next(),
            ) => {
                let msg = match msg {
                    Err(_) => {
                        let _ = result_tx.send(StreamEvent::Failed("bytedance WS read timeout".into()));
                        return Ok(());
                    }
                    Ok(None) => break,
                    Ok(Some(Err(e))) => {
                        let _ = result_tx.send(StreamEvent::Failed(format!("bytedance WS 读错误: {}", e)));
                        return Ok(());
                    }
                    Ok(Some(Ok(m))) => m,
                };
                match msg {
                    Message::Binary(data) => {
                        let parsed = parse_server_frame(&data)
                            .context("bytedance WS 响应解析失败")?;
                        match parsed.msg_type {
                            MSG_FULL_SERVER_RESPONSE => {
                                let json_str = decompress_or_raw(
                                    &parsed.payload,
                                    parsed.compression == COMP_GZIP,
                                )?;
                                let json_val: Value = serde_json::from_str(&json_str)
                                    .context("bytedance 响应 JSON 解析失败")?;
                                if let Some(text) = json_val["result"]["text"].as_str() {
                                    let _ = result_tx.send(StreamEvent::Text(text.to_string()));
                                }
                                if parsed.flags == 0x3 {
                                    // 末帧响应（NEG_WITH_SEQUENCE）——全部结束
                                    let _ = result_tx.send(StreamEvent::Finished);
                                    return Ok(());
                                }
                            }
                            MSG_ERROR_RESPONSE => {
                                let error_code = parsed.sequence; // 错误帧的 sequence 位存 error code
                                let error_msg = String::from_utf8_lossy(&parsed.payload);
                                let _ = result_tx.send(StreamEvent::Failed(
                                    format!("bytedance 错误 {}: {}", error_code, error_msg)
                                ));
                                return Ok(());
                            }
                            _ => {
                                log::debug!("bytedance: 未知消息类型 0x{:X}", parsed.msg_type);
                            }
                        }
                    }
                    _ => {} // text/close/ping 等忽略
                }
            }
        }
    }

    Ok(())
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
    serialization: u8,
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
    let byte0 = data[0];
    let byte1 = data[1];
    let byte2 = data[2];
    let _byte3 = data[3];

    let msg_type = (byte1 >> 4) & 0x0F;
    let flags = byte1 & 0x0F;
    let serialization = (byte2 >> 4) & 0x0F;
    let compression = byte2 & 0x0F;

    let _ = byte0; // version + header_size（固定 0x11，不校验）

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
        serialization,
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
    fn test_build_client_frame_audio() {
        let payload = b"test_audio";
        let frame = build_client_frame(
            MSG_AUDIO_ONLY_REQUEST,
            FLAG_NO_SEQUENCE,
            SER_NONE,
            COMP_GZIP,
            payload,
        ).unwrap();
        // header 4B + payload_size 4B + gzip(payload)
        assert_eq!(frame[0], 0x11); // ver=1, hdr=1
        assert_eq!((frame[1] >> 4) & 0xF, MSG_AUDIO_ONLY_REQUEST);
        assert_eq!(frame[1] & 0xF, FLAG_NO_SEQUENCE);
        assert_eq!((frame[2] >> 4) & 0xF, SER_NONE);
        assert_eq!(frame[2] & 0xF, COMP_GZIP);
    }

    #[test]
    fn test_build_client_frame_last() {
        let frame = build_client_frame(
            MSG_AUDIO_ONLY_REQUEST,
            FLAG_NEG_SEQUENCE,
            SER_NONE,
            COMP_GZIP,
            &[],
        ).unwrap();
        // 末帧 flags = 0x2 (NEG_SEQUENCE)
        assert_eq!(frame[1] & 0xF, FLAG_NEG_SEQUENCE);
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

        let mut frame = Vec::new();
        frame.push(0x11); // ver=1, hdr=1
        frame.push(0x91); // msg=FULL_SERVER_RESPONSE(9), flags=POS_SEQUENCE(1)
        frame.push(0x11); // ser=JSON(1), comp=GZIP(1)
        frame.push(0x00); // reserved
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
        let mut frame = Vec::new();
        frame.push(0x11);
        frame.push(0xF1); // msg=ERROR_RESPONSE(F), flags=POS_SEQUENCE(1)
        frame.push(0x00); // ser=NONE, comp=NONE
        frame.push(0x00);
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
}
