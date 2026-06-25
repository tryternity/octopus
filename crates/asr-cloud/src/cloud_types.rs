//! 云端 ASR 流式会话共用类型与句柄。
//!
//! 4 个 provider（Aliyun / ByteDance / Tencent / Baidu）共用同一组类型：
//! - [`PcmFrame`]：coordinator → 后台 WS task 的音频帧指令
//! - [`StreamEvent`]：后台 WS task → coordinator 的识别结果事件
//! - [`CloudStreamHandle`]：session 句柄，4 个 provider 的 `open()` 均返回此类型
//!
//! 消除原 4 个 provider 各自的 `XxxStreamSession` struct + 4 方法 × 4 = 16 个重复实现。

use anyhow::{anyhow, bail, Result};
use tokio::sync::mpsc;

/// PCM 帧指令：coordinator → 后台 WS task
pub(crate) enum PcmFrame {
    /// 推 PCM 样本（s16le bytes）
    Samples(Vec<u8>),
    /// 发 finish 信号 + 关闭发送端
    Finish,
}

/// 后台 reader 发给 coordinator 的事件。
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// partial / final 识别文本（累积句文本，每次覆盖取最新）
    Text(String),
    /// 服务端识别完成（最终结果已到位，连接可关闭）
    Finished,
    /// 错误信息
    Failed(String),
}

/// `close_async` 保底超时：WS task 若因网络挂起不回 Finished/Failed，recv 会一直 pending
/// 直到 TCP 超时（默认可达分钟级）。8s 与 engine_aliyun 段级超时一致。
const CLOUD_CLOSE_TIMEOUT_SECS: u64 = 8;

/// 云端流式会话句柄（4 provider 共用）。
///
/// 持有 PCM sender（供 coordinator 推音频）和 result receiver（取识别文本）。
/// 后台一条 tokio task 管理 WS 连接的双向收发，由各 provider 的 `run_xxx_session` 实现。
pub struct CloudStreamHandle {
    pcm_tx: mpsc::UnboundedSender<PcmFrame>,
    result_rx: mpsc::UnboundedReceiver<StreamEvent>,
}

impl CloudStreamHandle {
    /// 创建句柄，返回 `(handle, pcm_rx, result_tx)`。
    ///
    /// `pcm_rx` 和 `result_tx` 交给后台 WS task（`run_xxx_session`）。
    ///
    /// `pub(crate)`：仅本 crate 的 4 个 provider `open()` 与测试调用；外部经
    /// `open_cloud_session` 拿到 `CloudStreamHandle`，不自行构造（避免暴露 `pub(crate) PcmFrame`）。
    pub(crate) fn new() -> (
        Self,
        mpsc::UnboundedReceiver<PcmFrame>,
        mpsc::UnboundedSender<StreamEvent>,
    ) {
        let (pcm_tx, pcm_rx) = mpsc::unbounded_channel::<PcmFrame>();
        let (result_tx, result_rx) = mpsc::unbounded_channel::<StreamEvent>();
        (Self { pcm_tx, result_rx }, pcm_rx, result_tx)
    }

    /// 仅供测试：构造 handle + result 发送端（预载事件用）。不暴露 pcm_rx / `pub(crate) PcmFrame`。
    ///
    /// 返回 `(handle, result_tx)`：测试向 `result_tx` 投递 `StreamEvent` 后，`handle.try_recv_text`
    /// 可取到。供 desktop `cloud_pipeline::handle_with_events` 等 drain 测试跨 crate 构造预载 handle。
    #[doc(hidden)]
    pub fn new_for_test() -> (Self, mpsc::UnboundedSender<StreamEvent>) {
        let (handle, _pcm_rx, result_tx) = Self::new();
        (handle, result_tx)
    }

    /// 推 PCM 样本（f32[-1,1] → s16le），非阻塞。
    pub fn push_pcm(&self, samples: &[f32]) -> Result<()> {
        let pcm = samples_to_pcm_s16le(samples);
        self.pcm_tx
            .send(PcmFrame::Samples(pcm))
            .map_err(|_| anyhow!("cloud PCM channel closed"))
    }

    /// 非阻塞发送 Finish 信号，不等待结果。
    pub fn finish(&self) -> Result<()> {
        self.pcm_tx
            .send(PcmFrame::Finish)
            .map_err(|_| anyhow!("cloud PCM channel closed"))
    }

    /// 非阻塞取 partial / final 文本事件。
    pub fn try_recv_text(&mut self) -> Option<StreamEvent> {
        self.result_rx.try_recv().ok()
    }

    /// 非阻塞收尾的 async 内核：发 Finish + 收最终结果（超时上限 `CLOUD_CLOSE_TIMEOUT_SECS`）。
    ///
    /// coordinator 停止路径 spawn 本 future，结果以 `Command::CloudStreamingDone`
    /// 回传，期间进 `Stage::CloudClosing`——避免同步 `block_on` 卡 coordinator 主线程。
    pub async fn close_async(self) -> Result<String> {
        let _ = self.pcm_tx.send(PcmFrame::Finish);
        let mut rx = self.result_rx;
        let mut text = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(CLOUD_CLOSE_TIMEOUT_SECS),
            async {
                while let Some(event) = rx.recv().await {
                    match event {
                        StreamEvent::Text(t) => text = t,
                        StreamEvent::Finished => break,
                        StreamEvent::Failed(msg) => bail!("cloud task-failed: {}", msg),
                    }
                }
                Ok::<(), anyhow::Error>(())
            },
        )
        .await
        .map_err(|_| anyhow!("cloud close 超时（{}s）", CLOUD_CLOSE_TIMEOUT_SECS))??;
        Ok(text)
    }
}

/// f32[-1,1] 样本 → s16le PCM 字节。
///
/// 钳幅到 [-1, 1] 后乘 32767 四舍五入为 i16，按小端字节序展开。
pub fn samples_to_pcm_s16le(samples: &[f32]) -> Vec<u8> {
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
    fn test_samples_to_pcm_s16le_empty() {
        assert!(samples_to_pcm_s16le(&[]).is_empty());
    }

    #[test]
    fn test_samples_to_pcm_s16le_basic() {
        let samples = vec![0.0, 1.0, -1.0];
        let pcm = samples_to_pcm_s16le(&samples);
        assert_eq!(pcm.len(), 6); // 3 samples × 2 bytes
        // 0.0 → 0
        assert_eq!(i16::from_le_bytes([pcm[0], pcm[1]]), 0);
        // 1.0 → 32767
        assert_eq!(i16::from_le_bytes([pcm[2], pcm[3]]), 32767);
        // -1.0 → -32767
        assert_eq!(i16::from_le_bytes([pcm[4], pcm[5]]), -32767);
    }

    #[test]
    fn test_samples_to_pcm_s16le_clamp() {
        // 超出 [-1,1] 的值应被钳幅
        let pcm = samples_to_pcm_s16le(&[2.0]);
        assert_eq!(i16::from_le_bytes([pcm[0], pcm[1]]), 32767);
    }

    #[test]
    fn new_for_test_returns_handle_and_event_sender() {
        // new_for_test 构造的 (handle, sender)：sender 预载事件后 handle.try_recv_text 能取到。
        // 供跨 crate（desktop cloud_pipeline 测试）构造预载事件的 handle。
        let (mut handle, tx) = CloudStreamHandle::new_for_test();
        let _ = tx.send(StreamEvent::Text("hello".to_string()));
        assert!(
            matches!(handle.try_recv_text(), Some(StreamEvent::Text(t)) if t == "hello"),
            "new_for_test 预载的事件应能被 try_recv_text 取到"
        );
    }
}
