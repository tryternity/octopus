//! ASR pipeline 编排：批处理 helper（流式 helper / StreamingRunner 见后续阶段）。
//!
//! `transcribe_batch` 收编原 `engine::transcribe_with_vad` 的 VAD 分段编排，把纠错
//! （`correct`）与简繁归一化（`simplify`）从「读全局 app_config」参数化为 `PipelineConfig`
//! 字段，使编排可被多端（cli/desktop/server）以明确参数复用，而非隐式依赖全局配置。
//!
//! 设计详见 `docs/superpowers/specs/2026-06-23-asr-pipeline-design.md`。

use crate::config::load_app_config_cached;

/// 批处理 pipeline 配置。
///
/// 阶段1 精简版：`correct` / `simplify` 在 `transcribe_batch` 内替代原 `transcribe_with_vad`
/// 对全局 `app_config` 的读取；`ngram` 为预留字段（解码纠错，尚未实现）。流式相关字段
/// （`backend` / `denoise` / 音频源）随阶段2 流式 helper 加入。
pub struct PipelineConfig {
    pub language: String,
    /// 是否对 ASR 输出做拼音/bigram 纠错（原 `app_config.asr_correct`）。
    pub correct: bool,
    /// true→输出简体，false→输出繁体（原 `app_config.output_simplified`）。
    pub simplify: bool,
    /// ngram 解码纠错开关（预留，尚未实现；`transcribe_batch` 见到 true 仅 warn）。
    pub ngram: bool,
}

impl PipelineConfig {
    /// 从全局 `app_config` 构造（向后兼容 `transcribe_with_vad` / desktop 既有行为）。
    pub fn from_app_config(language: &str) -> Self {
        let app = load_app_config_cached();
        Self {
            language: language.to_string(),
            correct: app.asr_correct,
            simplify: app.output_simplified,
            ngram: false,
        }
    }
}
