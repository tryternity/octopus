//! LinuxProvider：占位（P2+ 待调研 PipeWire/X11）。

use crate::error::RecordError;
use crate::platform::HelperProvider;
use crate::protocol::{DisplayInfo, MicrophoneInfo, PermissionStatus, WindowInfo};
use std::path::PathBuf;

pub struct LinuxProvider;

impl HelperProvider for LinuxProvider {
    fn resolve_helper_path(&self, _: Option<&std::path::Path>) -> Result<PathBuf, RecordError> {
        Err(RecordError::PlatformNotImplemented("linux helper (P2+ 待调研 PipeWire/X11)"))
    }
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, RecordError> {
        Err(RecordError::PlatformNotImplemented("linux"))
    }
    fn list_windows(&self) -> Result<Vec<WindowInfo>, RecordError> {
        Err(RecordError::PlatformNotImplemented("linux"))
    }
    fn list_microphones(&self) -> Result<Vec<MicrophoneInfo>, RecordError> {
        Err(RecordError::PlatformNotImplemented("linux"))
    }
    fn check_permission(&self) -> Result<PermissionStatus, RecordError> {
        Err(RecordError::PlatformNotImplemented("linux"))
    }
    fn request_screen_permission(&self) -> Result<PermissionStatus, RecordError> {
        Err(RecordError::PlatformNotImplemented("linux"))
    }
}
