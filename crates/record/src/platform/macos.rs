//! MacOSProvider：macOS 平台的 helper 二进制查找与子命令调用。

use crate::error::{RecordError, RecordResult};
use crate::platform::{run_helper_subcommand, HelperProvider};
use crate::protocol::{DisplayInfo, MicrophoneInfo, PermissionStatus, WindowInfo};
use std::path::PathBuf;

pub struct MacOSProvider;

impl HelperProvider for MacOSProvider {
    fn resolve_helper_path(&self, app_resource_dir: Option<&std::path::Path>) -> RecordResult<PathBuf> {
        // 1. 打包后路径：Contents/Resources/binaries/octopus-sck-helper
        if let Some(dir) = app_resource_dir {
            let candidate = dir.join("binaries").join("octopus-sck-helper");
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        // 2. 开发期路径：crates/desktop/binaries/octopus-sck-helper
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let dev_path = PathBuf::from(manifest_dir)
            .join("../desktop/binaries/octopus-sck-helper");
        if dev_path.exists() {
            return Ok(dev_path);
        }
        Err(RecordError::HelperNotFound(
            app_resource_dir
                .map(|d| d.join("binaries/octopus-sck-helper"))
                .unwrap_or_else(|| PathBuf::from("octopus-sck-helper")),
        ))
    }

    fn list_displays(&self) -> RecordResult<Vec<DisplayInfo>> {
        let helper = self.resolve_helper_path(None)?;
        let v = futures_block_on(run_helper_subcommand(&helper, "--list-displays"))?;
        let displays: Vec<DisplayInfo> = serde_json::from_value(v)
            .unwrap_or_default();
        Ok(displays)
    }

    fn list_windows(&self) -> RecordResult<Vec<WindowInfo>> {
        let helper = self.resolve_helper_path(None)?;
        let v = futures_block_on(run_helper_subcommand(&helper, "--list-windows"))?;
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    fn list_microphones(&self) -> RecordResult<Vec<MicrophoneInfo>> {
        let helper = self.resolve_helper_path(None)?;
        let v = futures_block_on(run_helper_subcommand(&helper, "--list-microphones"))?;
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    fn check_permission(&self) -> RecordResult<PermissionStatus> {
        let helper = self.resolve_helper_path(None)?;
        let v = futures_block_on(run_helper_subcommand(&helper, "--check-permission"))?;
        let granted = v.get("granted").and_then(|g| g.as_bool()).unwrap_or(false);
        Ok(if granted { PermissionStatus::Granted } else { PermissionStatus::Denied })
    }

    fn request_screen_permission(&self) -> RecordResult<PermissionStatus> {
        let helper = self.resolve_helper_path(None)?;
        let v = futures_block_on(run_helper_subcommand(&helper, "--request-permission"))?;
        let granted = v.get("granted").and_then(|g| g.as_bool()).unwrap_or(false);
        Ok(if granted { PermissionStatus::Granted } else { PermissionStatus::Denied })
    }
}

/// 同步等待异步 future（platform trait 是同步的，简化 MVP）。
/// 完整版应让 trait 方法也 async，但 MVP 不引入复杂度。
fn futures_block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::task::block_in_place(|| {
        let runtime = tokio::runtime::Handle::current();
        runtime.block_on(f)
    })
}
