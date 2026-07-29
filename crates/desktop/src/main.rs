#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod activation;
mod action_bar;
// vault（Task 16+）：AppState + Tauri 命令 + 自动填写
// follow-up #10: vault feature gate——关闭后所有 vault 模块整体 cfg 掉。
// 例外：vault_secret_access **总是**编译（云端推理热路径 chokepoint，feature off 时
// 退化为返回 raw 原值的 no-op）。
#[cfg(feature = "vault")]
pub mod vault_state;
#[cfg(feature = "vault")]
pub mod vault_commands;
#[cfg(feature = "vault")]
pub mod vault_secret_access;
#[cfg(feature = "vault")]
pub mod vault_error;
#[cfg(feature = "vault")]
pub mod vault_sync_commands;
#[cfg(feature = "vault")]
pub mod autotype;
#[cfg(feature = "vault")]
pub mod password_generator_window;
mod overlay_window;
mod finder_selection;
mod app_context;
mod audio;
mod bootstrap;
mod setup;
mod config;
#[macro_use]
mod invoke_handler;
mod clipboard;
mod compact_editor_commands;
mod compact_editor_window;

mod i18n;
mod coordinator;
mod db_queue;
mod engine;
#[cfg(feature = "cloud")]
mod engine_aliyun;
#[cfg(feature = "cloud")]
mod cloud_pipeline;
mod engine_dispatch;
mod engine_embedded;
mod error_util;
mod extensions;
mod file_watcher;
#[cfg(feature = "remote-grpc")]
mod engine_grpc;
#[cfg(feature = "remote-ws")]
mod engine_ws;
mod model_commands;
mod model_migrate;
mod builtin_models;
mod download_window;
mod search_commands;
mod hotword_commands;
mod input_source;
mod keystroke;
mod paste;
mod pin_window;
mod perf_log;
mod pipeline;
mod result_window;
mod screenshot_commands;
mod screenshot_geometry;
mod sys_open;
// 录屏（Task 10，2026-07-25 screen record MVP）：仅 macOS 编译。
// 模块内部 `#![cfg(target_os = "macos")]` 守护，windows/linux 编译时此 mod 整体为空，
// 对应 invoke_handler 注册项也用 cfg gate（见 invoke_handler.rs::handler! 宏）。
#[cfg(target_os = "macos")]
mod record_commands;
// 录屏全局快捷键（Task 14，2026-07-25）：Cmd+Shift+R toggle + Esc stop。
// 与 record_commands 同样仅 macOS 编译。
#[cfg(target_os = "macos")]
mod record_hotkey;
// 录屏配置浮窗（Cmd+Shift+R 弹出，选 display/window/area + 音频开关）。
// 仅 macOS（录屏 helper 只 mac 实现）。
#[cfg(target_os = "macos")]
mod record_window;
// 录屏区域选区 picker（多屏全屏透明覆盖，用户拖框选区域）。
// 仅 macOS。复用 screenshot 的窗口创建 + 坐标换算模式。
#[cfg(target_os = "macos")]
mod record_area_picker;
// 录屏标注 overlay 窗口（录屏开始后显示，普通 level 让 SCK 录到）。
// 仅 macOS。spike7/8 验证：SCK 录窗口 buffer，不录 always_on_top 浮层。
#[cfg(target_os = "macos")]
mod record_annotation_window;
#[cfg(target_os = "macos")]
mod record_control_window;
// 录屏音频元数据探测（Task 2.1，2026-07-27 录后合并 phase）：
// ffprobe 读 mp4 实际音轨 + 给后续 Task 2.2 写 metadata 用。仅 macOS
// （octopus-record + RawAudioTrack 只 mac 编译；与 record_commands.rs 同 gate）。
#[cfg(target_os = "macos")]
mod record_audio_probe;
mod runtime_config;
mod settings_commands;
mod settings_window;
mod onboarding_window;
mod system_status_commands;
mod focus_tracker;
mod shortcut;
mod theme;
mod tray;
mod subtitle_polish;
mod transcript;
mod translation_commands;
mod window_factory;
mod window_position;

use coordinator::Coordinator;
use log::info;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = bootstrap::bootstrap();

    let mut app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            info!("Single instance: re-activated");
            if let Some(coordinator) = app.try_state::<Coordinator>() {
                coordinator.toggle();
            }
        }))
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_os::init())
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Debug)
                .level_for("enigo", log::LevelFilter::Warn)
                .level_for("tao", log::LevelFilter::Warn)
                .level_for("reqwest", log::LevelFilter::Info)
                .level_for("hyper", log::LevelFilter::Info)
                // tract + df::tract（libDF/DF3 模型加载时的 codegen/declutter/shape 推断 DEBUG 极多，
                // df::tract 的 Init encoder / Start init ERB decoder / ERB decoder input 等 Info/Debug
                // 同样刷屏）一律压到 Warn。全局 level(Debug)，未列出的 target 默认走 Debug。
                .level_for("tract_core", log::LevelFilter::Warn)
                .level_for("tract_hir", log::LevelFilter::Warn)
                .level_for("tract_onnx", log::LevelFilter::Warn)
                .level_for("tract_linalg", log::LevelFilter::Warn)
                .level_for("df::tract", log::LevelFilter::Warn)
                .level_for("octopus_desktop::window_position", log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(handler!())
        .setup(move |app| crate::setup::AppSetup::run(app, &config))
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // macOS: 设为 Accessory 模式，不在 Dock 显示图标（纯托盘应用）
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    app.run(move |app, event| {
            // 统一查看器窗口关窗前保存状态（Destroyed 时窗口已销毁，get_webview_window 返回 None）
            if let tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { .. },
                label,
                ..
            } = &event
            {
                if label == "compact_editor_window" {
                    compact_editor_window::on_compact_editor_save_state(app);
                }
            }
            // 设置窗口关闭 → macOS 切回 Accessory（仅托盘，Dock 图标消失）
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::Destroyed,
                label,
                ..
            } = &event
            {
                if label == "settings_window" {
                    settings_window::on_settings_closed(app);
                } else if label == "compact_editor_window" {
                    compact_editor_window::on_compact_editor_closed(app);
                } else if label == "onboarding_window" {
                    onboarding_window::on_onboarding_closed(app);
                }
            }
            // 应用退出前：排空后台 DB 写入队列，避免 Finalize 等命令入队未落库而丢失
            // （录音结束→Finalize 入队→立即退出，是 DB actor 最典型的丢数据路径）。
            if let tauri::RunEvent::ExitRequested { .. } = event {
                db_queue::shutdown_db();
            }
        });
}

fn main() {
    run();
}

/// follow-up #10: cargo feature 探针模块。
///
/// 放独立子模块是为了避开 tauri::command 宏与同模块 generate_handler! 之间的
/// 「macro-expanded macro_export 不能被绝对路径引用」限制（issue #52234）。
/// 命令本身永远注册，不被 vault feature gate——前端据此决定是否渲染 vault UI。
mod feature_flags {
    /// 返回编译期 `cfg!(feature = "vault")`。
    ///
    /// 前端 Settings/index.tsx / App.tsx 启动时 invoke 此命令，按返回值决定是否渲染
    /// VaultPanel nav / vault_picker_window 路由。
    #[tauri::command]
    pub fn is_vault_enabled() -> bool {
        cfg!(feature = "vault")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// follow-up #10: 验证 is_vault_enabled 与 cfg!(feature = "vault") 一致。
        ///
        /// 此测试在两条 feature 路径下都编译（is_vault_enabled 始终注册）。
        /// feature on → true；feature off → false。
        #[test]
        fn test_is_vault_enabled_reflects_cfg() {
            assert_eq!(is_vault_enabled(), cfg!(feature = "vault"));
        }
    }
}
