use anyhow::Result;
use std::sync::Arc;

/// 本地翻译引擎 trait。支持多引擎扩展（m2m100 / NLLB / 等）。
pub trait TranslationEngine: Send + Sync {
    /// 翻译文本。source_lang / target_lang 用 ISO 639-1 代码（"zh" / "en"）。
    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String>;

    /// 引擎显示名（如 "m2m100-418M"）。
    fn name(&self) -> &str;
}

/// 翻译引擎管理器——lazy load + 缓存，类似 AsrEngineManager。
pub struct TranslationManager {
    engine: parking_lot::Mutex<Option<Arc<dyn TranslationEngine>>>,
    engine_spec: String,
}

impl TranslationManager {
    pub fn new(engine_spec: &str) -> Self {
        Self {
            engine: parking_lot::Mutex::new(None),
            engine_spec: engine_spec.to_string(),
        }
    }

    /// 获取引擎（lazy load：首次调用时加载模型）。
    pub fn engine(&self) -> Result<Option<Arc<dyn TranslationEngine>>> {
        let mut guard = self.engine.lock();
        if guard.is_some() {
            return Ok(guard.clone());
        }
        if self.engine_spec == "local:m2m100" {
            let e: Arc<dyn TranslationEngine> = Arc::new(crate::m2m100::M2M100Engine::load()?);
            *guard = Some(e.clone());
            return Ok(Some(e));
        }
        Ok(None)
    }
}
