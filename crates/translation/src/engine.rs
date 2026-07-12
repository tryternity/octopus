use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// 本地翻译引擎 trait。支持多引擎扩展（m2m100 / opus-mt / 等）。
pub trait TranslationEngine: Send + Sync {
    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String>;
    fn name(&self) -> &str;
}

/// 全局缓存：spec → engine。spec 变化时按 spec 加载不同引擎。
type EngineCache = parking_lot::Mutex<HashMap<String, Arc<dyn TranslationEngine>>>;
static GLOBAL_CACHE: OnceLock<EngineCache> = OnceLock::new();

pub fn cached_engine(engine_spec: &str) -> Result<Option<Arc<dyn TranslationEngine>>> {
    if !engine_spec.starts_with("local:") {
        return Ok(None);
    }
    let cache = GLOBAL_CACHE.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));

    // 先查缓存
    {
        let guard = cache.lock();
        if let Some(e) = guard.get(engine_spec) {
            return Ok(Some(e.clone()));
        }
    }

    // 按引擎名加载
    let engine_name = &engine_spec["local:".len()..];
    let engine: Arc<dyn TranslationEngine> = if engine_name.starts_with("opus-mt") {
        // opus-mt 需要翻译方向信息才能加载对应子目录（zh-en / en-zh），
        // cached_engine 无方向参数，对 opus-mt 返回 None。
        // 实际加载由 do_translate → load_opus_mt(source, target) 处理。
        return Ok(None);
    } else {
        // 默认：m2m100
        Arc::new(crate::m2m100::M2M100Engine::load()?)
    };

    let mut guard = cache.lock();
    guard.insert(engine_spec.to_string(), engine.clone());
    Ok(Some(engine))
}

/// 加载 opus-mt 引擎（按方向）。与 cached_engine 分开，因为 opus-mt 需要方向信息。
pub fn load_opus_mt(source_lang: &str, target_lang: &str) -> Result<Arc<dyn TranslationEngine>> {
    let cache = GLOBAL_CACHE.get_or_init(|| parking_lot::Mutex::new(HashMap::new()));
    let direction_key = format!("local:opus-mt-{}-{}", source_lang, target_lang);

    {
        let guard = cache.lock();
        if let Some(e) = guard.get(&direction_key) {
            return Ok(e.clone());
        }
    }

    let engine: Arc<dyn TranslationEngine> =
        Arc::new(crate::opus_mt::OpusMTEngine::load(source_lang, target_lang)?);

    let mut guard = cache.lock();
    guard.insert(direction_key, engine.clone());
    Ok(engine)
}

pub struct TranslationManager {
    engine_spec: String,
}

impl TranslationManager {
    pub fn new(engine_spec: &str) -> Self {
        Self { engine_spec: engine_spec.to_string() }
    }

    pub fn engine(&self) -> Result<Option<Arc<dyn TranslationEngine>>> {
        cached_engine(&self.engine_spec)
    }
}
