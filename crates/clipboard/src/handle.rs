use anyhow::Result;
use clipboard_rs::common::{ContentFormat, RustImage};
use clipboard_rs::{Clipboard, ClipboardContext};
use std::sync::atomic::{AtomicBool, Ordering};
use parking_lot::Mutex;

pub struct ClipboardHandle {
    ctx: Mutex<ClipboardContext>,
    suppress_flag: AtomicBool,
    /// 是否记录剪贴板历史（运行时开关，对应 AppConfig.clipboard_enabled）。
    /// false 时 on_clipboard_change 直接 return（不存库、不 emit clipboard://changed）。
    /// watcher 始终运行，仅由此 flag 控制是否入库——避免 stop/restart watcher 的线程开销。
    recording_enabled: AtomicBool,
}

impl ClipboardHandle {
    pub fn new() -> Result<Self> {
        let ctx = ClipboardContext::new()
            .map_err(|e| anyhow::anyhow!("Clipboard init failed: {}", e))?;
        Ok(Self {
            ctx: Mutex::new(ctx),
            suppress_flag: AtomicBool::new(false),
            recording_enabled: AtomicBool::new(true),
        })
    }

    pub fn check_and_clear_suppress(&self) -> bool {
        self.suppress_flag.swap(false, Ordering::SeqCst)
    }

    /// 手动设置 suppress flag——下一次 on_clipboard_change 跳过记录。
    /// 用于 action bar 模拟 Cmd+C 前：osascript 直接写系统剪贴板，
    /// 绕过 write_text 的自动 suppress，需手动抑制 watcher。
    pub fn suppress_next(&self) {
        self.suppress_flag.store(true, Ordering::SeqCst);
    }

    /// 清除 suppress flag——用于 trigger 提前返回（未产生剪贴板事件）时撤销 suppress。
    pub fn clear_suppress(&self) {
        self.suppress_flag.store(false, Ordering::SeqCst);
    }

    /// 是否启用剪贴板历史记录（clipboard_enabled 的运行时镜像）。
    pub fn is_recording_enabled(&self) -> bool {
        self.recording_enabled.load(Ordering::SeqCst)
    }

    /// 设置是否记录（set_config 收到 clipboard_enabled 变更时热重载调用）。
    pub fn set_recording_enabled(&self, enabled: bool) {
        self.recording_enabled.store(enabled, Ordering::SeqCst);
    }

    pub fn write_text(&self, text: &str) -> Result<()> {
        self.suppress_flag.store(true, Ordering::SeqCst);
        let ctx = self.ctx.lock();
        ctx.set_text(text.to_string())
            .map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))?;
        Ok(())
    }

    /// 写入 PNG 图片到剪贴板（设置 suppress flag）。
    pub fn write_image(&self, png_bytes: &[u8]) -> Result<()> {
        self.suppress_flag.store(true, Ordering::SeqCst);
        let ctx = self.ctx.lock();
        let img = clipboard_rs::common::RustImageData::from_bytes(png_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to create RustImageData: {}", e))?;
        ctx.set_image(img)
            .map_err(|e| anyhow::anyhow!("Clipboard write image failed: {}", e))?;
        Ok(())
    }

    /// 写入文件路径列表到剪贴板（设置 suppress flag）。
    pub fn write_files(&self, files: Vec<String>) -> Result<()> {
        self.suppress_flag.store(true, Ordering::SeqCst);
        let ctx = self.ctx.lock();
        ctx.set_files(files)
            .map_err(|e| anyhow::anyhow!("Clipboard write files failed: {}", e))?;
        Ok(())
    }

    /// 直接写入 RustImageData 到剪贴板（设置 suppress flag）。
    /// 还原备份的图片时用，避免 write_image(&[u8]) 内部 from_bytes 二次解码。
    pub fn set_image(&self, img: clipboard_rs::common::RustImageData) -> Result<()> {
        self.suppress_flag.store(true, Ordering::SeqCst);
        let ctx = self.ctx.lock();
        ctx.set_image(img)
            .map_err(|e| anyhow::anyhow!("Clipboard set image failed: {}", e))?;
        Ok(())
    }

    pub fn read_text(&self) -> Result<String> {
        let ctx = self.ctx.lock();
        ctx.get_text()
            .map_err(|e| anyhow::anyhow!("Clipboard read failed: {}", e))
    }

    pub fn read_image(&self) -> Result<clipboard_rs::common::RustImageData> {
        let ctx = self.ctx.lock();
        ctx.get_image()
            .map_err(|e| anyhow::anyhow!("Clipboard read image failed: {}", e))
    }

    pub fn read_files(&self) -> Result<Vec<String>> {
        let ctx = self.ctx.lock();
        ctx.get_files()
            .map_err(|e| anyhow::anyhow!("Clipboard read files failed: {}", e))
    }

    pub fn has(&self, format: ContentFormat) -> bool {
        let ctx = self.ctx.lock();
        ctx.has(format)
    }

    /// 快捷检测：剪贴板当前是否有图片
    pub fn has_image(&self) -> bool {
        self.has(ContentFormat::Image)
    }

    /// 快捷检测：剪贴板当前是否有文件
    pub fn has_files(&self) -> bool {
        self.has(ContentFormat::Files)
    }

    pub fn available_formats(&self) -> Result<Vec<String>> {
        let ctx = self.ctx.lock();
        ctx.available_formats()
            .map_err(|e| anyhow::anyhow!("Clipboard available_formats failed: {}", e))
    }
}
