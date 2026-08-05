//! WS session 主循环骨架——收敛 4 provider × 5 session 函数的共同逻辑。
//!
//! 设计详见 `docs/superpowers/specs/2026-08-04-cloud-ws-session-trait-unification.md`。
//!
//! 各 provider（baidu/tencent/bytedance/aliyun-FunASR/aliyun-Qwen）的 session 函数原本
//! 各自实现「建连 → 初始帧 → pre-roll → 双向 select! 循环 → 终态」骨架（~30 行逐字重复 × 5
//! + 建连/pre-roll/Close 终态结构相似 ~40 行 × 5）。本模块抽出 [`WsSessionHandler`] trait +
//! [`run_ws_session_loop`] 骨架，provider 只填协议特定 hook。
//!
//! **不变量**：零行为变更。所有协议帧构造、JSON 解析、状态机逻辑、Close 终态判定一字不改地
//! 搬进各 provider 的 handler 实现。错误消息文本用 `H::LABEL` 插值保持与原逐字一致。

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{handshake::client::Request, Message},
};

use crate::cloud_types::{PcmFrame, StreamEvent};

/// WS session 协议 hook——各 provider 实现，由 [`run_ws_session_loop`] 驱动。
///
/// 生命周期：build_connect_request → build_init_message → build_pcm_message（pre-roll + N 帧实时）→
///           build_finish_message（PcmFrame::Finish 时）→ handle_message（× N，直到终态）。
///
/// handler 持有 provider 特定状态（如 baidu 的 `fin_texts`、aliyun-FunASR 的 `committed`）。
/// 状态在 `handle_message(&mut self, ...)` 调用间累积。
pub(crate) trait WsSessionHandler {
    /// provider 标签（用于错误消息前缀，如 "baidu"/"tencent"/"bytedance"/"aliyun"/"qwen-asr"）。
    /// 与原 session 函数的错误消息前缀逐字一致（不变量）。
    const LABEL: &'static str;

    // ── 建连阶段 ──

    /// 构造 `connect_async` 的请求（含 provider 特定鉴权 headers + query string）。
    ///
    /// 调用方传入 endpoint URL（已含 provider 的 query，如 baidu 的 `?sn=`、tencent 的签名 URL），
    /// handler 返回带 headers 的 request（如 aliyun/bytedance 的 `Authorization`/`X-Api-*`）。
    fn build_connect_request(&self, endpoint: &str) -> Result<Request>;

    // ── 发送阶段（返回 Message 由 loop 统一 ws.send）──

    /// 初始帧（建连后立即发）：baidu START / aliyun-FunASR run-task / qwen session.update /
    /// bytedance FULL_CLIENT_REQUEST。
    ///
    /// tencent 鉴权在 URL，无独立 init 帧——返回 `None`，loop 跳过此步。
    fn build_init_message(&self) -> Result<Option<Message>>;

    /// pre-roll / 实时 PCM 帧：返回要 send 的 Message。
    /// - baidu/tencent/aliyun-FunASR：`Message::Binary(pcm.to_vec())`
    /// - bytedance：`Message::Binary(build_client_frame(MSG_AUDIO_ONLY_REQUEST, pcm))`
    /// - aliyun-Qwen：`Message::Text(json!{input_audio_buffer.append, audio: base64(pcm)})`
    ///
    /// 入参是已转 s16le 的字节（loop 统一调 `samples_to_pcm_s16le`，避免每 provider 重复）。
    fn build_pcm_message(&self, pcm_s16le: &[u8]) -> Result<Message>;

    /// finish 帧（收到 `PcmFrame::Finish` 时发）：baidu FINISH / tencent end /
    /// aliyun-FunASR finish-task / bytedance 末帧 / qwen session.finish。
    fn build_finish_message(&self) -> Result<Message>;

    // ── 接收阶段 ──

    /// 处理一条 WS 消息（`Text`/`Binary`/`Close` 均会传入，handler 内部 match）。
    ///
    /// 返回值指示 loop 后续动作：
    /// - [`HandleOutcome::Continue`]：继续循环（handler 已自行 send `StreamEvent::Text`，或忽略）
    /// - [`HandleOutcome::TerminalFinished`]：识别完成（handler 已发最终 Text），loop 补发 `Finished` 后 return
    /// - [`HandleOutcome::TerminalFailed`]：失败，loop 发 `Failed(reason)` 后 return
    ///
    /// **Close 帧终态判定由 handler 负责**（各 provider 稳态判据不同：baidu `!fin_texts.is_empty()`、
    /// bytedance `last_text.is_some()`、aliyun-FunASR `!committed.is_empty()`、aliyun-Qwen
    /// `!accumulated_text.is_empty()`、tencent `!stable_segments.is_empty()`）。handler 收到
    /// `Message::Close` 时自行判断并返回 `Terminal*`。
    fn handle_message(
        &mut self,
        msg: Message,
        result_tx: &mpsc::UnboundedSender<StreamEvent>,
    ) -> HandleOutcome;
}

/// `handle_message` 的返回值——指示 loop 后续动作。
pub(crate) enum HandleOutcome {
    /// 继续循环（handler 已处理或忽略该消息）
    Continue,
    /// 终态：识别完成（handler 已发最终 `StreamEvent::Text`，loop 只补发 `Finished`）
    TerminalFinished,
    /// 终态：失败（loop 发 `StreamEvent::Failed(reason)`）
    TerminalFailed(String),
}

/// WS session 主循环骨架——驱动建连 → init → pre-roll → 双向循环 → 终态。
///
/// 收敛 5 份 session 函数的共同逻辑：
/// - connect timeout（`WS_CONNECT_TIMEOUT_SECS`）
/// - 发 init + pre-roll（非空才发）
/// - 双向 `select!{ pcm_rx.recv() => dispatch, ws.next() with timeout => 4 路 match }`
/// - 错误处理（timeout / `None` / `Some(Err)`）统一发 `Failed` 后 return
/// - `handle_message` 返回 `Terminal*` 时发终态事件后 return
///
/// handler 负责：建连 request、协议帧构造、消息解析、Close 终态判定。
///
/// **错误消息不变量**：所有错误前缀用 `H::LABEL` 插值（如 `"{label} WS connect timeout"`），
/// 与原 5 份 session 的硬编码前缀逐字一致。
pub(crate) async fn run_ws_session_loop<H: WsSessionHandler>(
    mut pcm_rx: mpsc::UnboundedReceiver<PcmFrame>,
    result_tx: mpsc::UnboundedSender<StreamEvent>,
    endpoint: &str,
    pre_roll_samples: &[f32],
    mut handler: H,
) -> Result<()> {
    let label = H::LABEL;

    // 1. 建连（handler 构造 request，loop 负责 timeout + connect_async）
    let request = handler
        .build_connect_request(endpoint)
        .with_context(|| format!("{label} WS 请求构造失败"))?;
    let (mut ws, _resp) = tokio::time::timeout(
        std::time::Duration::from_secs(octopus_infra::net::WS_CONNECT_TIMEOUT_SECS),
        connect_async(request),
    )
    .await
    .map_err(|_| anyhow::anyhow!("{label} WS connect timeout"))?
    .with_context(|| format!("{label} WS 连接失败: {endpoint}"))?;

    // 2. 发 init 帧（如 provider 无独立 init 帧——如 tencent 鉴权在 URL——返回 None 跳过）
    if let Some(init_msg) = handler.build_init_message()? {
        ws.send(init_msg)
            .await
            .with_context(|| format!("{label} WS 发送初始帧失败"))?;
    }

    // 3. 推 pre-roll PCM（非空才发；loop 统一转 s16le，handler 只负责包装成对应 Message）
    if !pre_roll_samples.is_empty() {
        let pcm = crate::cloud_types::samples_to_pcm_s16le(pre_roll_samples);
        let pre_roll_msg = handler.build_pcm_message(&pcm)?;
        ws.send(pre_roll_msg)
            .await
            .with_context(|| format!("{label} WS 发送 pre-roll PCM 失败"))?;
    }

    // 4. 双向循环
    loop {
        tokio::select! {
            // 收 PCM 指令
            frame = pcm_rx.recv() => {
                match frame {
                    Some(PcmFrame::Samples(pcm)) => {
                        let msg = handler.build_pcm_message(&pcm)?;
                        ws.send(msg)
                            .await
                            .with_context(|| format!("{label} WS 发送音频帧失败"))?;
                    }
                    Some(PcmFrame::Finish) => {
                        let msg = handler.build_finish_message()?;
                        ws.send(msg)
                            .await
                            .with_context(|| format!("{label} WS 发送结束帧失败"))?;
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
                        let _ = result_tx.send(StreamEvent::Failed(
                            format!("{label} WS read timeout")
                        ));
                        return Ok(());
                    }
                    Ok(None) => break,
                    Ok(Some(Err(e))) => {
                        let _ = result_tx.send(StreamEvent::Failed(
                            format!("{label} WS 读错误: {e}")
                        ));
                        return Ok(());
                    }
                    Ok(Some(Ok(m))) => m,
                };
                match handler.handle_message(msg, &result_tx) {
                    HandleOutcome::Continue => {}
                    HandleOutcome::TerminalFinished => {
                        let _ = result_tx.send(StreamEvent::Finished);
                        return Ok(());
                    }
                    HandleOutcome::TerminalFailed(reason) => {
                        let _ = result_tx.send(StreamEvent::Failed(reason));
                        return Ok(());
                    }
                }
            }
        }
    }

    Ok(())
}
