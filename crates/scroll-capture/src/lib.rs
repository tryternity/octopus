pub mod overlay;
pub mod stitch;
mod recording;
#[cfg(target_os = "macos")]
mod macos;

use std::sync::atomic::{AtomicBool, Ordering};

static RECORDING: AtomicBool = AtomicBool::new(false);

/// Start scroll capture: create overlay windows, wait for user selection, then record.
pub fn start(on_complete: Box<dyn FnOnce(Vec<u8>) + Send + 'static>) {
    RECORDING.store(false, Ordering::SeqCst);
    #[cfg(target_os = "macos")]
    macos::run(on_complete);
    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("scroll-capture: only macOS is supported");
    }
}

/// Stop recording (tray menu / ESC).
pub fn stop() {
    RECORDING.store(false, Ordering::SeqCst);
}

/// 是否正在录制（托盘菜单切换用）。
pub fn is_recording_active() -> bool {
    RECORDING.load(Ordering::SeqCst)
}

pub(crate) fn is_recording() -> bool {
    RECORDING.load(Ordering::SeqCst)
}

pub(crate) fn set_recording(v: bool) {
    RECORDING.store(v, Ordering::SeqCst);
}
