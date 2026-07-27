//! WindowsProvider：占位（P1 实现 vendor openscreen C++ wgc-capture）。

use crate::error::RecordError;
use crate::platform::HelperProvider;
use crate::protocol::{DisplayInfo, MicrophoneInfo, PermissionStatus, WindowInfo};
use async_trait::async_trait;
use std::path::PathBuf;

pub struct WindowsProvider;

#[async_trait]
impl HelperProvider for WindowsProvider {
    fn resolve_helper_path(&self, _: Option<&std::path::Path>) -> Result<PathBuf, RecordError> {
        Err(RecordError::PlatformNotImplemented("windows helper not yet implemented (P1)"))
    }
    async fn list_displays(&self) -> Result<Vec<DisplayInfo>, RecordError> {
        Err(RecordError::PlatformNotImplemented("windows"))
    }
    async fn list_windows(&self) -> Result<Vec<WindowInfo>, RecordError> {
        Err(RecordError::PlatformNotImplemented("windows"))
    }
    async fn list_microphones(&self) -> Result<Vec<MicrophoneInfo>, RecordError> {
        Err(RecordError::PlatformNotImplemented("windows"))
    }
    async fn check_permission(&self) -> Result<PermissionStatus, RecordError> {
        Err(RecordError::PlatformNotImplemented("windows"))
    }
    async fn request_screen_permission(&self) -> Result<PermissionStatus, RecordError> {
        Err(RecordError::PlatformNotImplemented("windows"))
    }
}
