//! 腾讯云实时语音识别流式会话（WebSocket HMAC-SHA1 签名鉴权）。
//!
//! 与 `aliyun_stream.rs` / `bytedance_stream.rs` 的接口完全一致
//!（`push_pcm` / `try_recv_text` / `finish` / `close_async`），
//! 但内部实现腾讯云的 URL 签名鉴权 + 原始 PCM 二进制帧 + JSON 文本响应协议。
//!
//! ## 协议
//!
//! Endpoint 固定：`wss://asr.cloud.tencent.com/asr/v2/<appid>?{params}`
//!
//! 鉴权（签名生成）：
//! 1. 除 `signature` 外所有参数按字典序排序，拼签名原文：
//!    `asr.cloud.tencent.com/asr/v2/<appid>?key1=value1&key2=value2&...`
//! 2. `signature = Base64(HMAC-SHA1(sign_str, SecretKey))`
//! 3. URL-encode signature 后追加到请求 URL
//!
//! 音频：WebSocket Binary 帧，原始 PCM s16le 字节（无额外头、无压缩）。
//! 结束：发 Text 帧 `{"type":"end"}`。
//! 响应：Text 帧 JSON，`result.slice_type`（0=开始 / 1=非稳态 / 2=稳态终态），`final=1` 全部结束。

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha1::Sha1;
use std::collections::BTreeMap;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::client::IntoClientRequest,
    tungstenite::Message,
};

use crate::cloud_types::{CloudStreamHandle, PcmFrame, StreamEvent};

/// HMAC-SHA1 类型别名。
type HmacSha1 = Hmac<Sha1>;

/// 固定 endpoint 前缀（appid 拼在路径段）。
const ENDPOINT_HOST: &str = "asr.cloud.tencent.com";
const ENDPOINT_PATH_PREFIX: &str = "/asr/v2/";

/// 建连 + 推 pre-roll PCM + 启动后台 WS task。
///
/// 参数：
/// - `appid_secretid`：`{appid}:{secretid}` 复合字段（来自 DB `source`）
/// - `secret_key`：SecretKey（来自 DB `secret_key`，用于 HMAC-SHA1 签名）
/// - `engine_model_type`：引擎模型类型（来自 DB `model_name`，如 `16k_zh`）
/// - `language`：语言配置（auto/zh/en，用于选择 engine_model_type 的辅助参考）
/// - `pre_roll_samples`：前导音频（f32[-1,1]）
pub fn open(
    rt: &tauri::async_runtime::RuntimeHandle,
    appid_secretid: String,
    secret_key: String,
    engine_model_type: String,
    _language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    rt.spawn(async move {
        let tx_for_err = result_tx.clone();
        let result = run_tencent_session(
            pcm_rx,
            result_tx,
            appid_secretid,
            secret_key,
            engine_model_type,
            pre_roll_samples,
        )
        .await;
        if let Err(e) = result {
            log::error!("tencent stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}

/// 后台 WS 会话主逻辑：建连 → pre-roll → 双向循环 → 结束信号 → 收结果。
async fn run_tencent_session(
    mut pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    appid_secretid: String,
    secret_key: String,
    engine_model_type: String,
    pre_roll_samples: Vec<f32>,
) -> Result<()> {
    // 1. 解析 appid:secretid
    let (appid, secretid) = appid_secretid
        .split_once(':')
        .context("tencent source 字段格式应为 appid:secretid")?;
    if appid.is_empty() || secretid.is_empty() {
        bail!("tencent appid 或 secretid 为空（source 字段格式 appid:secretid）");
    }

    // 2. 构造签名 URL
    let voice_id = uuid::Uuid::new_v4().to_string();
    let ws_url = build_signed_url(appid, secretid, &secret_key, &engine_model_type, &voice_id)?;

    // 3. 建连
    let request = ws_url.into_client_request().context("tencent WS 请求构造失败")?;
    let (mut ws, _resp) = connect_async(request)
        .await
        .with_context(|| format!("tencent WS 连接失败"))?;

    // 4. 推 pre-roll PCM
    if !pre_roll_samples.is_empty() {
        let pcm = crate::cloud_types::samples_to_pcm_s16le(&pre_roll_samples);
        ws.send(Message::Binary(pcm.into()))
            .await
            .context("tencent WS 发送 pre-roll PCM 失败")?;
    }

    // 5. 双向循环
    // 文本累积：按句 index 存 slice_type=2 稳态文本，partial 覆盖当前句
    let mut stable_segments: BTreeMap<i64, String> = BTreeMap::new();
    let mut current_partial = String::new();

    loop {
        tokio::select! {
            // 收 PCM 指令
            frame = pcm_rx.recv() => {
                match frame {
                    Some(PcmFrame::Samples(pcm)) => {
                        ws.send(Message::Binary(pcm.into()))
                            .await
                            .context("tencent WS 发送音频帧失败")?;
                    }
                    Some(PcmFrame::Finish) => {
                        // 发结束信号 text frame
                        ws.send(Message::Text(r#"{"type":"end"}"#.into()))
                            .await
                            .context("tencent WS 发送 end 信号失败")?;
                    }
                    None => break,
                }
            }
            // 收 WS 响应
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let json: Value = serde_json::from_str(&text)
                            .context("tencent 响应 JSON 解析失败")?;
                        let code = json["code"].as_i64().unwrap_or(-1);
                        if code != 0 {
                            let message = json["message"].as_str().unwrap_or("未知错误");
                            let _ = result_tx.send(StreamEvent::Failed(
                                format!("tencent 错误 {}: {}", code, message)
                            ));
                            return Ok(());
                        }
                        // 检查 final=1（全部识别结束）
                        if json.get("final").and_then(|f| f.as_i64()) == Some(1) {
                            // 最终文本 = 所有稳态句拼接
                            let stable: String = stable_segments.values().cloned().collect();
                            if !stable.is_empty() {
                                let _ = result_tx.send(StreamEvent::Text(stable));
                            }
                            let _ = result_tx.send(StreamEvent::Finished);
                            return Ok(());
                        }
                        // 处理识别结果
                        if let Some(result) = json.get("result") {
                            let slice_type = result["slice_type"].as_i64().unwrap_or(0);
                            let index = result["index"].as_i64().unwrap_or(0);
                            let voice_text = result["voice_text_str"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            match slice_type {
                                2 => {
                                    // 稳态终态：提交此句
                                    stable_segments.insert(index, voice_text);
                                    current_partial.clear();
                                }
                                0 | 1 => {
                                    // 非稳态：更新当前 partial
                                    current_partial = voice_text;
                                }
                                _ => {}
                            }
                            // 发送累积显示文本 = 稳态句 + 当前 partial
                            let stable: String = stable_segments.values().cloned().collect();
                            let display = if current_partial.is_empty() {
                                stable
                            } else {
                                format!("{}{}", stable, current_partial)
                            };
                            if !display.is_empty() {
                                let _ = result_tx.send(StreamEvent::Text(display));
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        // 腾讯 ASR 不发 binary 响应，忽略
                    }
                    Some(Ok(_)) => {} // ping/close 等忽略
                    Some(Err(e)) => {
                        let _ = result_tx.send(StreamEvent::Failed(
                            format!("tencent WS 读错误: {}", e)
                        ));
                        return Ok(());
                    }
                    None => {
                        let _ = result_tx.send(StreamEvent::Failed("WS 连接意外关闭".to_string()));
                        return Ok(());
                    }
                }
            }
        }
    }

    Ok(())
}

// ── 签名 URL 构造 ──

/// 构造签名后的 WebSocket URL。
///
/// 步骤：
/// 1. 收集所有参数（除 signature），按 key 字典序排序
/// 2. 拼签名原文：`asr.cloud.tencent.com/asr/v2/<appid>?k1=v1&k2=v2&...`
/// 3. `signature = Base64(HMAC-SHA1(sign_str, secret_key))`
/// 4. URL-encode signature，拼到 URL 末尾
fn build_signed_url(
    appid: &str,
    secretid: &str,
    secret_key: &str,
    engine_model_type: &str,
    voice_id: &str,
) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let timestamp = now.to_string();
    let expired = (now + 86400).to_string(); // 24h 有效
    let nonce = now.to_string(); // 随机正整数（用时间戳凑数）

    // 收集参数（字典序）
    let mut params: Vec<(&str, String)> = vec![
        ("engine_model_type", engine_model_type.to_string()),
        ("expired", expired),
        ("needvad", "1".to_string()),
        ("nonce", nonce),
        ("secretid", secretid.to_string()),
        ("timestamp", timestamp),
        ("voice_format", "1".to_string()), // PCM
        ("voice_id", voice_id.to_string()),
    ];
    params.sort_by(|a, b| a.0.cmp(&b.0));

    // 拼查询串
    let query: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");

    // 签名原文
    let sign_str = format!("{}{}{}?{}", ENDPOINT_HOST, ENDPOINT_PATH_PREFIX, appid, query);

    // HMAC-SHA1 + Base64
    let mut mac = HmacSha1::new_from_slice(secret_key.as_bytes())
        .map_err(|e| anyhow!("HMAC key 构造失败: {}", e))?;
    mac.update(sign_str.as_bytes());
    let signature_raw = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    // URL-encode signature
    let signature_encoded = percent_encode(&signature_raw);

    // 最终 URL
    Ok(format!(
        "wss://{}{}{}?{}&signature={}",
        ENDPOINT_HOST, ENDPOINT_PATH_PREFIX, appid, query, signature_encoded
    ))
}

/// 腾讯云要求的 URL 编码：编码 `+`、`=`、`/` 等特殊字符。
///
/// 比 standard percent-encode 更保守——腾讯文档强调"必须支持对 +、= 等特殊字符的编码"。
fn percent_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percent_encode_special_chars() {
        // 腾讯文档示例签名：G8jDQBRg1JfeBi/YnTjyjekxfDA=
        let sig = "G8jDQBRg1JfeBi/YnTjyjekxfDA=";
        let encoded = percent_encode(sig);
        assert_eq!(encoded, "G8jDQBRg1JfeBi%2FYnTjyjekxfDA%3D");
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('='));
        assert!(!encoded.contains('+'));
    }

    #[test]
    fn test_percent_encode_alphanumeric() {
        let encoded = percent_encode("hello123");
        assert_eq!(encoded, "hello123");
    }

    #[test]
    fn test_build_signed_url_structure() {
        let url = build_signed_url(
            "1259221234",
            "AKIDtestsecretid",
            "testsecretkey",
            "16k_zh",
            "test-voice-id-12345",
        )
        .unwrap();

        // 验证 URL 结构
        assert!(url.starts_with("wss://asr.cloud.tencent.com/asr/v2/1259221234?"));
        assert!(url.contains("engine_model_type=16k_zh"));
        assert!(url.contains("voice_format=1"));
        assert!(url.contains("needvad=1"));
        assert!(url.contains("voice_id=test-voice-id-12345"));
        assert!(url.contains("secretid=AKIDtestsecretid"));
        assert!(url.contains("&signature="));

        // signature 应已 URL-encode（不含裸 / 或 =）
        let sig_part = url.split("&signature=").nth(1).unwrap();
        assert!(!sig_part.contains('/'));
        assert!(!sig_part.contains('='));
    }

    #[test]
    fn test_build_signed_url_deterministic() {
        // 同样的参数 + 同一时间窗口应产生相同签名（但时间戳会变，所以只验证结构）
        let url1 = build_signed_url(
            "appid1",
            "secretid1",
            "key1",
            "16k_zh",
            "voice-id-1",
        )
        .unwrap();
        let url2 = build_signed_url(
            "appid1",
            "secretid1",
            "key1",
            "16k_zh",
            "voice-id-1",
        )
        .unwrap();
        // 参数部分应相同（时间戳可能差 1s，但结构一致）
        assert_eq!(
            url1.split("&signature=").next(),
            url2.split("&signature=").next()
        );
    }

    #[test]
    fn test_build_signed_url_different_keys() {
        let url1 = build_signed_url("appid", "secretid", "key1", "16k_zh", "voice").unwrap();
        let url2 = build_signed_url("appid", "secretid", "key2", "16k_zh", "voice").unwrap();
        // 不同 SecretKey 应产生不同签名
        let sig1 = url1.split("&signature=").nth(1).unwrap();
        let sig2 = url2.split("&signature=").nth(1).unwrap();
        assert_ne!(sig1, sig2);
    }
}
