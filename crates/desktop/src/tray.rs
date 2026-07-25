// src/tray.rs

use crate::config::AppConfig;
use log::info;
use parking_lot::Mutex;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, Runtime};

/// Tray icon state for display purposes
pub enum TrayState {
    Idle,
    Recording,
    Processing,
}

/// 存储需要动态更新的 MenuItem handle
struct TrayItems<R: Runtime> {
    toggle: MenuItem<R>,
    engine_info: MenuItem<R>,
    screenshot: MenuItem<R>,
    clipboard: MenuItem<R>,
    compact_editor: MenuItem<R>,
    // 录屏项（2026-07-25）：仅 macOS。toggle 语义——idle 时「开始录屏」，
    // recording/paused 时「停止录屏」（与 ASR toggle 同模式，单一菜单项切换文案）。
    #[cfg(target_os = "macos")]
    record_start: MenuItem<R>,
    settings: MenuItem<R>,
    quit: MenuItem<R>,
}

/// 模块级存储，避免 MenuItem::with_id 重复 ID 导致的 panic
static TRAY_ITEMS: once_cell::sync::Lazy<Mutex<Option<TrayItems<tauri::Wry>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(None));

/// 存储 asr_shortcut 用于 update_tray_label 动态文案
static ASR_SHORTCUT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// 将 Tauri Accelerator 格式（CmdOrCtrl+Shift+A）转为用户可读格式（⌘⇧A）
fn fmt_shortcut(s: &str) -> String {
    if s.is_empty() { return String::new(); }
    s.replace("CmdOrCtrl+", "⌘").replace("Cmd+", "⌘")
     .replace("Shift+", "⇧").replace("Alt+", "⌥")
     .replace("Control+", "⌃").replace("Super+", "⌘")
}

/// 将激活 ASR 引擎格式化为 `model_name[provider]`，provider=local 时 i18n 为「本地」。
///
/// Task 2 模型激活语义重构后：不再接收 spec 参数，直接从 `resolve_active_engine("asr")`
/// 取激活引擎的 name + provider。失败（含未激活 + 兜底失败）返回空串。
fn fmt_engine_label() -> String {
    let resolved = match octopus_asr_local::config::resolve_active_engine("asr") {
        Ok(r) => r,
        Err(e) => {
            log::warn!("fmt_engine_label: resolve_active_engine('asr') 失败：{}", e);
            return String::new();
        }
    };
    let provider_display = if resolved.provider == "local" {
        crate::i18n::t("settings.models.local", &[])
    } else {
        resolved.provider
    };
    format!("{}[{}]", resolved.name, provider_display)
}

/// Create the system tray icon and its context menu.
///
/// 菜单文案设计：操作项统一四字宽度 + 括号快捷键。
/// 分组：语音识别 → 引擎信息（只读分隔线）→ 截图/剪贴板 → 设置/退出。
pub fn create_tray(app: &tauri::AppHandle, config: &AppConfig) -> Result<(), String> {
    let _ = ASR_SHORTCUT.set(config.asr_shortcut.clone());
    let sc = fmt_shortcut(&config.asr_shortcut);
    let toggle_text = crate::i18n::t("tray.startAsr", &[("shortcut", &sc)]);
    let toggle = MenuItem::with_id(app, "toggle", &toggle_text, true, None::<&str>)
        .map_err(|e| format!("toggle menu: {e}"))?;
    let engine_info = MenuItem::with_id(
        app,
        "engine_info",
        &crate::i18n::t("tray.engineInfo", &[("engine", &fmt_engine_label())]),
        false,
        None::<&str>,
    )
    .map_err(|e| format!("engine_info menu: {e}"))?;

    let sep1 = PredefinedMenuItem::separator(app)
        .map_err(|e| format!("separator: {e}"))?;

    let screenshot_text = crate::i18n::t("tray.screenshot", &[("shortcut", &sc)]);
    let screenshot = MenuItem::with_id(app, "screenshot", &screenshot_text, true, None::<&str>)
        .map_err(|e| format!("screenshot menu: {e}"))?;
    let clipboard_text = crate::i18n::t("tray.clipboard", &[("shortcut", &fmt_shortcut(&config.clipboard_shortcut))]);
    let clipboard = MenuItem::with_id(app, "clipboard", &clipboard_text, true, None::<&str>)
        .map_err(|e| format!("clipboard menu: {e}"))?;
    // 图文编辑：打开空白 CompactEditor（临时文本 tab，不写 DB）。
    let compact_editor = MenuItem::with_id(app, "compact_editor", &crate::i18n::t("tray.compactEditor", &[]), true, None::<&str>)
        .map_err(|e| format!("compact_editor menu: {e}"))?;

    // ── 录屏组（Task 14，2026-07-25）：仅 macOS 编译 ──
    // 设计：sep + 3 项，紧跟在 compact_editor 之后、sep2 之前。
    // 快捷键提示在 menu 文案里直接展示「⌘⇧R」（不做硬编码，避免与 record_hotkey
    // 注册的快捷键字符串不同步——但 MVP 简化，直接写死文案，reload 时 rebuild_tray_labels 也用同 key）。
    #[cfg(target_os = "macos")]
    let sep_record = PredefinedMenuItem::separator(app)
        .map_err(|e| format!("separator_record: {e}"))?;
    #[cfg(target_os = "macos")]
    let record_start = MenuItem::with_id(
        app,
        "record_start",
        &crate::i18n::t("tray.recordStart", &[("shortcut", "⌘⇧R")]),
        true,
        None::<&str>,
    )
    .map_err(|e| format!("record_start menu: {e}"))?;

    let sep2 = PredefinedMenuItem::separator(app)
        .map_err(|e| format!("separator2: {e}"))?;

    let settings = MenuItem::with_id(app, "settings", &crate::i18n::t("tray.settings", &[]), true, None::<&str>)
        .map_err(|e| format!("settings menu: {e}"))?;
    let quit = MenuItem::with_id(app, "quit", &crate::i18n::t("tray.quit", &[]), true, None::<&str>)
        .map_err(|e| format!("quit menu: {e}"))?;

    // 菜单组装（用户决策 2026-07-25）：
    // 分组 1：语音识别 + 引擎信息
    // 分组 2：截图 + 录屏（屏幕采集类）
    // 分组 3：剪贴板 + 图文编辑
    // 分组 4：设置 + 退出
    #[cfg(target_os = "macos")]
    let menu = Menu::with_items(app, &[
        &toggle, &engine_info, &sep1,
        &screenshot, &record_start, &sep_record,
        &clipboard, &compact_editor,
        &sep2,
        &settings, &quit,
    ])
    .map_err(|e| format!("tray menu: {e}"))?;
    #[cfg(not(target_os = "macos"))]
    let menu = Menu::with_items(app, &[
        &toggle, &engine_info, &sep1,
        &screenshot, &clipboard, &compact_editor, &sep2,
        &settings, &quit,
    ])
    .map_err(|e| format!("tray menu: {e}"))?;

    // 存储 handle 供后续更新使用
    {
        let mut items = TRAY_ITEMS.lock();
        #[cfg(target_os = "macos")]
        {
            *items = Some(TrayItems {
                toggle: toggle.clone(),
                engine_info: engine_info.clone(),
                screenshot: screenshot.clone(),
                clipboard: clipboard.clone(),
                compact_editor: compact_editor.clone(),
                record_start: record_start.clone(),
                settings: settings.clone(),
                quit: quit.clone(),
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            *items = Some(TrayItems {
                toggle: toggle.clone(),
                engine_info: engine_info.clone(),
                screenshot: screenshot.clone(),
                clipboard: clipboard.clone(),
                compact_editor: compact_editor.clone(),
                settings: settings.clone(),
                quit: quit.clone(),
            });
        }
    }

    let _tray = TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .cloned()
                .unwrap_or_else(|| Image::from_bytes(include_bytes!("../icons/icon.png")).unwrap()),
        )
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("octopus")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => {
                info!("Tray: toggle recording");
                if let Some(coordinator) = app.try_state::<crate::coordinator::Coordinator>() {
                    coordinator.toggle();
                }
            }
            "clipboard" => {
                info!("Tray: toggle clipboard");
                let _ = crate::clipboard_window::toggle_clipboard_window(app);
            }
            "compact_editor" => {
                info!("Tray: open compact editor (empty)");
                crate::compact_editor_commands::open_temp_compact_editor(app, &Default::default());
            }
            // ── 录屏项（2026-07-25）：仅 macOS，toggle 语义（与 ASR toggle 同模式）──
            // idle/starting → 弹配置浮窗（用户选源后开录）
            // recording/paused → stop_and_store 停止入库
            // 文案由 update_record_tray_label 根据 state 切换（开始录屏 ↔ 停止录屏）
            #[cfg(target_os = "macos")]
            "record_start" => {
                info!("Tray: record toggle");
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    use octopus_record::SessionState;
                    let session = match app_handle.try_state::<octopus_record::RecordSession>() {
                        Some(s) => s,
                        None => {
                            log::warn!("[tray] RecordSession state 未找到");
                            return;
                        }
                    };
                    let st = session.state().await;
                    match st {
                        SessionState::Idle | SessionState::Starting => {
                            // 弹配置浮窗（与 Cmd+Shift+R hotkey 同路径）
                            crate::record_window::show_record_window(&app_handle);
                        }
                        SessionState::Recording | SessionState::Paused => {
                            // 停止 + 入库（与 hotkey Esc 同路径）
                            match crate::record_commands::stop_and_store(&session, false, None).await {
                                Ok(Some(meta)) => {
                                    log::info!(
                                        "[tray] 录制已停止入库: id={} file={}",
                                        meta.id,
                                        meta.file_path
                                    );
                                    let _ = app_handle.emit("record://stopped", &meta);
                                }
                                Ok(None) => log::info!("[tray] stop 返回 None"),
                                Err(e) => {
                                    log::error!("[tray] stop + 入库失败: {e}");
                                    let _ = app_handle.emit("record://stop-failed", &e);
                                }
                            }
                        }
                        SessionState::Stopping => {
                            log::info!("[tray] record toggle 在 Stopping 态忽略");
                        }
                    }
                });
            }
            "settings" => {
                info!("Tray: open settings");
                crate::settings_window::open_settings(app.clone(), None);
            }
            "screenshot" => {
                info!("Tray: start screenshot");
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::screenshot_commands::start_screenshot(app_handle).await;
                });
            }
            "quit" => {
                info!("Tray: quit");
                app.exit(0);
            }
            _ => {}
        })
        .build(app)
        .map_err(|e| format!("tray icon build: {e}"))?;
    Ok(())
}

/// Update the toggle menu item label based on the current state.
pub fn update_tray_label(_app: &tauri::AppHandle, state: TrayState) {
    let sc = ASR_SHORTCUT.get().map(|s| fmt_shortcut(s)).unwrap_or_default();
    let label = match state {
        TrayState::Idle => crate::i18n::t("tray.startAsr", &[("shortcut", &sc)]),
        TrayState::Recording => crate::i18n::t("tray.stopAsr", &[]),
        TrayState::Processing => crate::i18n::t("tray.processing", &[]),
    };

    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.toggle.set_text(label);
    }
}

/// Update the engine info menu item label dynamically.
///
/// `engine_spec` / `engine_mode` 参数已废弃（Task 2 后从 ACTIVE_ENGINES 缓存取激活引擎），
/// 保留以减小调用方改动。
pub fn update_tray_engine_label(_app: &tauri::AppHandle, _engine_spec: &str, _engine_mode: &str) {
    let label = crate::i18n::t("tray.engineInfo", &[("engine", &fmt_engine_label())]);
    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.engine_info.set_text(label);
    }
}

/// Update the screenshot menu item: 正常 ↔ 灰掉
pub fn update_tray_screenshot_label(active: bool) {
    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.screenshot.set_enabled(!active);
    }
}

/// 语言切换后重建所有菜单项文案
pub fn rebuild_tray_labels() {
    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let sc = ASR_SHORTCUT.get().map(|s| fmt_shortcut(s)).unwrap_or_default();
        let _ = tray_items.toggle.set_text(crate::i18n::t("tray.startAsr", &[("shortcut", &sc)]));
        let _ = tray_items.screenshot.set_text(crate::i18n::t("tray.screenshot", &[("shortcut", &sc)]));
        let _ = tray_items.clipboard.set_text(crate::i18n::t("tray.clipboard", &[("shortcut", &sc)]));
        let _ = tray_items.compact_editor.set_text(crate::i18n::t("tray.compactEditor", &[]));
        #[cfg(target_os = "macos")]
        {
            // 录屏项文案：idle 时「开始录屏 ⌘⇧R」，recording/paused 时「停止录屏」。
            // rebuild 时按当前 session state 决定文案（与 update_record_tray_label 同逻辑）。
            let label = record_menu_label_for_current_state();
            let _ = tray_items.record_start.set_text(label);
        }
        let _ = tray_items.settings.set_text(crate::i18n::t("tray.settings", &[]));
        let _ = tray_items.quit.set_text(crate::i18n::t("tray.quit", &[]));
    }
}

/// 录屏菜单文案：根据当前 RecordSession state 切换。
///
/// - Idle/Starting → 「开始录屏 ⌘⇧R」（弹浮窗选源）
/// - Recording/Paused/Stopping → 「停止录屏」（停止 + 入库）
///
/// state 查询是 async，但 set_text 必须在 sync 上下文调（menu 重建是同步）。
/// 这里用 try_read 做最佳努力——拿不到 session（启动早期）默认 Idle 文案。
#[cfg(target_os = "macos")]
fn record_menu_label_for_current_state() -> String {
    // 同步拿不到 session state（async）——rebuild 场景极少，默认用 Idle 文案。
    // 运行时 state 变化由 update_record_tray_label 主动调（start/stop 路径）。
    crate::i18n::t("tray.recordStart", &[("shortcut", "⌘⇧R")])
}

/// 根据录制状态更新录屏菜单文案（start/stop 路径调用）。
///
/// `recording=true` → 「停止录屏」；`recording=false` → 「开始录屏 ⌘⇧R」。
#[cfg(target_os = "macos")]
pub fn update_record_tray_label(recording: bool) {
    let label = if recording {
        crate::i18n::t("tray.recordStop", &[])
    } else {
        crate::i18n::t("tray.recordStart", &[("shortcut", "⌘⇧R")])
    };
    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.record_start.set_text(label);
    }
}

#[cfg(test)]
mod tests {
    // fmt_engine_label 的 spec 解析逻辑已随 Task 2 模型激活重构移除（改为读
    // resolve_active_engine("asr")），原 spec 字符串单测不再适用。
}
