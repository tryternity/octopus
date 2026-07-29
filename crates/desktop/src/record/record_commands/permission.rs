//! 录屏权限检测/请求命令子模块（macOS 独占）。
//!
//! 从 record_commands/mod.rs 拆出（Task 1.1）。包含：
//! - 屏幕录制权限（check / request，主进程 FFI）
//! - 麦克风权限（cpal probe，check = request 同实现）
//! - 辅助功能权限（check / request）
//! - open_privacy_settings（系统偏好设置跳转）

#![cfg(target_os = "macos")]

use octopus_record::{PermissionStatus, PrivacySection};
use tauri::command;

use crate::core::error_util::e2s;

#[command]
pub async fn check_record_permission() -> Result<PermissionStatus, String> {
    // 主进程 FFI（CGPreflightScreenCaptureAccess）——helper 子进程的 --check-permission
    // 在打包版不可靠（TCC 对子进程行为不一致）。三态映射：
    //   preflight true → Granted；false → NotDetermined（未请求过）或 Denied（请求被拒）
    // macOS 无 API 区分 NotDetermined 与 Denied，统一返 NotDetermined 让前端显示「申请权限」。
    Ok(if crate::platform::app_context::ffi::is_screen_capture_trusted() {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    })
}

#[command]
pub async fn request_screen_record_permission() -> Result<PermissionStatus, String> {
    // 主进程 FFI（CGRequestScreenCaptureAccess）——必须在主进程调，
    // helper 子进程调此函数不触发 TCC 弹窗（打包版 bug 根因）。
    crate::platform::app_context::ffi::prompt_screen_capture_permission();
    // 弹窗异步——返回当前态（首次几乎一定 NotDetermined），前端 setTimeout 重查
    Ok(if crate::platform::app_context::ffi::is_screen_capture_trusted() {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    })
}

/// 打开 macOS 系统偏好设置里的隐私面板。
///
/// 用 `x-apple.systempreferences:` URL scheme 直跳指定 section，
/// 比 opener crate 多一次进程 fork 但少一个依赖，与项目惯例（clipboard_commands /
/// search_commands 一律 std::process::Command::new("open")）一致。
#[command]
pub async fn open_privacy_settings(section: PrivacySection) -> Result<(), String> {
    let url = match section {
        PrivacySection::ScreenCapture => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        PrivacySection::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        PrivacySection::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Assistive"
        }
    };
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map_err(e2s)?;
    Ok(())
}

// ── B0. 权限检查/申请（引导页用，4 个）─────────────────────────

/// macOS 麦克风权限检查——cpal probe。
///
/// macOS 无直接查询麦克风授权态的 API（不像屏幕录制有 CGPreflightScreenCaptureAccess）。
/// 唯一可靠探测：尝试 `build_input_stream` + `play`——
///   - 授权：成功，立即 pause+drop（不真录音）
///   - 未授权/拒绝：build 或 play 失败
/// 副作用：首次调用（未授权态）触发 TCC 弹窗——故 check 与 request 实为同一实现。
///
/// **格式适配**（曾踩坑）：`default_input_config()` 可能返回 F32/I16/U16 任一格式，
/// build_input_stream 的 callback 类型必须匹配。按 config.sample_format() 分派，
/// 与 audio.rs::build_stream 同模式。
fn probe_microphone_permission() -> PermissionStatus {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => return PermissionStatus::Denied,
    };
    let config = match device.default_input_config() {
        Ok(c) => c,
        Err(_) => return PermissionStatus::Denied,
    };
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |_data: &[f32], _: &cpal::InputCallbackInfo| {},
            |_: cpal::StreamError| {},
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.into(),
            move |_data: &[i16], _: &cpal::InputCallbackInfo| {},
            |_: cpal::StreamError| {},
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config.into(),
            move |_data: &[u16], _: &cpal::InputCallbackInfo| {},
            |_: cpal::StreamError| {},
            None,
        ),
        _ => return PermissionStatus::Denied,
    };
    let stream = match stream {
        Ok(s) => s,
        Err(_) => return PermissionStatus::Denied,
    };
    if stream.play().is_err() {
        return PermissionStatus::Denied;
    }
    // pause + drop 释放 stream（cpal Stream 无 stop()，drop 即停止+释放资源）
    let _ = StreamTrait::pause(&stream);
    drop(stream);
    PermissionStatus::Granted
}

#[command]
pub async fn check_microphone_permission() -> Result<PermissionStatus, String> {
    Ok(probe_microphone_permission())
}

#[command]
pub async fn request_microphone_permission() -> Result<PermissionStatus, String> {
    // 与 check 同实现——probe 的 play 即触发 TCC 弹窗
    Ok(probe_microphone_permission())
}

#[command]
pub async fn check_accessibility_permission() -> Result<PermissionStatus, String> {
    Ok(if crate::platform::app_context::ffi::is_accessibility_trusted() {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    })
}

#[command]
pub async fn request_accessibility_permission() -> Result<PermissionStatus, String> {
    // prompt 触发 TCC 弹窗，返回当前态（首次几乎一定 false，前端 setTimeout 重查）
    crate::platform::app_context::ffi::prompt_accessibility_permission();
    Ok(if crate::platform::app_context::ffi::is_accessibility_trusted() {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    })
}
