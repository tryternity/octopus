//! 百度智能云实时语音识别流式会话（WebSocket START 帧鉴权）。
//!
//! 与 `aliyun_stream.rs` / `bytedance_stream.rs` / `tencent_stream.rs` 的接口完全一致
//!（`push_pcm` / `try_recv_text` / `finish` / `close_async`）。
//!
//! ## 协议
//!
//! Endpoint 固定：`wss://vop.baidu.com/realtime_asr?sn=<UUID>`
//!
//! 鉴权：START 帧 JSON `data` 内直接传 `appid` + `appkey`（API Key），不使用 access_token。
//!
//! 帧类型：
//! - **START**（Text/JSON）：`{"type":"START","data":{appid,appkey,dev_pid,cuid,format:"pcm",sample:16000}}`
//! - **音频数据**（Binary）：原始 PCM s16le（无头、无压缩），建议 160ms = 5120 字节/帧
//! - **FINISH**（Text/JSON）：`{"type":"FINISH"}`
//!
//! 响应（Text/JSON）：
//! - `type=MID_TEXT`：临时结果（非稳态），`result` 字段为当前句 partial
//! - `type=FIN_TEXT`：最终结果（稳态），`result` 为完整句文本
//! - `type=HEARTBEAT`：心跳（忽略）

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::cloud_types::{CloudStreamHandle, PcmFrame, StreamEvent};
use octopus_asr_local::sentence_separator;

/// 固定 endpoint。
const ENDPOINT: &str = "wss://vop.baidu.com/realtime_asr";

/// 建连 + 发 START 帧 + 推 pre-roll PCM + 启动后台 WS task。
///
/// 参数：
/// - `appid`：百度 AppID（来自 DB `source`）
/// - `appkey`：百度 API Key（来自 DB `secret_key`）
/// - `dev_pid`：语种模型 PID 字符串（来自 DB `model_name`，如 `"15372"`）
/// - `language`：语言配置（百度用 dev_pid 选模型；language 用于句间分隔符——
///   英文插空格避免单词粘连，中文逗号断句）
/// - `pre_roll_samples`：前导音频（f32[-1,1]）
///
/// **须在 tokio runtime 上下文调用**。
pub fn open(
    appid: String,
    appkey: String,
    dev_pid: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result =
            run_baidu_session(pcm_rx, result_tx, ENDPOINT, appid, appkey, dev_pid, language, pre_roll_samples)
                .await;
        // session 契约：Ok = 已通过 result_tx 通知最终结果（Finished/运行期 Failed，
        // 见 run_baidu_session 内 WS 错误分支 return Ok 处）；仅 Err（签名/建连等启动期失败，
        // 未及经 channel 通知）在此补发一次 Failed——避免与 session 内部已发的 Failed 重复。
        if let Err(e) = result {
            log::error!("baidu stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}

/// 后台 WS 会话主逻辑：建连 → START → pre-roll → 双向循环 → FINISH → 收结果。
///
/// `endpoint` 参数化（P2-1 WS mock）：prod 调用传真 `const ENDPOINT`（`wss://...`），
/// 测试用 in-process server 时传 `ws://127.0.0.1:{port}`（见 `test_ws_server`）。
async fn run_baidu_session(
    mut pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    endpoint: &str,
    appid: String,
    appkey: String,
    dev_pid: String,
    language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<()> {
    // 1. 解析 appid / dev_pid 字符串为整数（fail-fast：配置错误时明确报错，而非静默发 0）
    let appid_int: i64 = appid
        .parse()
        .with_context(|| format!("baidu appid '{}' 不是有效整数（应为百度控制台 AppID）", appid))?;
    let dev_pid_int: i64 = dev_pid
        .parse()
        .with_context(|| format!("baidu dev_pid '{}' 不是有效整数", dev_pid))?;

    // 2. 构造 sn（UUID，用于排查日志）
    let sn = uuid::Uuid::new_v4().to_string();
    let cuid = sn.clone(); // cuid 也用 UUID（统计 UV，不影响识别）

    // 3. 建连
    let ws_url = format!("{}?sn={}", endpoint, sn);
    let (mut ws, _resp) = tokio::time::timeout(
        std::time::Duration::from_secs(octopus_infra::net::WS_CONNECT_TIMEOUT_SECS),
        connect_async(&ws_url),
    )
    .await
    .map_err(|_| anyhow::anyhow!("baidu WS connect timeout"))?
    .with_context(|| format!("baidu WS 连接失败: {}", ws_url))?;

    // 4. 发 START 帧
    let start_frame = json!({
        "type": "START",
        "data": {
            "appid": appid_int,
            "appkey": appkey,
            "dev_pid": dev_pid_int,
            "cuid": cuid,
            "format": "pcm",
            "sample": 16000,
        }
    });
    ws.send(Message::Text(start_frame.to_string()))
        .await
        .context("baidu WS 发送 START 帧失败")?;

    // 5. 推 pre-roll PCM
    if !pre_roll_samples.is_empty() {
        let pcm = crate::cloud_types::samples_to_pcm_s16le(&pre_roll_samples);
        ws.send(Message::Binary(pcm))
            .await
            .context("baidu WS 发送 pre-roll PCM 失败")?;
    }

    // 6. 双向循环
    // 文本累积：FIN_TEXT 存入 Vec（按顺序拼接），MID_TEXT 覆盖 current_partial
    let mut fin_texts: Vec<String> = Vec::new();
    let mut current_partial = String::new();

    loop {
        tokio::select! {
            // 收 PCM 指令
            frame = pcm_rx.recv() => {
                match frame {
                    Some(PcmFrame::Samples(pcm)) => {
                        ws.send(Message::Binary(pcm))
                            .await
                            .context("baidu WS 发送音频帧失败")?;
                    }
                    Some(PcmFrame::Finish) => {
                        // 发 FINISH 帧
                        ws.send(Message::Text(r#"{"type":"FINISH"}"#.into()))
                            .await
                            .context("baidu WS 发送 FINISH 帧失败")?;
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
                        let _ = result_tx.send(StreamEvent::Failed("baidu WS read timeout".into()));
                        return Ok(());
                    }
                    Ok(None) => break,
                    Ok(Some(Err(e))) => {
                        let _ = result_tx.send(StreamEvent::Failed(format!("baidu WS 读错误: {}", e)));
                        return Ok(());
                    }
                    Ok(Some(Ok(m))) => m,
                };
                match msg {
                    Message::Text(text) => {
                        let json: Value = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(e) => {
                                log::warn!("baidu JSON 解析失败: {}（text={}）", e, text);
                                continue;
                            }
                        };
                        let err_no = json["err_no"].as_i64().unwrap_or(0);
                        if err_no != 0 {
                            let err_msg = json["err_msg"].as_str().unwrap_or("未知错误");
                            let _ = result_tx.send(StreamEvent::Failed(
                                format!("baidu 错误 {}: {}", err_no, err_msg)
                            ));
                            return Ok(());
                        }
                        let msg_type = json["type"].as_str().unwrap_or("");
                        match msg_type {
                            "MID_TEXT" => {
                                // 临时结果（非稳态）
                                current_partial = json["result"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string();
                                let display = accumulate_display(&fin_texts, &current_partial, &language);
                                if !display.is_empty() {
                                    let _ = result_tx.send(StreamEvent::Text(display));
                                }
                            }
                            "FIN_TEXT" => {
                                // 最终结果（稳态）——提交此句
                                let result = json["result"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string();
                                fin_texts.push(result);
                                current_partial.clear();
                                let display = accumulate_display(&fin_texts, &current_partial, &language);
                                if !display.is_empty() {
                                    let _ = result_tx.send(StreamEvent::Text(display));
                                }
                            }
                            "HEARTBEAT" => {
                                // 心跳，忽略
                            }
                            _ => {
                                log::debug!("baidu: 未知消息类型 '{}'", msg_type);
                            }
                        }
                    }
                    Message::Close(_) => {
                        // 服务端主动 Close（鉴权失败/超时/限流等）。按是否收到过稳态结果
                        // （FIN_TEXT，即 fin_texts 非空）判断：
                        // - 有稳态 → Finished（display 一定非空，因 fin_texts.join 非空）
                        // - 仅 partial（fin_texts 空但 current_partial 非空）→ Failed
                        //   （旧实现仅查 display 非空就发 Finished，把不稳态 partial 当最终结果）
                        // - 全空 → Failed
                        log::debug!("baidu: WS 连接关闭");
                        let stable = !fin_texts.is_empty();
                        if stable {
                            let display = accumulate_display(&fin_texts, &current_partial, &language);
                            let _ = result_tx.send(StreamEvent::Text(display));
                            let _ = result_tx.send(StreamEvent::Finished);
                        } else if !current_partial.is_empty() {
                            let _ = result_tx.send(StreamEvent::Failed(
                                "baidu WS 连接关闭但仅收到非稳态 partial".into()
                            ));
                        } else {
                            let _ = result_tx.send(StreamEvent::Failed(
                                "baidu WS 连接关闭但未收到识别结果".into()
                            ));
                        }
                        return Ok(());
                    }
                    Message::Binary(_) => {
                        // 百度不发 binary 响应，忽略
                    }
                    _ => {} // ping 等忽略
                }
            }
        }
    }

    Ok(())
}

/// 拼接稳态句 + 当前 partial 为显示文本。
///
/// 稳态句之间插入分隔符（英文空格 / 中文逗号），避免多句直接 concat 导致英文单词
/// 粘连（`"hello world"+"today"→"helloworldtoday"`）。
///
/// stable↔partial 之间也插 sep（修复 F2）：partial 是新句开头，与上一稳态句之间是
/// 句间关系，英文需空格分隔（`"hello world"+"to"→"hello world to"` 而非
/// `"hello worldto"`）。仅当 `!fin_texts.is_empty()`（已有稳态句）且 partial 非空时加——
/// 首句 partial（fin_texts 空）前不加，避免前导空格。
fn accumulate_display(fin_texts: &[String], current_partial: &str, language: &str) -> String {
    let sep = sentence_separator(language);
    let stable: String = fin_texts.join(sep);
    if current_partial.is_empty() {
        stable
    } else if stable.is_empty() {
        // 首句 partial（无稳态句）——直接用 partial，不加前导 sep
        current_partial.to_string()
    } else {
        // 有稳态句 + partial ——stable 与 partial 间插 sep（句间分隔）
        format!("{}{}{}", stable, sep, current_partial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulate_display_empty() {
        assert_eq!(accumulate_display(&[], "", "zh"), "");
    }

    #[test]
    fn test_accumulate_display_stable_only() {
        let fin = vec!["你好".to_string(), "世界".to_string()];
        // 中文稳态句之间插「，」分隔符（避免直接粘连）
        assert_eq!(accumulate_display(&fin, "", "zh"), "你好，世界");
    }

    #[test]
    fn test_accumulate_display_with_partial() {
        let fin = vec!["你好".to_string()];
        // 稳态句 + partial 之间插中文逗号（句间分隔）
        assert_eq!(accumulate_display(&fin, "世", "zh"), "你好，世");
    }

    #[test]
    fn test_accumulate_display_partial_only() {
        assert_eq!(accumulate_display(&[], "你好", "zh"), "你好");
    }

    /// 回归 #5：英文多句拼接需插空格分隔符，否则单词粘连不可用。
    #[test]
    fn test_accumulate_display_english_separator() {
        let fin = vec!["hello world".to_string(), "today is good".to_string()];
        // 英文稳态句之间插空格 → "hello world today is good"（而非 "hello worldtoday is good"）
        assert_eq!(accumulate_display(&fin, "", "en"), "hello world today is good");
    }

    /// 回归 #5 + F2：英文稳态句 + partial，partial 前插空格（句间分隔）。
    /// 旧实现 "hello world"+"to" → "hello worldto"（粘连），新 → "hello world to"。
    #[test]
    fn test_accumulate_display_english_with_partial() {
        let fin = vec!["hello world".to_string()];
        assert_eq!(accumulate_display(&fin, "to", "en"), "hello world to");
    }

    /// 回归 F2：首句 partial（无稳态句）不加前导分隔符。
    #[test]
    fn test_accumulate_display_first_partial_no_leading_sep() {
        // fin_texts 空 + partial 非空 → 直接 partial，无前导空格/逗号
        assert_eq!(accumulate_display(&[], "hello", "en"), "hello");
        assert_eq!(accumulate_display(&[], "你好", "zh"), "你好");
    }

    /// 回归 #11：Close 分支稳态判定逻辑（fin_texts 非空 = 稳态）。
    /// 这里测 accumulate_display 在 fin_texts 非空时一定返回非空（保证 Close 的 stable 分支安全发 Finished）。
    #[test]
    fn test_accumulate_display_stable_never_empty_when_fin_texts_present() {
        let fin = vec!["some result".to_string()];
        assert!(!accumulate_display(&fin, "", "en").is_empty());
        assert!(!accumulate_display(&fin, "partial", "en").is_empty());
    }

    #[test]
    fn test_start_frame_json_structure() {
        let frame = json!({
            "type": "START",
            "data": {
                "appid": 1050000017i64,
                "appkey": "UA4oPSxxxxkGOuFbb6",
                "dev_pid": 15372i64,
                "cuid": "test-cuid",
                "format": "pcm",
                "sample": 16000i64,
            }
        });
        assert_eq!(frame["type"].as_str(), Some("START"));
        assert_eq!(frame["data"]["format"].as_str(), Some("pcm"));
        assert_eq!(frame["data"]["sample"].as_i64(), Some(16000));
        assert_eq!(frame["data"]["dev_pid"].as_i64(), Some(15372));
    }

    #[test]
    fn test_dev_pid_parse() {
        assert_eq!("15372".parse::<i64>().unwrap(), 15372);
        assert_eq!("1737".parse::<i64>().unwrap(), 1737);
        assert!("invalid".parse::<i64>().is_err());
    }

    // ── P2-1 WS mock：Close 帧终态测试（spec §2.2）──
    // 用 in-process tokio-tungstenite server（test_ws_server harness）真走一遍 WS 协议，
    // 覆盖 run_baidu_session 的 Close 分支稳态判定逻辑。

    use crate::test_ws_server::WsTestServer;
    use crate::cloud_types::{CloudStreamHandle, StreamEvent};

    /// 回归 spec §2.2：收到 FIN_TEXT（稳态）后 Close → 应发 Text + Finished。
    #[tokio::test]
    async fn close_frame_emits_finished_when_stable() {
        // server 按剧本发：FIN_TEXT（稳态）→ Close。不读 client（避开握手时序）。
        let server = WsTestServer::start_script(vec![
            Message::Text(r#"{"type":"FIN_TEXT","result":"你好","err_no":0}"#.into()),
        ]).await;
        let url = server.ws_url();
        let (mut handle, pcm_rx, result_tx) = CloudStreamHandle::new();
        let tx_clone = result_tx.clone();
        tokio::spawn(async move {
            let result = run_baidu_session(
                pcm_rx, result_tx, &url,
                "1050000017".into(), "testkey".into(), "15372".into(),
                "zh".into(), Vec::new(),
            ).await;
            if let Err(e) = result { let _ = tx_clone.send(StreamEvent::Failed(e.to_string())); }
        });

        // 收集事件（最多 2s，等 connect + 握手 + 收消息）
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
        assert!(
            events.iter().any(|e| matches!(e, StreamEvent::Finished)),
            "稳态 Close 应发 Finished，实际 events: {:?}", events
        );
    }

    /// 回归 spec §2.2：仅 MID_TEXT（非稳态）后 Close → 应发 Failed（不发 Finished）。
    #[tokio::test]
    async fn close_frame_emits_failed_when_no_stable() {
        let server = WsTestServer::start_script(vec![
            Message::Text(r#"{"type":"MID_TEXT","result":"部分识别","err_no":0}"#.into()),
        ]).await;
        let url = server.ws_url();
        let (mut handle, pcm_rx, result_tx) = CloudStreamHandle::new();
        let tx_clone = result_tx.clone();
        tokio::spawn(async move {
            let result = run_baidu_session(
                pcm_rx, result_tx, &url,
                "1050000017".into(), "testkey".into(), "15372".into(),
                "zh".into(), Vec::new(),
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
