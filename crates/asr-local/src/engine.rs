use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::config;
use crate::qwen3_asr::Qwen3AsrEngine;
use crate::whisper::WhisperEngine;
use crate::sensevoice_orig::SenseVoiceOrigEngine;
use crate::paraformer::ParaformerEngine;
use crate::zipformer::{ZipformerCtcEngine, ZipformerTransducerEngine};
use crate::firered::FireRedEngine;
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
    /// 引擎缓存上限（每引擎数百 MB，桌面默认 2 控内存；server 多模型并发可放大）。
    max_cache: usize,
}

impl Default for AsrEngineManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AsrEngineManager {
    /// 桌面/cli 默认：缓存上限 2。
    pub fn new() -> Self {
        Self::new_with_capacity(2)
    }

    /// 指定引擎缓存上限。server 等多模型并发场景用更大值（如 8），避免频繁淘汰重载。
    pub fn new_with_capacity(max_cache: usize) -> Self {
        Self {
            cached_engines: RwLock::new(HashMap::new()),
            active_engine: RwLock::new(None),
            active_engine_name: RwLock::new(String::new()),
            max_cache: max_cache.max(1),
        }
    }

    /// Load or switch the active ASR engine to the requested model.
    ///
    /// `model_name` 支持 spec 格式（`provider:category:model_name` / `model_name`），
    /// 内部解析为裸 model_name 后作为缓存键。
    ///
    /// 单路场景（cli/desktop）：active 单例语义合理，用此方法。
    /// 多并发场景（server）改用 [`get_engine`](Self::get_engine)，避免全局 active 竞态。
    pub fn switch_model(&self, model_name: &str) -> Result<()> {
        let bare_name = config::parse_model_spec(model_name).model_name();

        // Quick check under read lock（避免重复切同引擎）
        {
            let active_name = self.active_engine_name.read().unwrap();
            if *active_name == bare_name {
                return Ok(());
            }
        }

        let engine = self.load_engine_into_cache(model_name)?;

        // Switch active references
        let mut active = self.active_engine.write().unwrap();
        *active = Some(engine);
        let mut active_name = self.active_engine_name.write().unwrap();
        *active_name = bare_name.to_string();

        Ok(())
    }

    /// 只读获取引擎 `Arc`（不改全局 active），供 server 等多并发场景替代 `switch_model`。
    ///
    /// 同模型并发受引擎内部 `Mutex<Session>` 串行化（见 `ParaformerEngine`/`SenseVoiceOrigEngine`），
    /// 跨模型天然并行——不再需要 server 级全局 `inference_lock`。
    pub fn get_engine(&self, model_name: &str) -> Result<Arc<dyn OfflineAsrEngine>> {
        self.load_engine_into_cache(model_name)
    }

    /// 解析 spec → 查缓存 → 未命中加载入缓存（按 `max_cache` 淘汰，保护 active），返回 `Arc`。
    /// 不改 `active_engine`/`active_engine_name`。`switch_model` 与 `get_engine` 共用此逻辑。
    fn load_engine_into_cache(&self, model_name: &str) -> Result<Arc<dyn OfflineAsrEngine>> {
        let bare_name = config::parse_model_spec(model_name).model_name();

        // 命中缓存直接返回（不触发淘汰/加载）
        {
            let cache = self.cached_engines.read().unwrap();
            if let Some(eng) = cache.get(bare_name) {
                return Ok(eng.clone());
            }
        }

        // 未命中：加载配置 + 实例化
        let cfg = config::load_config()?;
        let (category, _bare, entry) = config::resolve_engine_in_config(&cfg, model_name)
            .with_context(|| format!("Unknown engine model: {}", model_name))?;

        let new_eng: Arc<dyn OfflineAsrEngine> = match category {
            config::EngineCategory::Whisper => Arc::new(WhisperEngine::new(entry)?),
            config::EngineCategory::SenseVoiceOrig => Arc::new(SenseVoiceOrigEngine::new(entry)?),
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
            config::EngineCategory::FireRed => Arc::new(FireRedEngine::new(entry)?),
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

        // 入缓存 + 按 max_cache 淘汰（保护当前 active，避免淘汰正用的引擎）
        let current_active = self.active_engine_name.read().unwrap().clone();
        let mut cache = self.cached_engines.write().unwrap();
        if cache.len() >= self.max_cache {
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
        Ok(new_eng)
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

    /// 取出当前 active engine（供 cli 分流后统一调 `pipeline::transcribe_batch`）。
    ///
    /// 与本地/云端分流配合：cli 本地分支构造 `AsrEngineManager` + `switch_model` 后取
    /// `Arc<dyn OfflineAsrEngine>`，与云端分支的 `CloudBatchEngine` 同为 `dyn OfflineAsrEngine`，
    /// 喂同一 `transcribe_batch`。
    pub fn active_engine(&self) -> Result<Arc<dyn OfflineAsrEngine>> {
        self.active_engine
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No active ASR engine loaded in AsrEngineManager"))
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

