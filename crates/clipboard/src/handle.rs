use anyhow::Result;
use clipboard_rs::common::ContentFormat;
use clipboard_rs::{Clipboard, ClipboardContext};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub struct ClipboardHandle {
    ctx: Mutex<ClipboardContext>,
    suppress_flag: AtomicBool,
}

impl ClipboardHandle {
    pub fn new() -> Result<Self> {
        let ctx = ClipboardContext::new()
            .map_err(|e| anyhow::anyhow!("Clipboard init failed: {}", e))?;
        Ok(Self {
            ctx: Mutex::new(ctx),
            suppress_flag: AtomicBool::new(false),
        })
    }

    pub fn check_and_clear_suppress(&self) -> bool {
        self.suppress_flag.swap(false, Ordering::SeqCst)
    }

    pub fn write_text(&self, text: &str) -> Result<()> {
        self.suppress_flag.store(true, Ordering::SeqCst);
        let ctx = self.ctx.lock().unwrap();
        ctx.set_text(text.to_string())
            .map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))?;
        Ok(())
    }

    pub fn read_text(&self) -> Result<String> {
        let ctx = self.ctx.lock().unwrap();
        ctx.get_text()
            .map_err(|e| anyhow::anyhow!("Clipboard read failed: {}", e))
    }

    pub fn read_image(&self) -> Result<clipboard_rs::common::RustImageData> {
        let ctx = self.ctx.lock().unwrap();
        ctx.get_image()
            .map_err(|e| anyhow::anyhow!("Clipboard read image failed: {}", e))
    }

    pub fn read_files(&self) -> Result<Vec<String>> {
        let ctx = self.ctx.lock().unwrap();
        ctx.get_files()
            .map_err(|e| anyhow::anyhow!("Clipboard read files failed: {}", e))
    }

    pub fn has(&self, format: ContentFormat) -> bool {
        let ctx = self.ctx.lock().unwrap();
        ctx.has(format)
    }

    pub fn available_formats(&self) -> Result<Vec<String>> {
        let ctx = self.ctx.lock().unwrap();
        ctx.available_formats()
            .map_err(|e| anyhow::anyhow!("Clipboard available_formats failed: {}", e).into())
    }
}
