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
        // helper 输出格式：{"displays":[{id,name,width,height,is_primary}, ...]}
        // （见 crates/record/native/macos/Sources/OctopusSckHelper/main.swift::listDisplaysAndExit）
        // 取 v["displays"] 再反序列化——历史 bug：曾试图把整个 object 当 Vec 反序列化，
        // 失败后被 unwrap_or_default() 静默吞成空 Vec（2026-07-25 实测踩坑）。
        let arr = v.get("displays").unwrap_or(&serde_json::Value::Null);
        let displays: Vec<DisplayInfo> = serde_json::from_value(arr.clone())?;
        Ok(displays)
    }

    fn list_windows(&self) -> RecordResult<Vec<WindowInfo>> {
        let helper = self.resolve_helper_path(None)?;
        let v = futures_block_on(run_helper_subcommand(&helper, "--list-windows"))?;
        let arr = v.get("windows").unwrap_or(&serde_json::Value::Null);
        Ok(serde_json::from_value(arr.clone())?)
    }

    fn list_microphones(&self) -> RecordResult<Vec<MicrophoneInfo>> {
        let helper = self.resolve_helper_path(None)?;
        let v = futures_block_on(run_helper_subcommand(&helper, "--list-microphones"))?;
        let arr = v.get("microphones").unwrap_or(&serde_json::Value::Null);
        Ok(serde_json::from_value(arr.clone())?)
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
