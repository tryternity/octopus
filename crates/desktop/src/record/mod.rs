//! 录屏 + 截图功能域。

pub mod screenshot_commands;
pub mod screenshot_geometry;
pub mod subtitle_polish;
// macOS 独占：
#[cfg(target_os = "macos")]
pub mod record_commands;
#[cfg(target_os = "macos")]
pub mod record_area_picker;
#[cfg(target_os = "macos")]
pub mod record_audio_probe;
#[cfg(target_os = "macos")]
pub mod record_hotkey;
#[cfg(target_os = "macos")]
pub mod record_window;
#[cfg(target_os = "macos")]
pub mod record_annotation_window;
#[cfg(target_os = "macos")]
pub mod record_control_window;
