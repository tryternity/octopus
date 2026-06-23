//! CLI 批处理转写 pipeline：`switch_model` → `AsrEngineManager::transcribe_batch`。
//!
//! 取代旧 `do_transcribe`（直接调各引擎裸 `transcribe` 自由函数、无 VAD/纠错/简繁），
//! 让 cli 与 desktop 共用 `asr::pipeline::transcribe_batch` 的完整编排
//! （VAD 分段 + 纠错 + 简繁归一化）。cfg 从全局 app_config 构造，与 desktop 行为一致。

use anyhow::Result;
use octopus_asr::engine::AsrEngineManager;
use octopus_asr::pipeline::PipelineConfig;

/// 批处理转写：加载引擎 → transcribe_batch（VAD + 纠错 + 简繁）。
///
/// `model` 为 DB models 表的 model_name（支持 `provider:category:model` spec）。
/// 云端引擎（火山/腾讯/百度/阿里）会在 `switch_model` 阶段 bail（仅支持流式）——
/// 与旧 `do_transcribe` 的行为一致。
pub fn run(model: &str, language: &str, samples: &[f32]) -> Result<String> {
    let mgr = AsrEngineManager::new();
    mgr.switch_model(model)?;
    let cfg = PipelineConfig::from_app_config(language);
    mgr.transcribe_batch(samples, &cfg)
}
