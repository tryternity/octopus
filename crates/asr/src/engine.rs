use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::config;
use crate::qwen3_asr::Qwen3AsrEngine;
use crate::whisper::WhisperEngine;
use crate::sensevoice::SenseVoiceEngine;
use crate::paraformer::ParaformerEngine;
use crate::zipformer::ZipformerEngine;

/// Trait representing a reusable offline ASR model engine
pub trait OfflineAsrEngine: Send + Sync {
    /// Transcribe the 16kHz mono f32 samples using this engine.
    fn transcribe(&self, samples: &[f32], language: &str) -> Result<String>;
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
    pub fn switch_model(&self, model_name: &str) -> Result<()> {
        // Quick check under read lock
        {
            let active_name = self.active_engine_name.read().unwrap();
            if *active_name == model_name {
                return Ok(());
            }
        }

        // Check if already in cache
        let cached = {
            let cache = self.cached_engines.read().unwrap();
            cache.get(model_name).cloned()
        };

        let engine = if let Some(eng) = cached {
            eng
        } else {
            // Not cached, load configuration and instantiate
            let cfg = config::load_config()?;
            let category = config::resolve_engine_category(model_name)
                .context(format!("Unknown engine model: {}", model_name))?;

            let new_eng: Arc<dyn OfflineAsrEngine> = match category {
                config::EngineCategory::Whisper => {
                    let whisper_cfg = cfg.asr.whisper.as_ref().context("No whisper models in config")?;
                    let entry = whisper_cfg.get(model_name).context(format!("Model entry {} not found in whisper config", model_name))?;
                    Arc::new(WhisperEngine::new(entry)?)
                }
                config::EngineCategory::SenseVoice => {
                    let sv_cfg = cfg.asr.sensevoice.as_ref().context("No sensevoice models in config")?;
                    let entry = sv_cfg.get(model_name).context(format!("Model entry {} not found in sensevoice config", model_name))?;
                    Arc::new(SenseVoiceEngine::new(entry)?)
                }
                config::EngineCategory::Paraformer => {
                    let para_cfg = cfg.asr.paraformer.as_ref().context("No paraformer models in config")?;
                    let entry = para_cfg.get(model_name).context(format!("Model entry {} not found in paraformer config", model_name))?;
                    Arc::new(ParaformerEngine::new(entry)?)
                }
                config::EngineCategory::Qwen3Asr => {
                    let qwen_cfg = cfg.asr.qwen3_asr.as_ref().context("No qwen3_asr models in config")?;
                    let entry = qwen_cfg.get(model_name).context(format!("Model entry {} not found in qwen3_asr config", model_name))?;
                    Arc::new(Qwen3AsrEngine::new(entry)?)
                }
                config::EngineCategory::Zipformer => {
                    let zip_cfg = cfg.asr.zipformer.as_ref().context("No zipformer models in config")?;
                    let entry = zip_cfg.get(model_name).context(format!("Model entry {} not found in zipformer config", model_name))?;
                    Arc::new(ZipformerEngine::new(entry)?)
                }
            };

            // Write to cache
            let mut cache = self.cached_engines.write().unwrap();
            cache.insert(model_name.to_string(), new_eng.clone());
            new_eng
        };

        // Switch active references
        let mut active = self.active_engine.write().unwrap();
        *active = Some(engine);
        let mut active_name = self.active_engine_name.write().unwrap();
        *active_name = model_name.to_string();

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
    // 30 seconds threshold (480,000 samples @ 16kHz)
    if samples.len() <= 480_000 {
        return engine.transcribe(samples, language);
    }

    // Try to load Silero VAD. If VAD cannot be loaded (e.g., model file missing),
    // fallback to transcribing the entire audio in one shot.
    let vad_path = match crate::config::find_silero_vad() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Warning: Silero VAD not found, falling back to full audio transcription: {}", e);
            return engine.transcribe(samples, language);
        }
    };

    let mut vad = match crate::vad::SileroVad::new(&vad_path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Warning: Failed to initialize Silero VAD, falling back to full audio transcription: {}", e);
            return engine.transcribe(samples, language);
        }
    };

    let total_secs = samples.len() as f64 / 16000.0;
    eprintln!("[ASR] Long audio detected ({:.2}s). Segmenting audio using VAD...", total_secs);

    let segments = crate::audio::segment_audio_vad(
        samples,
        &mut vad,
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

    Ok(final_text)
}
