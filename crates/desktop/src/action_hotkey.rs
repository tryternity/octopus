//! Run Quickly 全局快捷键——为配置了 global_shortcut 的菜单项注册全局快捷键，跳过 ActionBar 浮窗直接执行。
//!
//! 与 ActionBar 路径的区别：全局快捷键省去"弹出浮窗 + 手动选菜单"两步。
//! 结果仍展示在 CompactEditor（与 ActionBar 路径完全一致），不直接粘贴替换。
//!
//! 链路：全局热键 → detect_selection → execute_action_bar_inner → CompactEditor

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 注册所有配置了 global_shortcut 的菜单项的全局快捷键。
/// 启动时 + 设置变更后调用。先注销旧的再注册新的。
pub fn register_action_hotkeys(app: &AppHandle) {
    let items = match octopus_infra::db::list_action_hotkeys() {
        Ok(items) => items,
        Err(e) => {
            log::warn!("[action-hotkey] 查询 DB 失败: {}", e);
            return;
        }
    };

    // 先注销所有已注册的快捷键
    for item in &items {
        if let Ok(sc) = item.global_shortcut.parse::<Shortcut>() {
            let _ = app.global_shortcut().unregister(sc);
        }
    }

    // 重新注册
    for item in &items {
        let shortcut_str = item.global_shortcut.clone();
        let shortcut: Shortcut = match shortcut_str.parse() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("[action-hotkey] 「{}」快捷键解析失败 '{}': {}", item.title, shortcut_str, e);
                continue;
            }
        };

        let item_id = item.id;
        let app_clone = app.clone();
        match app.global_shortcut().on_shortcut(shortcut, move |_app, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                log::info!("[action-hotkey] 触发 item_id={}", item_id);
                let app_for_worker = app_clone.clone();
                std::thread::spawn(move || {
                    quick_execute(item_id, &app_for_worker);
                });
            }
        }) {
            Ok(()) => log::info!("[action-hotkey] 注册: 「{}」→ {}", item.title, shortcut_str),
            Err(e) => log::warn!("[action-hotkey] 注册失败 「{}」 '{}': {} (可能被系统占用)", item.title, shortcut_str, e),
        }
    }
}

/// 快速执行链路（worker 线程）：detect → execute → CompactEditor。
/// 不弹出 ActionBar 浮窗，不直接粘贴——结果展示在 CompactEditor（与 ActionBar 路径一致）。
fn quick_execute(item_id: i64, app: &AppHandle) {
    // baseline 隔离——detect 写 CHANGE_COUNT_BASELINE，恢复原值不污染 ActionBar 路径
    let saved_baseline = crate::action_bar_commands::save_change_count_baseline();

    log::info!("[action-hotkey] 开始 detect_selection");
    let selection = crate::action_bar_commands::detect_selection(app);

    crate::action_bar_commands::restore_change_count_baseline(saved_baseline);

    let text = match &selection {
        crate::action_bar_commands::Selection::Text { text, .. } => text.clone(),
        _ => {
            // 无选中——用 ActionBar 浮窗 fallback（让用户看到搜索框）
            log::info!("[action-hotkey] 无选中，fallback 到 ActionBar 浮窗");
            crate::action_bar_commands::trigger_action_bar(app.clone());
            return;
        }
    };

    // 隐藏 ActionBar（如可见）
    if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
        if win.is_visible().unwrap_or(false) {
            crate::action_bar_window::hide_action_bar_window(app);
        }
    }

    // 执行动作——非 silent 模式（is_silent=false），走正常 CompactEditor 展示
    log::info!("[action-hotkey] 执行 item_id={}, text len={}", item_id, text.len());
    let app_clone = app.clone();
    let result = std::thread::spawn(move || -> Result<bool, String> {
        let rt = tokio::runtime::Runtime::new().map_err(|e| format!("Runtime 创建失败: {}", e))?;
        rt.block_on(crate::action_bar_commands::execute_action_bar_inner(item_id, text, &app_clone))
    }).join();

    match result {
        Ok(Ok(true)) => log::info!("[action-hotkey] 执行完成（结果已在 CompactEditor 展示）"),
        Ok(Ok(false)) => log::info!("[action-hotkey] 执行完成（无需展示）"),
        Ok(Err(e)) => log::warn!("[action-hotkey] 执行失败: {}", e),
        Err(e) => log::warn!("[action-hotkey] 执行线程异常: {:?}", e),
    }
}
