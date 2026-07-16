//! Run And Paste 全局快捷键——为 auto_paste 菜单项注册全局快捷键，触发 silent 执行链路。
//!
//! 链路：全局热键 → detect_selection → overlay 进度
//!   → execute_action_bar_inner（30s 超时）→ paste
//!
//! silent 模式下 overlay 不获取焦点，源应用始终是前台——
//! detect_selection 的 simulate_copy 不改前台（源应用本来就在前台），
//! paste 直接发给源应用，不需要 ActivateWindowByPid。

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use std::sync::atomic::{AtomicBool, Ordering};

/// silent 执行的取消标志——Esc 或新触发时置 true，worker 线程检查后退出。
static SILENT_CANCELLED: AtomicBool = AtomicBool::new(false);

/// 注册所有 auto_paste 菜单项的全局快捷键。
/// 启动时 + 设置变更后调用。先注销旧的再注册新的。
pub fn register_action_hotkeys(app: &AppHandle) {
    let items = match octopus_infra::db::list_action_hotkeys() {
        Ok(items) => items,
        Err(e) => {
            log::warn!("[action-hotkey] 查询 DB 失败: {}", e);
            return;
        }
    };

    // 先注销所有已注册的 action_hotkey 快捷键（用 DB 里的值精确注销）
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
                // 取消上一次执行（如果仍在进行）
                SILENT_CANCELLED.store(true, Ordering::SeqCst);
                let app_for_worker = app_clone.clone();
                std::thread::spawn(move || {
                    // 短暂等待让上一次 worker 退出，然后重置取消标志
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    SILENT_CANCELLED.store(false, Ordering::SeqCst);
                    silent_run_and_paste(item_id, &app_for_worker);
                });
            }
        }) {
            Ok(()) => log::info!("[action-hotkey] 注册: 「{}」→ {}", item.title, shortcut_str),
            Err(e) => log::warn!("[action-hotkey] 注册失败 「{}」 '{}': {} (可能被系统占用)", item.title, shortcut_str, e),
        }
    }
}

/// 取消当前 silent 执行（Esc 触发 / 新热键触发）。
pub fn cancel_silent() {
    SILENT_CANCELLED.store(true, Ordering::SeqCst);
    log::info!("[action-hotkey] 用户取消 silent 执行");
}

/// silent 执行链路（worker 线程）。
fn silent_run_and_paste(item_id: i64, app: &AppHandle) {
    // detect_selection 内部会写 CHANGE_COUNT_BASELINE（全局静态量），
    // silent 路径与 ActionBar 路径共享此 baseline 会互相污染——
    // silent detect 的 Cmd+C 模拟 + 恢复写入产生的 changeCount 值
    // 会让后续 ActionBar detect 误判有无选中。
    // 解法：detect 前保存 baseline，detect 后恢复，隔离 silent 的副作用。
    let saved_baseline = crate::action_bar_commands::save_change_count_baseline();

    // detect_selection 需要 AppHandle 且内部有 Cmd+C 模拟 + sleep
    log::info!("[action-hotkey][silent] 开始 detect_selection");
    let selection = crate::action_bar_commands::detect_selection(app);

    // 恢复 baseline
    crate::action_bar_commands::restore_change_count_baseline(saved_baseline);

    if SILENT_CANCELLED.load(Ordering::SeqCst) {
        return;
    }

    let text = match &selection {
        crate::action_bar_commands::Selection::Text { text, .. } => text.clone(),
        _ => {
            crate::overlay_window::show_overlay_toast(app, "请先选中文本", "warn", 2000);
            return;
        }
    };

    // 读 DB 取 item（title 用于 overlay）
    let item = match octopus_infra::db::load_action_bar_item(item_id) {
        Ok(Some(i)) => i,
        Ok(None) => {
            crate::overlay_window::show_overlay_toast(app, "菜单项不存在", "error", 3000);
            return;
        }
        Err(e) => {
            crate::overlay_window::show_overlay_toast(app, &format!("读取菜单项失败: {}", e), "error", 3000);
            return;
        }
    };

    // 隐藏 ActionBar（如可见）
    if let Some(win) = app.get_webview_window(crate::action_bar_window::WINDOW_LABEL) {
        if win.is_visible().unwrap_or(false) {
            crate::action_bar_window::hide_action_bar_window(app);
        }
    }

    if SILENT_CANCELLED.load(Ordering::SeqCst) {
        crate::overlay_window::hide_overlay_window(app);
        return;
    }

    // 显示 overlay loading（"正在执行 {title}... · 按 Esc 取消"）
    crate::overlay_window::show_overlay_loading(app, &item.title);

    // 执行动作（复用 execute_action_bar_inner，auto_paste=true 时内部走 run_and_paste）
    let app_clone = app.clone();
    let result = std::thread::spawn(move || -> Result<bool, String> {
        let rt = tokio::runtime::Runtime::new().map_err(|e| format!("Runtime 创建失败: {}", e))?;
        rt.block_on(crate::action_bar_commands::execute_action_bar_inner(item_id, text, &app_clone, true))
    }).join();

    if SILENT_CANCELLED.load(Ordering::SeqCst) {
        crate::overlay_window::hide_overlay_window(app);
        return;
    }

    match result {
        Ok(Ok(true)) => {
            // auto_paste=true 时内部已调 action_bar_run_and_paste（paste 在子线程异步执行）
            // overlay 由 action_bar_run_and_paste 内部隐藏
            log::info!("[action-hotkey] 执行完成");
        }
        Ok(Ok(false)) => {
            // 动作不需要 paste（如 url/async script）
            crate::overlay_window::hide_overlay_window(app);
        }
        Ok(Err(e)) => {
            crate::overlay_window::hide_overlay_window(app);
            crate::overlay_window::show_overlay_toast(app, &e, "error", 3000);
        }
        Err(e) => {
            crate::overlay_window::hide_overlay_window(app);
            crate::overlay_window::show_overlay_toast(app, &format!("执行异常: {:?}", e), "error", 3000);
        }
    }
}
