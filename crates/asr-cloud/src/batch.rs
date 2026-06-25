//! 云端 ASR 批引擎（impl `octopus_asr::engine::OfflineAsrEngine`）。
//!
//! 语义：`transcribe(samples, language)` = 单段音频（≤30s，由上层 `transcribe_segments`
//! 保证）→ 单个 WSS session → 完整文本。VAD 分段 + CJK 连接由
//! `asr::pipeline::transcribe_segments` 自动完成，本引擎不分段、不拼接。
//!
//! `skip_corrector() = true`：云端结果质量高，跳过本地拼音纠错（对齐桌面端云端行为）；
//! 简繁转换仍由 `transcribe_batch` 处理。

use crate::open_cloud_session;
use anyhow::{bail, Result};
use octopus_asr::engine::OfflineAsrEngine;
use octopus_infra::db::{parse_model_spec, ModelSpec};

/// 分块推送粒度（采样点）：200ms @ 16kHz = 3200。平滑灌入避免单帧过大。
const CLOUD_PUSH_CHUNK_SAMPLES: usize = 3200;

/// 判断 spec 是否云端 ASR（3-part provider 前缀为 aliyun/bytedance/tencent/baidu）。
///
/// 用 `parse_model_spec` 取 provider 字段，**不查 DB**（纯字符串解析，可单测）。
/// 2-part/裸名 → `NameOnly` → false（走本地分支）。3-part 是标准 spec 格式
///（如 `aliyun:Fun-ASR:fun-asr-realtime`）。cli 分流与本 crate 的 `from_spec` 共用此判定。
pub fn is_cloud_spec(spec: &str) -> bool {
    matches!(
        parse_model_spec(spec),
        ModelSpec::Full { provider, .. } if is_cloud_provider(provider)
    )
}

/// provider 字符串是否云端（大小写不敏感）。
fn is_cloud_provider(provider: &str) -> bool {
    provider.eq_ignore_ascii_case("aliyun")
        || provider.eq_ignore_ascii_case("bytedance")
        || provider.eq_ignore_ascii_case("tencent")
        || provider.eq_ignore_ascii_case("baidu")
}

/// 云端 ASR 批引擎。
pub struct CloudBatchEngine {
    /// 完整 3-part spec（如 `aliyun:Fun-ASR:fun-asr-realtime`），`open_cloud_session` 据此解析配置。
    spec: String,
    /// 自有 tokio runtime（驱动各 provider `open` 的 `tokio::spawn` + `close_async`）。
    rt: tokio::runtime::Runtime,
}

impl CloudBatchEngine {
    /// 从 spec 构造。校验 provider 前缀为云端（不查 DB）+ 建 runtime。
    /// DB 查找（resolve_*_config）推迟到 `transcribe` 内的 `open_cloud_session`。
    pub fn from_spec(spec: &str) -> Result<Self> {
        if !is_cloud_spec(spec) {
            bail!(
                "非云端 ASR spec（'{}'）；CloudBatchEngine 仅支持 3-part 云端 spec \
                 （aliyun/bytedance/tencent/baidu:category:model_name）",
                spec
            );
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self { spec: spec.to_string(), rt })
    }
}

impl OfflineAsrEngine for CloudBatchEngine {
    /// 单段音频（≤30s）→ 单个 WSS session → 完整文本。
    ///
    /// **须在非 tokio runtime context 调用**：内部 `self.rt.block_on` 会嵌套 panic
    ///（"Cannot start a runtime from within a runtime"）。cli 主线程满足此约束；
    /// 勿在 server/tauri 的 async handler 内直接调用。
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        let spec = self.spec.clone();
        let lang = language.to_string();
        self.rt.block_on(async move {
            let handle = open_cloud_session(&spec, &lang, Vec::new())?;
            // 分块推 PCM（批处理一次推完；空 samples 也安全：不进循环，直接 finish）。
            // push_pcm 用 &self（内部 mpsc clone 发送），无需 mut。
            for chunk in samples.chunks(CLOUD_PUSH_CHUNK_SAMPLES) {
                handle.push_pcm(chunk)?;
            }
            // close_async：发 Finish + 收最终结果（超时上限 CLOUD_CLOSE_TIMEOUT_SECS=8s）
            //（消费 handle，本 session 结束）。
            handle.close_async().await
        })
    }

    fn skip_corrector(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_cloud_spec_recognizes_3part_cloud() {
        // 3-part 云端 spec（provider 前缀为云端）→ true（不查 DB）。
        // category 段取 asr/config.rs category_label 的实际值。
        assert!(is_cloud_spec("aliyun:Fun-ASR:fun-asr-realtime"));
        assert!(is_cloud_spec("bytedance:Doubao-ASR:doubao-asr-1.0-streaming"));
        assert!(is_cloud_spec("tencent:Tencent-ASR:16k_zh"));
        assert!(is_cloud_spec("baidu:Baidu-ASR:15372"));
    }

    #[test]
    fn is_cloud_spec_rejects_local_3part_bare_and_2part() {
        // 本地 3-part（provider=local）→ false。
        assert!(!is_cloud_spec("local:zipformer:zipformer-small-ctc"));
        // 裸名 → NameOnly → false。
        assert!(!is_cloud_spec("zipformer-small-ctc"));
        // 2-part → NameOnly 兜底 → false（须 3-part 才判云端）。
        assert!(!is_cloud_spec("aliyun:fun-asr-realtime"));
    }

    #[test]
    fn from_spec_rejects_non_cloud() {
        assert!(CloudBatchEngine::from_spec("local:zipformer:zipformer-small-ctc").is_err());
        assert!(CloudBatchEngine::from_spec("zipformer-small-ctc").is_err());
        assert!(CloudBatchEngine::from_spec("aliyun:fun-asr-realtime").is_err()); // 2-part
    }

    #[test]
    fn from_spec_accepts_cloud_3part() {
        // 云端 3-part → 构造成功（不查 DB、不连网；仅建 runtime）。
        assert!(CloudBatchEngine::from_spec("aliyun:Fun-ASR:fun-asr-realtime").is_ok());
    }

    /// 真实 DashScope 集成测试：`cargo test -p octopus-asr-cloud --lib -- --ignored batch::real_aliyun`。
    /// 需 ~/.octopus/config.yaml 的 asr.aliyun.<model> 配好 secret_key。
    /// 用 `cargo run` 录一段样本或用现成 wav → f32 样本后断言非空文本。
    #[ignore]
    #[test]
    fn real_aliyun_transcribe_nonempty() {
        // 占位：实际验证靠 cli 端到端（Task 8 e2e 清单）。
        // 此测试保留为「有本地 key 时的最小集成入口」，样本来源由用户准备。
        // 无样本时直接返回，避免误失败。
        eprintln!("[ignore] 跳过：需本地 DashScope key + 音频样本，见 Task 8 e2e 清单");
    }
}
