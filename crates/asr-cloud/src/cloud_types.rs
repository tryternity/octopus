//! 云端 ASR 流式会话共用类型与句柄。
//!
//! 4 个 provider（Aliyun / ByteDance / Tencent / Baidu）共用同一组类型：
//! - [`PcmFrame`]：coordinator → 后台 WS task 的音频帧指令
//! - [`StreamEvent`]：后台 WS task → coordinator 的识别结果事件
//! - [`CloudStreamHandle`]：session 句柄，4 个 provider 的 `open()` 均返回此类型
//!
//! 消除原 4 个 provider 各自的 `XxxStreamSession` struct + 4 方法 × 4 = 16 个重复实现。

use anyhow::{anyhow, bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Finish 幂等守卫：首个 `finish()` / `close_async()` 发 `Finish` 后置 true，
    /// 后续调用跳过——防 tick 的 `sess.finish()` 与 `close_async` 双发 `Finish`，
    /// 导致服务端收到两次 finish-task/末帧/end/FINISH（4 provider 的 WS task 收到
    /// `Finish` 都只发服务端信号、不退出循环，第二个 `Finish` 会被原样处理）。
    finished: AtomicBool,
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
        (
            Self {
                pcm_tx,
                result_rx,
                finished: AtomicBool::new(false),
            },
            pcm_rx,
            result_tx,
        )
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
    ///
    /// **幂等**：首个调用发 `Finish` 并置 `finished=true`，后续调用（含 `close_async`）直接
    /// 返回 `Ok(())`——防止调用方「先 `finish()` 再 `close_async()`」时双发 `Finish`。
    pub fn finish(&self) -> Result<()> {
        if self.finished.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        self.pcm_tx
            .send(PcmFrame::Finish)
            .map_err(|_| anyhow!("cloud PCM channel closed"))
    }

    /// 非阻塞取 partial / final 文本事件。
    pub fn try_recv_text(&mut self) -> Option<StreamEvent> {
        self.result_rx.try_recv().ok()
    }

    /// 非阻塞收尾的 async 内核：发 Finish（若未发过）+ 收最终结果（超时上限 `CLOUD_CLOSE_TIMEOUT_SECS`）。
    ///
    /// coordinator 停止路径 spawn 本 future，结果以 `Command::CloudStreamingDone`
    /// 回传，期间进 `Stage::CloudClosing`——避免同步 `block_on` 卡 coordinator 主线程。
    ///
    /// **幂等**：若 `finish()` 已发过 `Finish`（`finished=true`）则不重发，只收结果——
    /// 防「tick 的 `sess.finish()` + `close_async`」双发 `Finish` 到服务端。
    pub async fn close_async(self) -> Result<String> {
        // 幂等：finish() 已发过则不重发（防双发）；未发过才补发收尾。
        if !self.finished.swap(true, Ordering::Relaxed) {
            let _ = self.pcm_tx.send(PcmFrame::Finish);
        }
        let mut rx = self.result_rx;
        let mut text = String::new();
        // 健康的 WS task 总会发 Finished（成功）或 Failed（错误）作为终态。若 sender
        // drop 而没发终态（WS task 因服务端主动 Close 静默退出见 #4，或 panic），
        // rx.recv() 返 None → 循环正常结束。旧实现直接返回 Ok(text)，把 partial/空
        // 结果当成功，鉴权过期/超时/限流断连时错误被完全吞没。现改为 bail!——与
        // Failed 分支一致，让调用方能区分「正常完成」与「异常截断」。
        let mut finished = false;
        let inner: Result<()> = tokio::time::timeout(
            std::time::Duration::from_secs(CLOUD_CLOSE_TIMEOUT_SECS),
            async {
                while let Some(event) = rx.recv().await {
                    match event {
                        // 防御性判空（D1）：空 Text 不覆盖已累积的非空文本。
                        // 契约上 provider 不应发空 Text（各 provider 有 !display.is_empty()
                        // 保护），但历史 bug（H1 bytedance / R1 aliyun FunASR）证明逐
                        // provider 堵漏有单点遗漏风险。close_async 作为公共收尾层加防御，
                        // 从根上消除「任一 provider 漏判空 → 空结果当成功」整类 bug。
                        StreamEvent::Text(t) if !t.is_empty() => text = t,
                        StreamEvent::Text(_) => {} // 空 Text 忽略（保留上次非空累积）
                        StreamEvent::Finished => {
                            finished = true;
                            break;
                        }
                        StreamEvent::Failed(msg) => bail!("cloud task-failed: {}", msg),
                    }
                }
                Ok::<(), anyhow::Error>(())
            },
        )
        .await
        .map_err(|_| anyhow!("cloud close 超时（{}s）", CLOUD_CLOSE_TIMEOUT_SECS))?;
        inner?;
        if !finished {
            // sender drop 但无终态事件——异常截断（服务端断连未发 Failed 等）。
            // 不返回 partial text（可能不准）；让调用方按错误处理。影响 4 个 provider。
            bail!(
                "cloud session closed without terminal event ({} bytes partial text)",
                text.len()
            );
        }
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

/// 拼接稳态句（已 join 为单个 String）+ 当前 partial 为显示文本。
///
/// 4 个 provider（baidu/tencent/aliyun-FunASR/aliyun-Qwen）的 partial 显示逻辑同构：
/// - 两者都空 → 空串
/// - 仅 stable 空 → 直接 partial（首句，不加前导 sep）
/// - 仅 partial 空 → 直接 stable
/// - 两者都非空 → stable + sep + partial（句间分隔，防粘连）
///
/// 2026-08-05 抽取（问题 2）：消除各 handler 内联的 if/else 拼接判定。各 provider 的
/// "稳态句收集"结构不同（Vec/BTreeMap/String），但最终都 join 成 `stable: String` 后
/// 调本函数。
pub(crate) fn combine_stable_partial(stable: &str, partial: &str, sep: &str) -> String {
    if partial.is_empty() {
        stable.to_string()
    } else if stable.is_empty() {
        // 首句 partial（无稳态句）——直接 partial，不加前导 sep
        partial.to_string()
    } else {
        // 有稳态句 + partial ——stable 与 partial 间插 sep（句间分隔）
        format!("{}{}{}", stable, sep, partial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combine_stable_partial_both_empty() {
        assert_eq!(combine_stable_partial("", "", "，"), "");
    }

    #[test]
    fn combine_stable_partial_stable_only() {
        assert_eq!(combine_stable_partial("你好，世界", "", "，"), "你好，世界");
    }

    #[test]
    fn combine_stable_partial_first_partial_no_leading_sep() {
        // 首句 partial（stable 空）——不加前导 sep
        assert_eq!(combine_stable_partial("", "你好", "，"), "你好");
        assert_eq!(combine_stable_partial("", "hello", " "), "hello");
    }

    #[test]
    fn combine_stable_partial_stable_plus_partial_inserts_sep() {
        // 有稳态句 + partial ——中间插 sep（防粘连）
        assert_eq!(combine_stable_partial("你好", "世界", "，"), "你好，世界");
        assert_eq!(combine_stable_partial("hello world", "today", " "), "hello world today");
    }

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

    #[test]
    fn finish_is_idempotent() {
        // finish() 幂等：连调两次只发一个 Finish（防 tick sess.finish 与 close_async 双发）。
        let (handle, mut pcm_rx, _result_tx) = CloudStreamHandle::new();
        handle.finish().unwrap();
        handle.finish().unwrap(); // 第二次 swap 已 true → 跳过，不报错
        let mut finish_count = 0;
        while let Ok(frame) = pcm_rx.try_recv() {
            if matches!(frame, PcmFrame::Finish) {
                finish_count += 1;
            }
        }
        assert_eq!(finish_count, 1, "finish() 幂等，只应发一个 Finish");
    }

    #[tokio::test]
    async fn close_async_after_finish_skips_resend() {
        // finish() 先发 Finish 置 finished=true，close_async 不应重发（防双发到服务端）。
        let (handle, mut pcm_rx, result_tx) = CloudStreamHandle::new();
        handle.finish().unwrap(); // 发 Finish #1，置 finished=true
        // 预发结果让 close_async 不超时（CLOUD_CLOSE_TIMEOUT_SECS=8s）
        result_tx.send(StreamEvent::Text("hi".into())).ok();
        result_tx.send(StreamEvent::Finished).ok();
        drop(result_tx);
        let text = handle.close_async().await.unwrap();
        assert_eq!(text, "hi");
        // close_async 应跳过 Finish：pcm_rx 只剩 finish() 发的那一个
        let mut finish_count = 0;
        while let Ok(frame) = pcm_rx.try_recv() {
            if matches!(frame, PcmFrame::Finish) {
                finish_count += 1;
            }
        }
        assert_eq!(finish_count, 1, "close_async 在 finish() 之后不应重发 Finish");
    }

    /// 回归 #3：sender drop 但没发终态（Finished/Failed）时，close_async 必须报错，
    /// 不能把 partial/空 text 当成功返回。模拟服务端主动 Close（#4）导致 WS task
    /// 静默退出、未发终态的场景。
    #[tokio::test]
    async fn close_async_fails_on_channel_close_without_finished() {
        let (handle, _pcm_rx, result_tx) = CloudStreamHandle::new();
        handle.finish().unwrap(); // 置 finished=true，跳过重发
        // 只发 partial Text，不发 Finished 就 drop sender
        result_tx.send(StreamEvent::Text("partial".into())).ok();
        drop(result_tx);
        let res = handle.close_async().await;
        assert!(
            res.is_err(),
            "sender 无终态 drop 时应报错，而非返回 partial text"
        );
        let msg = res.unwrap_err().to_string();
        assert!(
            msg.contains("without terminal event"),
            "错误信息应说明是异常截断，got: {msg}"
        );
    }

    /// 正常路径：sender 发 Text + Finished，close_async 返回 text（保证修复未破坏正常流程）。
    #[tokio::test]
    async fn close_async_returns_text_on_normal_finished() {
        let (handle, _pcm_rx, result_tx) = CloudStreamHandle::new();
        handle.finish().unwrap();
        result_tx.send(StreamEvent::Text("最终结果".into())).ok();
        result_tx.send(StreamEvent::Finished).ok();
        drop(result_tx);
        let text = handle.close_async().await.unwrap();
        assert_eq!(text, "最终结果");
    }

    /// 防御性判空（D1 修复）：close_async 忽略空 Text，保留上次非空累积。
    /// 历史：H1（bytedance）/ R1（aliyun FunASR）证明「逐 provider 堵漏」有单点遗漏
    /// 风险——任一 provider 漏判空就发空 Text，旧契约 `text = t` 无条件覆盖导致有效
    /// 结果丢失。D1 在 close_async 公共收尾层加防御 `if !t.is_empty()`，从根上消除
    /// 整类 bug。此测试固化新契约：空 Text 不覆盖。
    #[tokio::test]
    async fn close_async_ignores_empty_text_keeps_last_non_empty() {
        let (handle, _pcm_rx, result_tx) = CloudStreamHandle::new();
        handle.finish().unwrap();
        // 序列 [Text("你好"), Text(""), Finished] —— 空 Text 不应覆盖 "你好"
        result_tx.send(StreamEvent::Text("你好".into())).ok();
        result_tx.send(StreamEvent::Text("".into())).ok();
        result_tx.send(StreamEvent::Finished).ok();
        drop(result_tx);
        let text = handle.close_async().await.unwrap();
        // 新契约：空 Text 忽略 → 返 "你好"（保留上次非空累积）
        assert_eq!(text, "你好");
    }

    /// Failed 终态正常传播（既有行为，回归测试保证）。
    #[tokio::test]
    async fn close_async_propagates_failed_event() {
        let (handle, _pcm_rx, result_tx) = CloudStreamHandle::new();
        handle.finish().unwrap();
        result_tx.send(StreamEvent::Failed("鉴权失败".into())).ok();
        drop(result_tx);
        let res = handle.close_async().await;
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("鉴权失败"));
    }
}
