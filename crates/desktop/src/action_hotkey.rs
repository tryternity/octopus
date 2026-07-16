//! Run Quickly 全局快捷键——为配置了 global_shortcut 的菜单项注册全局快捷键，跳过 ActionBar 浮窗直接执行。
//!
//! 与 ActionBar 路径的区别：全局快捷键省去"弹出浮窗 + 手动选菜单"两步。
//! 结果仍展示在 CompactEditor（与 ActionBar 路径完全一致），不直接粘贴替换。
//!
//! 链路：全局热键 → detect_selection → execute_action_bar_inner → CompactEditor

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 注册所有配置了 global_shortcut 的菜单项的全局快捷键。
/// 启动时 + 设置变更后调用。
///
/// ⚠️ **必须先 unregister_all 清空再全量重注册**：原先只遍历「当前 DB 中非空快捷键」来
/// unregister，删除/清空场景下（DB 已是 ''，不在结果集里）旧 handler 永远残留在进程内，
/// 直到下次重启。曾导致：用户把菜单项快捷键从 `Cmd+Shift+G` 删除后，按键仍被 octopus 吞。
pub fn register_action_hotkeys(app: &AppHandle) {
    let items = match octopus_infra::db::list_action_hotkeys() {
        Ok(items) => items,
        Err(e) => {
            log::warn!("[action-hotkey] 查询 DB 失败: {}", e);
            return;
        }
    };

    // 全量清空所有菜单项快捷键（含已从 DB 删除但仍在进程内残留的），
    // action_bar / clipboard / asr / edit / polish / screenshot 各自独立注册的快捷键
    // 不受影响（它们走各自的 register_*_shortcut 路径）。
    let _ = app.global_shortcut().unregister_all();

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
        other => {
            // 无文本选中（如 Finder 选中文件夹、桌面空选）——菜单项热键的语义是
            // 「对这段文本执行动作」，没文本就不该继续。原先 fallback 到 ActionBar 浮窗，
            // 会劫持系统快捷键（Finder 的 Cmd+Shift+G「前往文件夹」被吞）并误导用户。
            // 静默失败即可，留给用户在 ActionBar 浮窗主动弹出时再处理。
            log::info!(
                "[action-hotkey] 非文本选中 ({})，跳过菜单项 item_id={} 执行",
                selection_kind_name(other),
                item_id,
            );
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

/// 用于日志输出——Selection 未 derive Debug，手工映射为可读字符串。
fn selection_kind_name(sel: &crate::action_bar_commands::Selection) -> &'static str {
    use crate::action_bar_commands::Selection;
    match sel {
        Selection::None => "None",
        Selection::Text { .. } => "Text",
        Selection::File { .. } => "File",
        Selection::Folder { .. } => "Folder",
    }
}
