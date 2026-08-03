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

/// 建连（含签名 URL）+ 推 pre-roll PCM + 启动后台 WS task。
///
/// 参数：
/// - `appid_secretid`：`{appid}:{secretid}` 复合字段（来自 DB `source`）
/// - `secret_key`：SecretKey（来自 DB `secret_key`，用于 HMAC-SHA1 签名）
/// - `engine_model_type`：引擎模型类型（来自 DB `model_name`，如 `16k_zh`）
/// - `language`：语言配置（auto/zh/en，用于选择 engine_model_type 的辅助参考）
/// - `pre_roll_samples`：前导音频（f32[-1,1]）
///
/// **须在 tokio runtime 上下文调用**。
pub fn open(
    appid_secretid: String,
    secret_key: String,
    engine_model_type: String,
    _language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    // 签名在 open() 完成（启动期失败经 Err 补发 Failed）；run 函数接收已签名 URL。
    let (appid, secretid) = appid_secretid
        .split_once(':')
        .context("tencent source 字段格式应为 appid:secretid")?;
    if appid.is_empty() || secretid.is_empty() {
        bail!("tencent appid 或 secretid 为空（source 字段格式 appid:secretid）");
    }
    let voice_id = uuid::Uuid::new_v4().to_string();
    let ws_url = build_signed_url(appid, secretid, &secret_key, &engine_model_type, &voice_id)?;

    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result = run_tencent_session(
            pcm_rx,
            result_tx,
            ws_url,
            pre_roll_samples,
        )
        .await;
        // session 契约：Ok = 已通过 result_tx 通知最终结果（Finished/运行期 Failed，
        // 见 run_tencent_session 内 WS 错误分支 return Ok 处）；仅 Err（签名/建连等启动期失败，
        // 未及经 channel 通知）在此补发一次 Failed——避免与 session 内部已发的 Failed 重复。
        if let Err(e) = result {
            log::error!("tencent stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}

/// 后台 WS 会话主逻辑：建连 → pre-roll → 双向循环 → 结束信号 → 收结果。
///
/// `ws_url` 参数化（P2-1 WS mock）：prod 调用传真签名 URL（`wss://...&signature=...`），
/// 测试用 in-process server 时传 `ws://127.0.0.1:{port}`（不校验签名，见 `test_ws_server`）。
async fn run_tencent_session(
    mut pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    ws_url: String,
    pre_roll_samples: Vec<f32>,
) -> Result<()> {
    // 1. 建连（URL 已在 open() 签名完成）
    let request = ws_url.into_client_request().context("tencent WS 请求构造失败")?;
    let (mut ws, _resp) = tokio::time::timeout(
        std::time::Duration::from_secs(octopus_infra::net::WS_CONNECT_TIMEOUT_SECS),
        connect_async(request),
    )
    .await
    .map_err(|_| anyhow::anyhow!("tencent WS connect timeout"))?
    .context("tencent WS 连接失败")?;

    // 4. 推 pre-roll PCM
    if !pre_roll_samples.is_empty() {
        let pcm = crate::cloud_types::samples_to_pcm_s16le(&pre_roll_samples);
        ws.send(Message::Binary(pcm))
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
                        ws.send(Message::Binary(pcm))
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
            // 收 WS 响应（加读取超时）
            msg = tokio::time::timeout(
                std::time::Duration::from_secs(octopus_infra::net::WS_READ_TIMEOUT_SECS),
                ws.next(),
            ) => {
                let msg = match msg {
                    Err(_) => {
                        let _ = result_tx.send(StreamEvent::Failed("tencent WS read timeout".into()));
                        return Ok(());
                    }
                    Ok(None) => break,
                    Ok(Some(Err(e))) => {
                        let _ = result_tx.send(StreamEvent::Failed(format!("tencent WS 读错误: {}", e)));
                        return Ok(());
                    }
                    Ok(Some(Ok(m))) => m,
                };
                match msg {
                    Message::Text(text) => {
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
                    Message::Binary(_) => {
                        // 腾讯 ASR 不发 binary 响应，忽略
                    }
                    Message::Close(_) => {
                        // 服务端主动 Close（鉴权失败/超时/限流等）。旧实现落 _ => {} 忽略，
                        // 随后 ws.next() 返 Ok(None) → break → return Ok(()) 无终态事件，
                        // close_async 把 partial 当成功（#3）。现显式处理：有稳态结果发
                        // Finished，否则 Failed 暴露异常（参照 baidu_stream.rs:214）。
                        log::debug!("tencent: WS 连接关闭");
                        let stable: String = stable_segments.values().cloned().collect();
                        if !stable.is_empty() {
                            let _ = result_tx.send(StreamEvent::Text(stable));
                            let _ = result_tx.send(StreamEvent::Finished);
                        } else {
                            let _ = result_tx.send(StreamEvent::Failed(
                                "tencent WS 连接关闭但未收到稳态识别结果".into()
                            ));
                        }
                        return Ok(());
                    }
                    _ => {} // ping 等忽略
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
    // 随机正整数（≤10 位，符合腾讯文档 + specs/2026-06-21-tencent-asr-design.md）。
    // 取 uuid v4 低 32 位：u32::MAX=4294967295 恰好 10 位，满足位数约束且真随机。
    // 不复用 timestamp：同秒多请求/未来并发场景下 nonce 须唯一；voice_id 虽唯一已能
    // 避免被误判重放，但按文档用随机数更稳妥。复用 uuid v4 随机性，避免引入 rand 依赖。
    // +1 保证非 0：u32 低 32 位理论全 0（概率 2^-32）时 nonce="0" 非正整数可能被签名拒；
    // +1 后 [1, 2^32] 均为正整数，最坏 nonce=1（仍合法）。
    let nonce = ((uuid::Uuid::new_v4().as_u128() as u32).saturating_add(1)).to_string();

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
    params.sort_by(|a, b| a.0.cmp(b.0));

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
    fn test_build_signed_url_repeatable_structure() {
        // nonce 随机化后，两次调用的 query 不再字节相等（nonce/timestamp 每次不同）；
        // 改为验证每次调用都生成结构合法的 URL：必填字段齐全 + signature 已 URL-encode。
        for _ in 0..2 {
            let url = build_signed_url(
                "appid1",
                "secretid1",
                "key1",
                "16k_zh",
                "voice-id-1",
            )
            .unwrap();
            assert!(url.starts_with("wss://asr.cloud.tencent.com/asr/v2/appid1?"));
            for field in &[
                "engine_model_type=16k_zh",
                "voice_id=voice-id-1",
                "secretid=secretid1",
                "nonce=",
                "timestamp=",
                "expired=",
                "voice_format=1",
                "needvad=1",
                "&signature=",
            ] {
                assert!(url.contains(field), "缺少字段 {}，url={}", field, url);
            }
            // nonce 应为纯数字随机正整数，且 ≤10 位（spec 约束：u32 范围）
            let nonce_val = url
                .split("nonce=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .unwrap();
            let nonce_num: u64 = nonce_val.parse().expect("nonce 非数字");
            assert!(
                nonce_num <= u32::MAX as u64,
                "nonce 超 10 位约束：{}",
                nonce_val
            );
            // signature 应已 URL-encode（不含裸 / 或 =）
            let sig = url.split("&signature=").nth(1).unwrap();
            assert!(!sig.contains('/'));
            assert!(!sig.contains('='));
        }
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

    // ── P2-1 WS mock：Close 帧终态测试（spec §2.2）──

    use crate::test_ws_server::WsTestServer;
    use crate::cloud_types::{CloudStreamHandle, StreamEvent};

    /// 辅助：spawn run_tencent_session 连 in-process server，收集事件直到 Finished/Failed。
    async fn spawn_tencent_and_collect(url: String) -> Vec<StreamEvent> {
        let (mut handle, pcm_rx, result_tx) = CloudStreamHandle::new();
        let tx_clone = result_tx.clone();
        tokio::spawn(async move {
            let result = run_tencent_session(pcm_rx, result_tx, url, Vec::new()).await;
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

    /// 回归 spec §2.2：收到稳态（slice_type=2）后 Close → Finished。
    #[tokio::test]
    async fn close_frame_emits_finished_when_stable() {
        let server = WsTestServer::start_script(vec![
            // 稳态结果：code=0 + result.slice_type=2
            Message::Text(r#"{"code":0,"result":{"slice_type":2,"index":0,"voice_text_str":"你好"}}"#.into()),
        ]).await;
        let events = spawn_tencent_and_collect(server.ws_url()).await;
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::Finished)),
            "稳态 Close 应发 Finished，实际 events: {:?}", events
        );
    }

    /// 回归 spec §2.2：仅非稳态（slice_type=0/1）后 Close → Failed。
    #[tokio::test]
    async fn close_frame_emits_failed_when_no_stable() {
        let server = WsTestServer::start_script(vec![
            // 非稳态 partial：slice_type=1
            Message::Text(r#"{"code":0,"result":{"slice_type":1,"index":0,"voice_text_str":"部分"}}"#.into()),
        ]).await;
        let events = spawn_tencent_and_collect(server.ws_url()).await;
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::Failed(_))),
            "非稳态 Close 应发 Failed，实际 events: {:?}", events
        );
        assert!(
            !events.iter().any(|e| matches!(e, StreamEvent::Finished)),
            "非稳态 Close 不应发 Finished"
        );
    }
}
