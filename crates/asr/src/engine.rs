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
}

/// Helper function to perform VAD-based segmentation and batch transcription for long audio.
pub fn transcribe_with_vad(
    engine: &dyn OfflineAsrEngine,
    samples: &[f32],
    language: &str,
) -> Result<String> {
    let raw_text = if samples.len() <= 480_000 {
        engine.transcribe(samples, language)?
    } else {
        // Try to load Silero VAD. If VAD cannot be loaded (e.g., model file missing),
        // fallback to transcribing the entire audio in one shot.
        let vad_path = match crate::config::find_silero_vad() {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("Warning: Silero VAD not found, falling back to full audio transcription: {}", e);
                None
            }
        };

        let vad = vad_path.and_then(|p| match crate::vad::SileroVad::new(&p) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("Warning: Failed to initialize Silero VAD, falling back to full audio transcription: {}", e);
                None
            }
        });

        if let Some(mut v) = vad {
            let total_secs = samples.len() as f64 / 16000.0;
            eprintln!("[ASR] Long audio detected ({:.2}s). Segmenting audio using VAD...", total_secs);

            let segments = crate::audio::segment_audio_vad(
                samples,
                &mut v,
                480,    // frame_size
                0.4,    // threshold
                500,    // min_silence_ms
                25000,  // max_segment_ms
            );

            eprintln!("[ASR] Audio segmented into {} speech chunks.", segments.len());

            let mut final_text = String::new();
            for (idx, seg) in segments.iter().enumerate() {
                if !seg.is_empty() {
                    let seg_secs = seg.len() as f64 / 16000.0;
                    eprintln!("[ASR] Transcribing segment {}/{} ({:.2}s)...", idx + 1, segments.len(), seg_secs);
                    let text = engine.transcribe(seg, language)?;
                    let text_cleaned = text.replace("<|nospeech|>", "");
                    let text_trimmed = text_cleaned.trim();
                    if !text_trimmed.is_empty() {
                        if !final_text.is_empty() {
                            let last_char = final_text.chars().last();
                            let next_char = text_trimmed.chars().next();
                            let needs_space = match (last_char, next_char) {
                                (Some(lc), Some(nc)) => {
                                    let is_cjk = |c: char| {
                                        let u = c as u32;
                                        (0x4E00..=0x9FFF).contains(&u) || // CJK Unified Ideographs
                                        (0x3040..=0x309F).contains(&u) || // Hiragana
                                        (0x30A0..=0x30FF).contains(&u) || // Katakana
                                        (0xAC00..=0xD7AF).contains(&u)    // Hangul
                                    };
                                    !is_cjk(lc) || !is_cjk(nc)
                                }
                                _ => true,
                            };
                            if needs_space {
                                final_text.push(' ');
                            }
                        }
                        final_text.push_str(text_trimmed);
                    }
                }
            }
            final_text
        } else {
            engine.transcribe(samples, language)?
        }
    };

    // Apply correction if config.asr_correct is true, engine 不自带纠错，且非 en 识别。
    // en 跳过：corrector 是中文拼音/bigram 纠错器，对英文无意义且可能扰动。language 来自
    // 调用方（desktop=config.language 配置项、server=请求、CLI=--language）。
    let app_cfg = crate::config::load_app_config_cached();
    let is_english = language.eq_ignore_ascii_case("en");
    let text = if app_cfg.asr_correct && !engine.skip_corrector() && !is_english {
        crate::corrector::get_corrector().correct(&raw_text)
    } else {
        raw_text
    };
    // 简繁归一化（config.output_simplified）：ASR 输出最后一步，统一字形
    Ok(crate::hans::normalize_variant(&text))
}

