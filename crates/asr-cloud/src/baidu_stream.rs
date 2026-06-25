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

/// 固定 endpoint。
const ENDPOINT: &str = "wss://vop.baidu.com/realtime_asr";

/// 建连 + 发 START 帧 + 推 pre-roll PCM + 启动后台 WS task。
///
/// 参数：
/// - `appid`：百度 AppID（来自 DB `source`）
/// - `appkey`：百度 API Key（来自 DB `secret_key`）
/// - `dev_pid`：语种模型 PID 字符串（来自 DB `model_name`，如 `"15372"`）
/// - `_language`：语言配置（百度用 dev_pid 选模型，此参数保留兼容）
/// - `pre_roll_samples`：前导音频（f32[-1,1]）
///
/// **须在 tokio runtime 上下文调用**。
pub fn open(
    appid: String,
    appkey: String,
    dev_pid: String,
    _language: String,
    pre_roll_samples: Vec<f32>,
) -> Result<CloudStreamHandle> {
    let (handle, pcm_rx, result_tx) = CloudStreamHandle::new();
    tokio::spawn(async move {
        let tx_for_err = result_tx.clone();
        let result =
            run_baidu_session(pcm_rx, result_tx, appid, appkey, dev_pid, pre_roll_samples)
                .await;
        if let Err(e) = result {
            log::error!("baidu stream session error: {}", e);
            let _ = tx_for_err.send(StreamEvent::Failed(e.to_string()));
        }
    });
    Ok(handle)
}

/// 后台 WS 会话主逻辑：建连 → START → pre-roll → 双向循环 → FINISH → 收结果。
async fn run_baidu_session(
    mut pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    appid: String,
    appkey: String,
    dev_pid: String,
    pre_roll_samples: Vec<f32>,
) -> Result<()> {
    // 1. 解析 dev_pid 字符串为整数
    let dev_pid_int: i64 = dev_pid
        .parse()
        .with_context(|| format!("baidu dev_pid '{}' 不是有效整数", dev_pid))?;

    // 2. 构造 sn（UUID，用于排查日志）
    let sn = uuid::Uuid::new_v4().to_string();
    let cuid = sn.clone(); // cuid 也用 UUID（统计 UV，不影响识别）

    // 3. 建连
    let ws_url = format!("{}?sn={}", ENDPOINT, sn);
    let (mut ws, _resp) = connect_async(&ws_url)
        .await
        .with_context(|| format!("baidu WS 连接失败: {}", ws_url))?;

    // 4. 发 START 帧
    let start_frame = json!({
        "type": "START",
        "data": {
            "appid": appid.parse::<i64>().unwrap_or(0),
            "appkey": appkey,
            "dev_pid": dev_pid_int,
            "cuid": cuid,
            "format": "pcm",
            "sample": 16000,
        }
    });
    ws.send(Message::Text(start_frame.to_string().into()))
        .await
        .context("baidu WS 发送 START 帧失败")?;

    // 5. 推 pre-roll PCM
    if !pre_roll_samples.is_empty() {
        let pcm = crate::cloud_types::samples_to_pcm_s16le(&pre_roll_samples);
        ws.send(Message::Binary(pcm.into()))
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
                        ws.send(Message::Binary(pcm.into()))
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
            // 收 WS 响应
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
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
                                let display = accumulate_display(&fin_texts, &current_partial);
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
                                let display = accumulate_display(&fin_texts, &current_partial);
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
                    Some(Ok(Message::Close(_))) => {
                        log::debug!("baidu: WS 连接关闭");
                        let display = accumulate_display(&fin_texts, &current_partial);
                        if !display.is_empty() {
                            let _ = result_tx.send(StreamEvent::Text(display));
                        }
                        let _ = result_tx.send(StreamEvent::Finished);
                        return Ok(());
                    }
                    Some(Ok(Message::Binary(_))) => {
                        // 百度不发 binary 响应，忽略
                    }
                    Some(Ok(_)) => {} // ping 等忽略
                    Some(Err(e)) => {
                        let _ = result_tx.send(StreamEvent::Failed(
                            format!("baidu WS 读错误: {}", e)
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

/// 拼接稳态句 + 当前 partial 为显示文本。
fn accumulate_display(fin_texts: &[String], current_partial: &str) -> String {
    let stable: String = fin_texts.concat();
    if current_partial.is_empty() {
        stable
    } else {
        format!("{}{}", stable, current_partial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulate_display_empty() {
        assert_eq!(accumulate_display(&[], ""), "");
    }

    #[test]
    fn test_accumulate_display_stable_only() {
        let fin = vec!["你好".to_string(), "世界".to_string()];
        assert_eq!(accumulate_display(&fin, ""), "你好世界");
    }

    #[test]
    fn test_accumulate_display_with_partial() {
        let fin = vec!["你好".to_string()];
        assert_eq!(accumulate_display(&fin, "世"), "你好世");
    }

    #[test]
    fn test_accumulate_display_partial_only() {
        assert_eq!(accumulate_display(&[], "你好"), "你好");
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
}
