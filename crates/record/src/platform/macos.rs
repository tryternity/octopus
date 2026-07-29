//! MacOSProvider：macOS 平台的 helper 二进制查找与子命令调用。

use crate::error::{RecordError, RecordResult};
use crate::platform::{run_helper_subcommand, HelperProvider};
use crate::protocol::{DisplayInfo, MicrophoneInfo, PermissionStatus, WindowInfo};
use async_trait::async_trait;
use std::path::PathBuf;

pub struct MacOSProvider;

#[async_trait]
impl HelperProvider for MacOSProvider {
    fn resolve_helper_path(&self, app_resource_dir: Option<&std::path::Path>) -> RecordResult<PathBuf> {
        // 1. 显式传入的 resource_dir（优先级最高，测试 / 覆盖用）
        if let Some(dir) = app_resource_dir {
            let candidate = dir.join("binaries").join("octopus-sck-helper");
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        // 2. Tauri .app bundle：exe 在 Contents/MacOS/，resources 在 Contents/Resources/binaries/
        //    （exe-relative 几何，与 seeds_dir() 复用 infra::paths::tauri_app_resource）
        //    所有调用方（含传 None 的 5 个内部 trait 方法）打包后都走这里。
        if let Some(p) = octopus_infra::paths::tauri_app_resource("binaries/octopus-sck-helper") {
            return Ok(p);
        }
        // 3. 开发期路径：crates/desktop/binaries/octopus-sck-helper
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

    async fn list_displays(&self) -> RecordResult<Vec<DisplayInfo>> {
        let helper = self.resolve_helper_path(None)?;
        let v = run_helper_subcommand(&helper, "--list-displays").await?;
        // helper 输出格式：{"displays":[{id,name,width,height,is_primary}, ...]}
        // （见 crates/record/native/macos/Sources/OctopusSckHelper/main.swift::listDisplaysAndExit）
        // 取 v["displays"] 再反序列化——历史 bug：曾试图把整个 object 当 Vec 反序列化，
        // 失败后被 unwrap_or_default() 静默吞成空 Vec（2026-07-25 实测踩坑）。
        let arr = v.get("displays").unwrap_or(&serde_json::Value::Null);
        let displays: Vec<DisplayInfo> = serde_json::from_value(arr.clone())?;
        Ok(displays)
    }

    async fn list_windows(&self) -> RecordResult<Vec<WindowInfo>> {
        let helper = self.resolve_helper_path(None)?;
        let v = run_helper_subcommand(&helper, "--list-windows").await?;
        let arr = v.get("windows").unwrap_or(&serde_json::Value::Null);
        Ok(serde_json::from_value(arr.clone())?)
    }

    async fn list_microphones(&self) -> RecordResult<Vec<MicrophoneInfo>> {
        let helper = self.resolve_helper_path(None)?;
        let v = run_helper_subcommand(&helper, "--list-microphones").await?;
        let arr = v.get("microphones").unwrap_or(&serde_json::Value::Null);
        Ok(serde_json::from_value(arr.clone())?)
    }

    async fn check_permission(&self) -> RecordResult<PermissionStatus> {
        let helper = self.resolve_helper_path(None)?;
        let v = run_helper_subcommand(&helper, "--check-permission").await?;
        let granted = v.get("granted").and_then(|g| g.as_bool()).unwrap_or(false);
        Ok(if granted { PermissionStatus::Granted } else { PermissionStatus::Denied })
    }

    async fn request_screen_permission(&self) -> RecordResult<PermissionStatus> {
        let helper = self.resolve_helper_path(None)?;
        let v = run_helper_subcommand(&helper, "--request-permission").await?;
        let granted = v.get("granted").and_then(|g| g.as_bool()).unwrap_or(false);
        Ok(if granted { PermissionStatus::Granted } else { PermissionStatus::Denied })
    }
}
