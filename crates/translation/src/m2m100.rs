use anyhow::Result;
use crate::engine::TranslationEngine;

pub struct M2M100Engine;

impl M2M100Engine {
    pub fn load() -> Result<Self> {
        anyhow::bail!("M2M100Engine 尚未实现（Task 2）")
    }
}

impl TranslationEngine for M2M100Engine {
    fn translate(&self, _text: &str, _source_lang: &str, _target_lang: &str) -> Result<String> {
        anyhow::bail!("M2M100Engine 尚未实现")
    }
    fn name(&self) -> &str { "m2m100-418M" }
}
