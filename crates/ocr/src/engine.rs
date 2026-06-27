use anyhow::Result;
use std::sync::Arc;

pub struct OcrEngine {
    inner: ocr_rs::OcrEngine,
}

impl OcrEngine {
    pub fn instance() -> Result<Arc<OcrEngine>> {
        anyhow::bail!("not implemented yet")
    }

    pub fn recognize(&self, _png_bytes: &[u8]) -> Result<String> {
        anyhow::bail!("not implemented yet")
    }
}
