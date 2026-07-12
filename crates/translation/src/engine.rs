use anyhow::Result;
use std::sync::{Arc, OnceLock};

/// 本地翻译引擎 trait。支持多引擎扩展（m2m100 / NLLB / 等）。
pub trait TranslationEngine: Send + Sync {
    /// 翻译文本。source_lang / target_lang 用 ISO 639-1 代码（"zh" / "en"）。
    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> Result<String>;

    /// 引擎显示名（如 "m2m100-418M"）。
    fn name(&self) -> &str;
}

/// 全局缓存的翻译引擎——进程生命周期内只加载一次 ONNX 模型。
static GLOBAL_ENGINE: OnceLock<parking_lot::Mutex<Option<Arc<dyn TranslationEngine>>>> = OnceLock::new();

/// 获取全局缓存的翻译引擎。首次调用时从磁盘加载模型，后续直接返回缓存。
/// engine_spec 为空或 "llm" 返回 None；"local:*" 触发加载。
pub fn cached_engine(engine_spec: &str) -> Result<Option<Arc<dyn TranslationEngine>>> {
    if !engine_spec.starts_with("local:") {
        return Ok(None);
    }
    let cell = GLOBAL_ENGINE.get_or_init(|| parking_lot::Mutex::new(None));
    let mut guard = cell.lock();
    if guard.is_some() {
        return Ok(guard.clone());
    }
    let e: Arc<dyn TranslationEngine> = Arc::new(crate::m2m100::M2M100Engine::load()?);
    *guard = Some(e.clone());
    Ok(Some(e))
}

/// 翻译引擎管理器——保留兼容，内部委托给全局缓存。
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
