//! CLI 批处理转写 pipeline：本地 / 云端分流 → `transcribe_batch`（VAD + 纠错 + 简繁）。
//!
//! 分流在 cli 层（`asr` crate 不依赖 `asr-cloud`，避免循环）：
//! - 云端 spec（aliyun/bytedance/tencent/baidu）→ `CloudBatchEngine::from_spec`。
//! - 本地 onnx → `AsrEngineManager` + `active_engine`。
//!
//! 两端都经 `asr::pipeline::transcribe_batch` 编排（VAD 分段 + 纠错 + 简繁）。

use anyhow::Result;
use octopus_asr::engine::AsrEngineManager;
use octopus_asr::pipeline::{transcribe_batch, PipelineConfig};
use octopus_asr_cloud::{is_cloud_spec, CloudBatchEngine};

/// 批处理转写：分流 → transcribe_batch（VAD 分段 + 纠错 + 简繁）。
///
/// `model` 为 DB models 表的 model_name（支持 `provider:category:model` spec）。
/// 云端 spec → `CloudBatchEngine`（内部 WSS，`skip_corrector=true`）；本地 → onnx 引擎。
pub fn run(model: &str, language: &str, samples: &[f32]) -> Result<String> {
    let cfg = PipelineConfig::from_app_config(language);
    if is_cloud_spec(model) {
        let engine = CloudBatchEngine::from_spec(model)?;
        transcribe_batch(&engine, samples, &cfg)
    } else {
        let mgr = AsrEngineManager::new();
        mgr.switch_model(model)?;
        let engine = mgr.active_engine()?;
        transcribe_batch(&*engine, samples, &cfg)
    }
}
