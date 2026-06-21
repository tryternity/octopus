//! 云端 ASR 流式会话统一封装：Aliyun（DashScope）、ByteDance（豆包）、Tencent（腾讯云）与 Baidu（百度）共用同一接口。
//!
//! coordinator 通过 `CloudSession` enum 分派，上层 VAD-gated per-utterance 逻辑零改动。
//! 四个 provider 的 session 句柄方法签名完全一致：
//! `push_pcm` / `finish` / `try_recv_text` / `close_async`。

use anyhow::Result;

use crate::aliyun_stream::StreamEvent;

/// 云端流式会话（Aliyun / ByteDance / Tencent / Baidu）。
///
/// 由 coordinator 在语音 onset 时根据 `EngineCategory` 构造对应变体，
/// 后续所有交互（push/finish/recv/close）经本 enum 的方法分派，调用方无需关心具体协议。
pub(crate) enum CloudSession {
    Aliyun(crate::aliyun_stream::AliyunStreamSession),
    ByteDance(crate::bytedance_stream::ByteDanceStreamSession),
    Tencent(crate::tencent_stream::TencentStreamSession),
    Baidu(crate::baidu_stream::BaiduStreamSession),
}

impl CloudSession {
    /// 推 PCM 样本（f32[-1,1] → s16le），非阻塞。
    pub fn push_pcm(&self, samples: &[f32]) -> Result<()> {
        match self {
            CloudSession::Aliyun(s) => s.push_pcm(samples),
            CloudSession::ByteDance(s) => s.push_pcm(samples),
            CloudSession::Tencent(s) => s.push_pcm(samples),
            CloudSession::Baidu(s) => s.push_pcm(samples),
        }
    }

    /// 非阻塞发送 Finish 信号，不等待结果。
    pub fn finish(&self) -> Result<()> {
        match self {
            CloudSession::Aliyun(s) => s.finish(),
            CloudSession::ByteDance(s) => s.finish(),
            CloudSession::Tencent(s) => s.finish(),
            CloudSession::Baidu(s) => s.finish(),
        }
    }

    /// 非阻塞取 partial / final 文本事件。
    pub fn try_recv_text(&mut self) -> Option<StreamEvent> {
        match self {
            CloudSession::Aliyun(s) => s.try_recv_text(),
            CloudSession::ByteDance(s) => s.try_recv_text(),
            CloudSession::Tencent(s) => s.try_recv_text(),
            CloudSession::Baidu(s) => s.try_recv_text(),
        }
    }

    /// 非阻塞收尾的 async 内核：发 Finish + 收最终结果。
    pub async fn close_async(self) -> Result<String> {
        match self {
            CloudSession::Aliyun(s) => s.close_async().await,
            CloudSession::ByteDance(s) => s.close_async().await,
            CloudSession::Tencent(s) => s.close_async().await,
            CloudSession::Baidu(s) => s.close_async().await,
        }
    }
}
