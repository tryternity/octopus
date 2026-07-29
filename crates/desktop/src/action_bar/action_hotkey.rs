//! Run Quickly 全局快捷键——为配置了 global_shortcut 的菜单项注册全局快捷键，跳过 ActionBar 浮窗直接执行。
//!
//! 与 ActionBar 路径的区别：全局快捷键省去"弹出浮窗 + 手动选菜单"两步。
//! 结果仍展示在 CompactEditor（与 ActionBar 路径完全一致），不直接粘贴替换。
//!
//! 链路：全局热键 → detect_selection → execute_action_bar_inner → CompactEditor

use std::collections::HashSet;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 本模块已注册的快捷键字符串集合（菜单项 Quick Execute 快捷键）。
///
/// **必须维护这份清单**——`unregister_all()` 会清掉整个 global_shortcut plugin
/// 持有的所有快捷键，包括 asr / clipboard / edit_global / polish_global /
/// screenshot / action_bar 等其他模块注册的。曾因 `register_action_hotkeys`
/// 用 `unregister_all()` 导致启动时其他 5 个快捷键全失效（启动顺序：其他先注册 →
/// 本函数清空 → 只有 action_bar_shortcut 在后面重新注册成功）。
///
/// 改为「按清单精确 unregister」：本模块注册成功 → 加入清单；重建时遍历清单
/// 逐个 unregister + 清单重置。DB 里已删的快捷键（结果集不含）只要曾在清单里就能
/// 被精确注销，覆盖「用户删除菜单项快捷键后旧 handler 残留」的根因场景。
static REGISTERED_SHORTCUTS: once_cell::sync::Lazy<Mutex<HashSet<String>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashSet::new()));

/// 注册所有配置了 global_shortcut 的菜单项的全局快捷键。
/// 启动时 + 设置变更后调用。
///
/// 重建语义：先按 `REGISTERED_SHORTCUTS` 清单精确注销本模块注册过的所有快捷键
/// （含 DB 里已删除但仍残留的），再按当前 DB 全量重注册。**仅清本模块的**，
/// 不影响其他模块注册的快捷键（asr/clipboard/edit_global/polish_global/screenshot/action_bar）。
pub fn register_action_hotkeys(app: &AppHandle) {
    let items = match octopus_infra::db::list_action_hotkeys() {
        Ok(items) => items,
        Err(e) => {
            log::warn!("[action-hotkey] 查询 DB 失败: {}", e);
            return;
        }
    };

    // 按清单精确注销本模块已注册的快捷键——绝不用 unregister_all（会误清其他模块）
    let to_unregister: Vec<String> = {
        let mut set = REGISTERED_SHORTCUTS.lock().unwrap();
        set.drain().collect()
    };
    for sc_str in &to_unregister {
        if let Ok(sc) = sc_str.parse::<Shortcut>() {
            let _ = app.global_shortcut().unregister(sc);
        }
    }

    // 重新注册 + 记录到清单
    let mut new_registered: HashSet<String> = HashSet::new();
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
            Ok(()) => {
                log::info!("[action-hotkey] 注册: 「{}」→ {}", item.title, shortcut_str);
                new_registered.insert(shortcut_str);
            }
            Err(e) => log::warn!("[action-hotkey] 注册失败 「{}」 '{}': {} (可能被系统占用)", item.title, shortcut_str, e),
        }
    }
    *REGISTERED_SHORTCUTS.lock().unwrap() = new_registered;
}

/// 快速执行链路（worker 线程）：detect → 路由到 Text/Files/None 分支。
///
/// 与 ActionBar 路径的区别：不弹出 ActionBar 浮窗，不直接粘贴。
/// - **Text 选中** → `handle_text_selection`（沿用原 quick_execute 全部行为：gather
///   context、刷新 PENDING_CONTEXT、activate_self、keep_active hide、execute → CompactEditor）。
/// - **File/Folder 选中** → `handle_files_selection`（新：写 PENDING_CONTEXT 后按
///   `decide_files_action` 决策走 `trigger_agent_voice_core` 或 `execute_action_bar_inner`）。
/// - **无选中** → 静默跳过（不劫持系统快捷键）。
fn quick_execute(item_id: i64, app: &AppHandle) {
    // baseline 隔离——detect 写 CHANGE_COUNT_BASELINE，恢复原值不污染 ActionBar 路径
    let saved_baseline = crate::action_bar::action_bar_commands::save_change_count_baseline();

    log::info!("[action-hotkey] 开始 detect_selection");
    let selection = crate::action_bar::action_bar_commands::detect_selection(app);

    crate::action_bar::action_bar_commands::restore_change_count_baseline(saved_baseline);

    match selection {
        crate::action_bar::action_bar_commands::Selection::Text { text, .. } => {
            handle_text_selection(item_id, app, text);
        }
        crate::action_bar::action_bar_commands::Selection::File { files, .. }
        | crate::action_bar::action_bar_commands::Selection::Folder { folders: files, .. } => {
            handle_files_selection(item_id, app, files);
        }
        crate::action_bar::action_bar_commands::Selection::None => {
            // 无选中（桌面空选、菜单栏点击等）——菜单项热键的语义是「对选中内容执行动作」，
            // 无选中就不该继续。静默跳过即可。
            log::info!("[action-hotkey] 无选中，跳过 item_id={}", item_id);
        }
    }
}

/// Text 选中分支——从原 quick_execute 提取（行为保持不变）。
///
/// 链路：刷新 PENDING_CONTEXT（含 gather_context）→ activate_self 夺焦 →
/// keep_active hide ActionBar（如可见）→ execute_action_bar_inner → CompactEditor。
fn handle_text_selection(item_id: i64, app: &AppHandle, text: String) {
    // ── 刷新 PENDING_CONTEXT（发现 2 修复）──
    // quick_execute 此前不写 PENDING_CONTEXT，会读到上次 trigger_action_bar 残留的
    // source/surrounding（甚至更早的 quick_execute 残留）。AI 动作（润色/摘要/解释）经
    // build_enriched_text 读这些字段 → 来源/上下文与当前选中文本错位。
    // 与 trigger_action_bar 的 Text 分支对齐：先清空，再 gather 写新值。
    // 失败时降级到仅 text（source/surrounding=None），build_enriched_text 会跳过拼接。
    //
    // 注意：只在 Text 分支调（File/Folder 走 trigger 路径由 trigger_action_bar 负责），
    // 且 gather 内部已含 run_command_with_deadline 兜底（osascript/lsof 等 500ms 超时）。
    let mut ctx = crate::action_bar::action_bar_commands::ActionBarContext::text(text.clone());
    match crate::app_context::gather_context(&text) {
        Ok(extra) => {
            ctx.source = Some(extra.source);
            ctx.surrounding = extra.surrounding;
        }
        Err(e) => log::warn!("[action-hotkey] context gather 失败（降级到仅 text）: {}", e),
    }
    crate::action_bar::action_bar_commands::set_pending_context(ctx);

    // gather_context 内部 subl --command / osascript 会激活源 app（trigger_action_bar
    // 的注释明确指出这点）。trigger 路径靠随后的 ActionBar show + set_focus 夺回焦点，
    // 但 quick_execute 不 show ActionBar——gather 完成后 app 可能仍是后台，紧接着打开
    // CompactEditor（set_focus 不激活后台 app）→ 用户看不到结果。
    // R3 修复（2026-07-17）：投递主线程 activate_self 把本 app 带回前台，
    // 让随后的 CompactEditor show+set_focus 能正确夺焦。activate_self 必须主线程执行
    // （NSApplication::sharedApplication 要求），worker 线程直接调会被跳过。
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(|| {
            crate::activation::activate_self();
        });
    }

    // 隐藏 ActionBar（如可见）——发现 3 修复：用 keep_active 变体，对齐
    // action_bar_show_result_internal（action_bar_commands.rs:445-452）的三步：
    //   win.hide() + after_floating_window_hide_keep_active + finalize_action_bar
    // 原先用 hide_action_bar_window（标准 variant），was_inactive=true 时
    // activateWithOptions(prev_app) 把源 app 拉回前台、本 app 退后台，
    // 紧接着打开 CompactEditor 时后台 app 的 set_focus 不激活 → 用户看不到结果。
    if let Some(win) = app.get_webview_window(crate::action_bar::action_bar_window::WINDOW_LABEL) {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
            #[cfg(target_os = "macos")]
            { crate::activation::after_floating_window_hide_keep_active(app); }
            crate::action_bar::action_bar_commands::finalize_action_bar_pub(app);
        }
    }

    // 执行动作——非 silent 模式（is_silent=false），走正常 CompactEditor 展示
    log::info!("[action-hotkey] 执行 item_id={}, text len={}", item_id, text.len());
    let app_clone = app.clone();
    let result = std::thread::spawn(move || -> Result<bool, String> {
        let rt = tokio::runtime::Runtime::new().map_err(|e| format!("Runtime 创建失败: {}", e))?;
        rt.block_on(crate::action_bar::action_bar_commands::execute_action_bar_inner(item_id, text, &app_clone))
    }).join();

    match result {
        Ok(Ok(true)) => log::info!("[action-hotkey] 执行完成（结果已在 CompactEditor 展示）"),
        Ok(Ok(false)) => log::info!("[action-hotkey] 执行完成（无需展示）"),
        Ok(Err(e)) => log::warn!("[action-hotkey] 执行失败: {}", e),
        Err(e) => log::warn!("[action-hotkey] 执行线程异常: {:?}", e),
    }
}

/// File/Folder 选中分支——agent × Files × 语音流程入口。
///
/// 语义：Finder 选中文件/夹后按全局快捷键，把 `files` 写入 PENDING_CONTEXT，
/// 然后按菜单项类型决策：
/// - **agent + 含 `{{voice}}`** → `trigger_agent_voice_core(hide_action_bar=false)`：
///   写 agent_task → 触发音录。`hide_action_bar=false` 是因为 quick_execute 路径
///   ActionBar 本就没显示（全局快捷键直触发，省去浮窗），不应再 hide 不可见窗口
///   （hide 会触发 activateWithOptions 把源 app 拉到前台，干扰随后录音 UI）。
/// - **其他（script/url/copy_path/agent-without-task）** → `execute_action_bar_inner`：
///   用 PENDING_CONTEXT.files 渲染 `{{files}}` 后执行，结果展示在 CompactEditor。
fn handle_files_selection(item_id: i64, app: &AppHandle, files: Vec<String>) {
    // 1. 写 PENDING_CONTEXT (kind=Files)——execute_action_bar_inner 和 trigger_agent_voice_core 都从这里读 files
    let ctx = crate::action_bar::action_bar_commands::ActionBarContext::files(files.clone());
    crate::action_bar::action_bar_commands::set_pending_context(ctx);

    // 2. 查 item 决定路径
    let item = match octopus_infra::db::load_action_bar_item(item_id) {
        Ok(Some(it)) => it,
        Ok(None) => {
            log::warn!("[action-hotkey] File 选中但 item_id={} 不存在", item_id);
            return;
        }
        Err(e) => {
            log::warn!("[action-hotkey] File 选中但查 item 失败: {}", e);
            return;
        }
    };

    // 3. 决策路径——纯函数 decide_files_action（便于单测）
    let (should_trigger_voice, should_execute_directly) =
        decide_files_action(&item.action_type, item.need_voice);

    if should_trigger_voice {
        // agent + need_voice → 走音录路径
        log::info!(
            "[action-hotkey] File 选中 + agent + need_voice → 触发音录 item_id={}, files={}",
            item_id,
            files.len(),
        );
        let coordinator = match app.try_state::<crate::coordinator::Coordinator>() {
            Some(c) => c,
            None => {
                log::error!("[action-hotkey] Coordinator state 未找到（无法启动音录）");
                return;
            }
        };
        // hide_action_bar=false：quick_execute 路径 ActionBar 未显示，不 hide
        if let Err(e) = crate::action_bar::action_bar_commands::trigger_agent_voice_core(
            &item,
            app,
            coordinator.inner(),
            false,
        ) {
            log::error!("[action-hotkey] trigger_agent_voice_core 失败: {}", e);
        }
        return;
    }

    if should_execute_directly {
        // 非 agent 或无 {{voice}} → 直接执行（prompt 用 {{files}} 渲染）
        log::info!(
            "[action-hotkey] File 选中 + 直接执行 item_id={}, files={}",
            item_id,
            files.len(),
        );
        let app_clone = app.clone();
        let result = std::thread::spawn(move || -> Result<bool, String> {
            let rt = tokio::runtime::Runtime::new().map_err(|e| format!("Runtime 创建失败: {}", e))?;
            // text 传空：File 场景不需要文本，execute_action_bar_inner 会从 PENDING_CONTEXT 读 files
            rt.block_on(crate::action_bar::action_bar_commands::execute_action_bar_inner(
                item_id,
                String::new(),
                &app_clone,
            ))
        }).join();

        match result {
            Ok(Ok(true)) => log::info!("[action-hotkey] File 执行完成（结果已在 CompactEditor 展示）"),
            Ok(Ok(false)) => log::info!("[action-hotkey] File 执行完成（无需展示）"),
            Ok(Err(e)) => log::warn!("[action-hotkey] File 执行失败: {}", e),
            Err(e) => log::warn!("[action-hotkey] File 执行线程异常: {:?}", e),
        }
    }
}

/// 决策 File/Folder 选中时的执行路径——纯函数，便于单测。
///
/// 输入：菜单项的 `action_type` 与 `action_data`（prompt 模板）。
/// 输出：`(should_trigger_voice, should_execute_directly)`：
/// - `(true, false)` → 走 `trigger_agent_voice_core`（agent + 含 `{{voice}}` → 需要语音录入 task）
/// - `(false, true)` → 走 `execute_action_bar_inner`（script/url/copy_path/agent-without-task）
/// - `(false, false)` → 静默跳过（理论不出现：所有非 voice 路径都走 direct）
///
/// 语义：只有 `action_type=agent` 且 `need_voice=true` 才需要语音，
/// 因为用户在设置面板勾选了「需要语音输入」（agent 菜单的 prompt 含 {{voice}}）。
/// 其他情况（agent 但 need_voice=false、script、url、copy_path 等）直接渲染执行即可。
///
/// 2026-07-19 v40 改：判定从「action_data 含 {{voice}}」改为「need_voice 字段」，
/// 避免扫描 prompt 字符串的脆弱性。need_voice 在 ActionBarItem.need_voice 字段。
fn decide_files_action(action_type: &str, need_voice: bool) -> (bool, bool) {
    if action_type == "agent" && need_voice {
        (true, false)
    } else {
        (false, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_files_action_agent_with_need_voice_triggers_voice() {
        let (voice, direct) = decide_files_action("agent", true);
        assert_eq!((voice, direct), (true, false));
    }

    #[test]
    fn decide_files_action_agent_without_need_voice_executes_directly() {
        let (voice, direct) = decide_files_action("agent", false);
        assert_eq!((voice, direct), (false, true));
    }

    #[test]
    fn decide_files_action_script_type_executes_directly() {
        let (voice, direct) = decide_files_action("script", false);
        assert_eq!((voice, direct), (false, true));
    }

    #[test]
    fn decide_files_action_url_type_executes_directly() {
        let (voice, direct) = decide_files_action("url", false);
        assert_eq!((voice, direct), (false, true));
    }
}
