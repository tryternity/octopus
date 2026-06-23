use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::config;
use crate::qwen3_asr::Qwen3AsrEngine;
use crate::whisper::WhisperEngine;
use crate::sensevoice::SenseVoiceEngine;
use crate::paraformer::ParaformerEngine;
use crate::zipformer::{ZipformerCtcEngine, ZipformerTransducerEngine};
use crate::moonshine::MoonshineEngine;

/// Trait representing a reusable offline ASR model engine
pub trait OfflineAsrEngine: Send + Sync {
    /// Transcribe the 16kHz mono f32 samples using this engine.
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String>;

    /// 是否跳过通用中文 corrector（`transcribe_with_vad` 末尾的纠错步骤）。
    ///
    /// 仅用于「非语言原因」需跳过的引擎（如 qwen3 自带纠错/高质量，不需外挂纠错）。
    /// en-only 场景由 `transcribe_with_vad` 基于 language 自动跳过（language=en），不在此覆盖。
    fn skip_corrector(&self) -> bool {
        false
    }
}

/// Orchestrator to load, cache, and swap ASR engines dynamically.
pub struct AsrEngineManager {
    cached_engines: RwLock<HashMap<String, Arc<dyn OfflineAsrEngine>>>,
    active_engine: RwLock<Option<Arc<dyn OfflineAsrEngine>>>,
    active_engine_name: RwLock<String>,
}

impl AsrEngineManager {
    pub fn new() -> Self {
        Self {
            cached_engines: RwLock::new(HashMap::new()),
            active_engine: RwLock::new(None),
            active_engine_name: RwLock::new(String::new()),
        }
    }

    /// Load or switch the active ASR engine to the requested model.
    ///
    /// `model_name` 支持 spec 格式（`provider:category:model_name` / `model_name`），
    /// 内部解析为裸 model_name 后作为缓存键。
    pub fn switch_model(&self, model_name: &str) -> Result<()> {
        let parsed = config::parse_model_spec(model_name);
        let bare_name = parsed.model_name();

        // Quick check under read lock
        {
            let active_name = self.active_engine_name.read().unwrap();
            if *active_name == bare_name {
                return Ok(());
            }
        }

        // Check if already in cache
        let cached = {
            let cache = self.cached_engines.read().unwrap();
            cache.get(bare_name).cloned()
        };

        let engine = if let Some(eng) = cached {
            eng
        } else {
            // Not cached, load configuration and instantiate
            let cfg = config::load_config()?;
            let (category, _bare, entry) = config::resolve_engine_in_config(&cfg, model_name)
                .with_context(|| format!("Unknown engine model: {}", model_name))?;

            let new_eng: Arc<dyn OfflineAsrEngine> = match category {
                config::EngineCategory::Whisper => Arc::new(WhisperEngine::new(entry)?),
                config::EngineCategory::SenseVoice => Arc::new(SenseVoiceEngine::new(entry)?),
                config::EngineCategory::Paraformer => Arc::new(ParaformerEngine::new(entry)?),
                config::EngineCategory::Qwen3Asr => Arc::new(Qwen3AsrEngine::new(entry)?),
                config::EngineCategory::Zipformer => {
                    // 检测有无 decoder.onnx：有则为 Transducer（RNN-T），无则为 CTC
                    let hf_path = config::resolve_model_dir(&entry.source)?;
                    let has_decoder = hf_path.join("decoder.onnx").exists();
                    if has_decoder {
                        Arc::new(ZipformerTransducerEngine::new(entry)?)
                    } else {
                        Arc::new(ZipformerCtcEngine::new(entry)?)
                    }
                }
                // Aliyun 云端引擎由 Task 2 实现（AliyunEngine）；Task 1 阶段本地实例化无实现。
                config::EngineCategory::Aliyun => anyhow::bail!(
                    "阿里云云端 ASR 引擎尚未接入（spec='{}'，见 Task 2 AliyunEngine）",
                    model_name
                ),
                config::EngineCategory::Moonshine => Arc::new(MoonshineEngine::new(entry)?),
                config::EngineCategory::ByteDance => anyhow::bail!(
                    "字节跳动云端 ASR 引擎仅支持流式模式（需 WS 连接），不支持本地实例化（spec='{}'）",
                    model_name
                ),
                config::EngineCategory::Tencent => anyhow::bail!(
                    "腾讯云云端 ASR 引擎仅支持流式模式（需 WS 连接），不支持本地实例化（spec='{}'）",
                    model_name
                ),
                config::EngineCategory::Baidu => anyhow::bail!(
                    "百度云云端 ASR 引擎仅支持流式模式（需 WS 连接），不支持本地实例化（spec='{}'）",
                    model_name
                ),
            };

            // Write to cache
            let current_active = {
                self.active_engine_name.read().unwrap().clone()
            };
            let mut cache = self.cached_engines.write().unwrap();
            if cache.len() >= 2 {
                let key_to_remove = cache.keys()
                    .find(|k| *k != &current_active)
                    .cloned()
                    .or_else(|| cache.keys().next().cloned());
                if let Some(k) = key_to_remove {
                    log::info!("Evicting engine '{}' from cache to free up memory", k);
                    cache.remove(&k);
                }
            }
            cache.insert(bare_name.to_string(), new_eng.clone());
            new_eng
        };

        // Switch active references
        let mut active = self.active_engine.write().unwrap();
        *active = Some(engine);
        let mut active_name = self.active_engine_name.write().unwrap();
        *active_name = bare_name.to_string();

        Ok(())
    }

    /// Transcribe using the active engine.
    pub fn transcribe(&self, samples: &[f32], language: &str) -> Result<String> {
        let engine = {
            let active = self.active_engine.read().unwrap();
            active.clone()
        };
        if let Some(eng) = engine {
            transcribe_with_vad(eng.as_ref(), samples, language)
        } else {
            anyhow::bail!("No active ASR engine loaded in AsrEngineManager")
        }
    }

    /// 批处理转写（pipeline 入口）：用 active engine + cfg 调 `pipeline::transcribe_batch`。
    /// 供 cli/server 等多端复用，取代端侧各自读全局 config 的旧路径。
    pub fn transcribe_batch(
        &self,
        samples: &[f32],
        cfg: &crate::pipeline::PipelineConfig,
    ) -> Result<String> {
        let engine = {
            let active = self.active_engine.read().unwrap();
            active.clone()
        };
        let eng = engine
            .ok_or_else(|| anyhow::anyhow!("No active ASR engine loaded in AsrEngineManager"))?;
        crate::pipeline::transcribe_batch(eng.as_ref(), samples, cfg)
    }
}

/// 保留入口（desktop 经 `AsrEngineManager::transcribe` 使用）：从全局 app_config 构造 cfg
/// 后委托 `pipeline::transcribe_batch`。行为与重构前完全一致（asr_correct / output_simplified
/// 仍来自 app_config）。
pub fn transcribe_with_vad(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    language: &str,
) -> Result<String> {
    let cfg = crate::pipeline::PipelineConfig::from_app_config(language);
    crate::pipeline::transcribe_batch(engine, samples, &cfg)
}

