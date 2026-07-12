use anyhow::Result;
use std::sync::{Arc, OnceLock};

/// 本地翻译引擎 trait。支持多引擎扩展（m2m100 / NLLB / 等）。
pub trait TranslationEngine: Send + Sync {
    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String>;
    fn name(&self) -> &str;
}

/// 全局缓存：spec → engine。spec 变化时自动重新加载。
static GLOBAL_ENGINE: OnceLock<parking_lot::Mutex<(String, Option<Arc<dyn TranslationEngine>>)>> = OnceLock::new();

pub fn cached_engine(engine_spec: &str) -> Result<Option<Arc<dyn TranslationEngine>>> {
    if !engine_spec.starts_with("local:") {
        return Ok(None);
    }
    let cell = GLOBAL_ENGINE.get_or_init(|| parking_lot::Mutex::new((String::new(), None)));
    let mut guard = cell.lock();
    // spec 相同且有缓存 → 直接返回
    if guard.0 == engine_spec && guard.1.is_some() {
        return Ok(guard.1.clone());
    }
    // spec 改变或首次加载
    let e: Arc<dyn TranslationEngine> = Arc::new(crate::m2m100::M2M100Engine::load()?);
    *guard = (engine_spec.to_string(), Some(e.clone()));
    Ok(Some(e))
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
