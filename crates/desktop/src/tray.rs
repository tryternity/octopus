// src/tray.rs

use crate::config::AppConfig;
use log::info;
use parking_lot::Mutex;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
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
    // 麦克风子菜单（2026-07-29）：父项 + 设备列表。切换时重建 checkmark + 父项文案。
    mic_submenu: Submenu<R>,
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
/// 存储 record_shortcut 用于录屏菜单动态文案。
/// 用 Mutex（非 OnceLock）——用户在 Settings 改快捷键后立即更新 tray 文案。
#[cfg(target_os = "macos")]
static RECORD_SHORTCUT: parking_lot::Mutex<String> = parking_lot::Mutex::new(String::new());

/// 返回 RECORD_SHORTCUT 的 lock guard，让 settings_commands 在热重载成功后更新镜像。
#[cfg(target_os = "macos")]
pub fn record_shortcut_mirror() -> parking_lot::MutexGuard<'static, String> {
    RECORD_SHORTCUT.lock()
}

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

// ── 麦克风快捷选择子菜单（2026-07-29）──
//
// 在「语音识别」与「引擎信息」之间插入 Submenu，父项显示当前麦克风，
// 子项列出所有设备（+「默认设备」项），点击切换 microphone 配置。
// 切换逻辑复用 settings_commands::set_config（保证持久化与设置页一致）。
//
// 设备项 id 约定：
//   "mic:default"     → 默认设备（microphone 清空为 ""）
//   "mic:{device_name}" → 具体设备（microphone = device_name）

/// 麦克风子菜单 id 前缀。
const MIC_ITEM_PREFIX: &str = "mic:";
/// 「默认设备」项的 id（对应 microphone 配置为空串）。
const MIC_DEFAULT_ID: &str = "mic:default";

/// 枚举系统麦克风设备名（cpal）。复用 settings_commands 的同款逻辑。
/// 返回排序后的设备名列表（可能为空——无麦克风或权限未授予）。
/// 枚举系统麦克风设备名（cpal）。**只能在后台线程调用**——cpal 首次调用会同步
/// 初始化 CoreAudio 子系统，阻塞主线程会导致同时初始化的 WKWebView 内容进程
/// 超时被 macOS 终止（web content process terminated）。
/// 返回排序后的设备名列表（可能为空——无麦克风或权限未授予）。
fn list_microphone_devices() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => {
            let mut mics: Vec<String> = devices.filter_map(|d| d.name().ok()).collect();
            mics.sort();
            mics
        }
        Err(_) => Vec::new(),
    }
}

/// 构建麦克风子菜单父项文案：「麦克风: <当前>」。
/// 当前为空串时显示「默认设备」。
fn fmt_microphone_parent_text(current: &str) -> String {
    let display = if current.is_empty() {
        crate::i18n::t("tray.microphoneDefault", &[])
    } else {
        current.to_string()
    };
    format!("{}: {}", crate::i18n::t("tray.microphone", &[]), display)
}

/// 构建麦克风子菜单：父项 + 「默认设备」项。
///
/// **不在启动时枚举设备**——cpal `input_devices()` 首次调用会同步初始化 CoreAudio，
/// 阻塞主线程导致同时初始化的 WKWebView 内容进程超时被杀（web content process terminated）。
/// 设备项由 `preheat_microphone_submenu` 在后台线程枚举后异步填充。
fn build_microphone_submenu(
    app: &tauri::AppHandle,
    current_mic: &str,
) -> Result<Submenu<tauri::Wry>, String> {
    // 「默认设备」项（始终在最前）。
    let default_item = CheckMenuItem::with_id(
        app,
        MIC_DEFAULT_ID,
        &crate::i18n::t("tray.microphoneDefault", &[]),
        true,
        current_mic.is_empty(),
        None::<&str>,
    )
    .map_err(|e| format!("mic default item: {e}"))?;

    Submenu::with_items(
        app,
        fmt_microphone_parent_text(current_mic),
        true,
        &[&default_item],
    )
    .map_err(|e| format!("mic submenu: {e}"))
}

/// 后台枚举麦克风设备，完成后回主线程把设备项 append 到子菜单。
///
/// 在 `create_tray` 返回后由 main.rs 调用（spawn 后台线程），避免阻塞启动主线程。
/// 已存在的设备项不重复添加（按 id 去重）。当前选中项打勾。
pub fn preheat_microphone_submenu(app: &tauri::AppHandle, current_mic: &str) {
    let app_handle = app.clone();
    let current = current_mic.to_string();
    std::thread::spawn(move || {
        // 后台线程枚举设备（cpal CoreAudio 初始化在此线程，不阻塞主线程）。
        let devices = list_microphone_devices();
        if devices.is_empty() {
            return;
        }
        // 回主线程 append 菜单项（tauri menu 必须主线程操作）。
        let app2 = app_handle.clone();
        let current2 = current.clone();
        let _ = app_handle.run_on_main_thread(move || {
            append_microphone_devices(&app2, &devices, &current2);
        });
    });
}

/// 把枚举到的设备项 append 到麦克风子菜单（主线程调用）。
/// 已存在的设备项跳过（按 id 去重，防重复 preheat）。
fn append_microphone_devices(app: &tauri::AppHandle, devices: &[String], current_mic: &str) {
    let items = TRAY_ITEMS.lock();
    let Some(tray_items) = items.as_ref() else { return };

    // 收集已存在的设备 id，去重。
    let existing: std::collections::HashSet<String> = tray_items
        .mic_submenu
        .items()
        .unwrap_or_default()
        .iter()
        .map(|i| i.id().as_ref().to_string())
        .collect();

    for name in devices {
        let id = format!("{}{}", MIC_ITEM_PREFIX, name);
        if existing.contains(&id) {
            continue;
        }
        let item = match CheckMenuItem::with_id(
            app,
            &id,
            name,
            true,
            name == current_mic,
            None::<&str>,
        ) {
            Ok(item) => item,
            Err(_) => continue,
        };
        if let Err(e) = tray_items.mic_submenu.append(&item) {
            log::warn!("[mic-submenu] append 设备项失败 {:?}: {}", name, e);
        }
    }
}

/// Create the system tray icon and its context menu.
///
/// 菜单文案设计：操作项统一四字宽度 + 括号快捷键。
/// 分组：设置（顶部，高频入口）→ 语音识别 → 引擎信息（只读分隔线）→ 截图/剪贴板 → 退出。
/// 系统设置放第一位（2026-07-29 用户决策）：作为最高频入口，与下方功能组用分隔线隔开。
pub fn create_tray(app: &tauri::AppHandle, config: &AppConfig) -> Result<(), String> {
    let _ = ASR_SHORTCUT.set(config.asr_shortcut.clone());
    #[cfg(target_os = "macos")]
    {
        *RECORD_SHORTCUT.lock() = config.record_shortcut.clone();
    }
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

    // 麦克风快捷选择子菜单（2026-07-29）：语音识别与引擎信息之间。
    // 父项显示当前麦克风，子项列出设备 + 默认设备，点击切换。
    let mic_submenu = build_microphone_submenu(app, &config.microphone)
        .map_err(|e| format!("microphone submenu: {e}"))?;

    let sep1 = PredefinedMenuItem::separator(app)
        .map_err(|e| format!("separator: {e}"))?;

    let screenshot_text = crate::i18n::t("tray.screenshot", &[("shortcut", &fmt_shortcut(&config.screenshot_shortcut))]);
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
    // 录屏菜单项文案：从 RECORD_SHORTCUT 读用户配置的快捷键（与 ASR 同模式）。
    // 用户改快捷键后 set_config 热重载 + update_record_tray_label 同步文案。
    #[cfg(target_os = "macos")]
    let record_sc_display = fmt_shortcut(
        &RECORD_SHORTCUT.lock().clone()
    );
    #[cfg(target_os = "macos")]
    let sep_record = PredefinedMenuItem::separator(app)
        .map_err(|e| format!("separator_record: {e}"))?;
    #[cfg(target_os = "macos")]
    let record_start = MenuItem::with_id(
        app,
        "record_start",
        &crate::i18n::t("tray.recordStart", &[("shortcut", &record_sc_display)]),
        true,
        None::<&str>,
    )
    .map_err(|e| format!("record_start menu: {e}"))?;

    let sep2 = PredefinedMenuItem::separator(app)
        .map_err(|e| format!("separator2: {e}"))?;

    let settings = MenuItem::with_id(app, "settings", &crate::i18n::t("tray.settings", &[]), true, None::<&str>)
        .map_err(|e| format!("settings menu: {e}"))?;
    // 系统设置（顶部）与下方功能组之间的分隔线（2026-07-29）。
    let sep_settings = PredefinedMenuItem::separator(app)
        .map_err(|e| format!("separator_settings: {e}"))?;
    let quit = MenuItem::with_id(app, "quit", &crate::i18n::t("tray.quit", &[]), true, None::<&str>)
        .map_err(|e| format!("quit menu: {e}"))?;

    // 菜单组装（用户决策 2026-07-25；2026-07-29 调整 settings 位置 + 加麦克风子菜单）：
    // 顶部：设置（高频入口，单独一组）
    // 分组 1：语音识别 + 麦克风子菜单 + 引擎信息
    // 分组 2：截图 + 录屏（屏幕采集类，仅 macOS）
    // 分组 3：剪贴板 + 图文编辑
    // 底部：退出
    #[cfg(target_os = "macos")]
    let menu = Menu::with_items(app, &[
        &settings, &sep_settings,
        &toggle, &mic_submenu, &engine_info, &sep1,
        &screenshot, &record_start, &sep_record,
        &clipboard, &compact_editor,
        &sep2,
        &quit,
    ])
    .map_err(|e| format!("tray menu: {e}"))?;
    #[cfg(not(target_os = "macos"))]
    let menu = Menu::with_items(app, &[
        &settings, &sep_settings,
        &toggle, &mic_submenu, &engine_info, &sep1,
        &screenshot, &clipboard, &compact_editor, &sep2,
        &quit,
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
                mic_submenu: mic_submenu.clone(),
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
                mic_submenu: mic_submenu.clone(),
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
                let _ = crate::clipboard::clipboard_window::toggle_clipboard_window(app);
            }
            "compact_editor" => {
                info!("Tray: open compact editor (empty)");
                crate::commands::compact_editor_commands::open_temp_compact_editor(app, &Default::default());
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
                            crate::record::record_window::show_record_window(&app_handle);
                        }
                        SessionState::Recording | SessionState::Paused => {
                            // 停止 + 入库（与 hotkey Esc 同路径）
                            match crate::record::record_commands::stop_and_store(&session, &app_handle, false, None).await {
                                Ok(Some(meta)) => {
                                    log::info!(
                                        "[tray] 录制已停止入库: id={} file={}",
                                        meta.id,
                                        meta.file_path
                                    );
                                    // 关闭标注 overlay（Source::Area 录制时才有）
                                    crate::record::record_annotation_window::close_annotation_window(&app_handle);
                                    // 关闭录制控制浮窗 pill（display/window 录制时才有；与 ESC/stop-requested/kill 路径一致）
                                    crate::record::record_control_window::close_control_window(&app_handle);
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
                    let _ = crate::record::screenshot_commands::start_screenshot(app_handle).await;
                });
            }
            "quit" => {
                info!("Tray: quit");
                app.exit(0);
            }
            // 麦克风快捷选择（2026-07-29）：子菜单设备项点击。
            // id = "mic:default"（清空为系统默认）或 "mic:{device_name}"。
            // 复用 set_config 持久化 microphone 配置，再重建子菜单 checkmark + 父项文案。
            id if id.starts_with(MIC_ITEM_PREFIX) => {
                let device = if id == MIC_DEFAULT_ID {
                    String::new() // 默认设备 = 空串
                } else {
                    id[MIC_ITEM_PREFIX.len()..].to_string()
                };
                info!("Tray: switch microphone to {:?}", if device.is_empty() { "(default)" } else { &device });
                // 复用 set_config 持久化（保证与设置页一致：写 DB + 更新 runtime config）。
                // set_config 需要 State，tray 闭包里直接走 DB 写入 + runtime config 更新的等价路径。
                switch_microphone_from_tray(app, device);
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

/// 语言切换后重建所有菜单项文案。
///
/// 传入当前 `AppConfig`——所有快捷键文案从 config 读，不依赖全局 mirror。
/// （曾因复用 ASR_SHORTCUT 全局变量导致截图/剪贴板菜单显示成 ASR 的快捷键——2026-07-28 修复）
pub fn rebuild_tray_labels(config: &AppConfig) {
    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let asr_sc = fmt_shortcut(&config.asr_shortcut);
        let screenshot_sc = fmt_shortcut(&config.screenshot_shortcut);
        let clipboard_sc = fmt_shortcut(&config.clipboard_shortcut);
        let _ = tray_items.toggle.set_text(crate::i18n::t("tray.startAsr", &[("shortcut", &asr_sc)]));
        let _ = tray_items.screenshot.set_text(crate::i18n::t("tray.screenshot", &[("shortcut", &screenshot_sc)]));
        let _ = tray_items.clipboard.set_text(crate::i18n::t("tray.clipboard", &[("shortcut", &clipboard_sc)]));
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
        // 麦克风子菜单父项文案（子项设备名不随语言变；语言切换只更新父项前缀文案）。
        let _ = tray_items.mic_submenu.set_text(fmt_microphone_parent_text(&config.microphone));
    }
}

/// 录屏菜单文案：根据当前 RecordSession state 切换。
///
/// - Idle/Starting → 「开始录屏 ⌘⇧R」（弹浮窗选源）
/// - Recording/Paused/Stopping → 「停止录屏  ⎋」（停止 + 入库，提示 ESC 快捷键）
///
/// 快捷键符号从 RECORD_SHORTCUT 读（用户可配置），ESC 固定（全局通用停止键）。
#[cfg(target_os = "macos")]
fn record_menu_label_for_current_state() -> String {
    // 同步拿不到 session state（async）——rebuild 场景极少，默认用 Idle 文案。
    // 运行时 state 变化由 update_record_tray_label 主动调（start/stop 路径）。
    let sc = fmt_shortcut(&RECORD_SHORTCUT.lock().clone());
    crate::i18n::t("tray.recordStart", &[("shortcut", &sc)])
}

/// 根据录制状态更新录屏菜单文案（start/stop 路径调用）。
///
/// `recording=true` → 「停止录屏  ⎋」（停止 + 入库，提示 ESC）
/// `recording=false` → 「开始录屏 ⌘⇧R」（用户配置的 toggle 快捷键）
#[cfg(target_os = "macos")]
pub fn update_record_tray_label(recording: bool) {
    let label = if recording {
        // 录制中：红点前缀（●）+ 停止文案。
        // ● 是 U+25CF，macOS menu 渲染为当前文本色（深色菜单栏=白，浅色=黑），
        // 不是真红色——真红点需替换 tray icon PNG（P1-3 后续，待用户提供图标）。
        // 现版本：文本红点作为菜单栏视觉提示（P1-7 浮窗已有红点动画，tray 是补充）。
        format!("● {}", crate::i18n::t("tray.recordStop", &[("shortcut", "⎋")]))
    } else {
        let sc = fmt_shortcut(&RECORD_SHORTCUT.lock().clone());
        crate::i18n::t("tray.recordStart", &[("shortcut", &sc)])
    };
    let items = TRAY_ITEMS.lock();
    if let Some(tray_items) = items.as_ref() {
        let _ = tray_items.record_start.set_text(label);
    }
}

// ── 麦克风子菜单切换与更新（2026-07-29）──

/// 托盘切换麦克风：持久化 microphone 配置 + 更新子菜单 checkmark + 父项文案。
///
/// 复用 set_config 的持久化路径（写 DB + 更新 runtime config），保证与设置页一致。
/// 不重启 audio stream——下次录音 build_stream 时用新设备名（与设置页行为一致）。
fn switch_microphone_from_tray(app: &tauri::AppHandle, device: String) {
    use crate::runtime_config::SharedRuntimeConfig;
    use tauri::Manager;

    // 1. 更新 runtime config + 持久化 DB（复用 set_config 的等价逻辑）。
    if let Some(rc) = app.try_state::<SharedRuntimeConfig>() {
        let cfg = {
            let g = rc.read();
            let mut c = g.clone();
            c.microphone = device.clone();
            c
        };
        if octopus_infra::db::save_app_config(&cfg).is_ok() {
            let mut g = rc.write();
            *g = cfg;
        }
    }

    // 2. 更新子菜单 checkmark + 父项文案。
    update_microphone_submenu(&device);
}

/// 按当前选中设备更新麦克风子菜单：
/// - 父项文案改为「麦克风: <current>」
/// - 遍历子项，匹配 id 的设 checked=true，其余 false
fn update_microphone_submenu(current: &str) {
    let items = TRAY_ITEMS.lock();
    let Some(tray_items) = items.as_ref() else { return };
    // 父项文案
    let _ = tray_items.mic_submenu.set_text(fmt_microphone_parent_text(current));
    // 子项 checkmark：匹配选中 id 的勾选，其余取消。
    // 选中 id：空串 → mic:default；否则 mic:{device}
    let selected_id = if current.is_empty() {
        MIC_DEFAULT_ID.to_string()
    } else {
        format!("{}{}", MIC_ITEM_PREFIX, current)
    };
    if let Ok(children) = tray_items.mic_submenu.items() {
        for child in &children {
            if let Some(check_item) = child.as_check_menuitem() {
                let _ = check_item.set_checked(child.id().as_ref() == selected_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // fmt_engine_label 的 spec 解析逻辑已随 Task 2 模型激活重构移除（改为读
    // resolve_active_engine("asr")），原 spec 字符串单测不再适用。
}
