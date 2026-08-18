//! macOS 系统菜单（原生 Menu）——i18n + File「打开文件」（2026-08-18）。
//!
//! Tauri 默认菜单标签为英文硬编码；本模块按 `ui_language` 定制全部标签
//! （键在前端 locales 的 `menu:` 段，经 `ui::i18n` 共享读取），并在 File 下加
//! 「打开文件…」（⌘O）——走 Rust 侧 plugin-dialog 选择器 + `open_files` 管线
//! （collect_open_tabs → open_tabs_batched，与前端工具栏按钮同一后端路径）。
//! 语言切换时经 `locale-changed` 事件重建（对齐 tray 的 rebuild 模式）。
//!
//! 仅 macOS 安装（其余平台无 app 级菜单概念，托盘仍是主入口）。

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager};

/// 选择器过滤扩展名——与前端 `openFilesUtils.ts` 的 TEXT_IMAGE_EXTS 保持一致
///（后端才是真相源：collect_open_tabs 分流；此处仅为选择器 UX）。
const TEXT_IMAGE_EXTS: &[&str] = &[
    "md", "markdown", "txt", "log", "json", "yml", "yaml", "toml", "xml", "csv",
    "html", "htm", "js", "jsx", "ts", "tsx", "py", "rs", "sh", "css", "svg",
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif",
];

fn tr(key: &str) -> String {
    crate::ui::i18n::t(key, &[])
}

/// 构建 + 安装 app 级菜单（macOS）。
#[cfg(target_os = "macos")]
pub fn build_and_install(app: &AppHandle) -> tauri::Result<()> {
    let menu = build(app)?;
    app.set_menu(menu)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn build(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    // App 子菜单（标题留空——macOS 自动显示 app 名）
    let app_submenu = Submenu::with_items(
        app,
        "",
        true,
        &[
            &PredefinedMenuItem::about(app, Some(&tr("menu.about")), None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, Some(&tr("menu.services")))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some(&tr("menu.hide")))?,
            &PredefinedMenuItem::hide_others(app, Some(&tr("menu.hideOthers")))?,
            &PredefinedMenuItem::show_all(app, Some(&tr("menu.showAll")))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some(&tr("menu.quit")))?,
        ],
    )?;
    // File：打开文件（⌘O）+ 关闭窗口（⌘W，预定义项自带 accelerator）
    let file_submenu = Submenu::with_items(
        app,
        tr("menu.file"),
        true,
        &[
            &MenuItem::with_id(app, "open-files", tr("menu.openFiles"), true, Some("CmdOrCtrl+O"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some(&tr("menu.closeWindow")))?,
        ],
    )?;
    let edit_submenu = Submenu::with_items(
        app,
        tr("menu.edit"),
        true,
        &[
            &PredefinedMenuItem::undo(app, Some(&tr("menu.undo")))?,
            &PredefinedMenuItem::redo(app, Some(&tr("menu.redo")))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some(&tr("menu.cut")))?,
            &PredefinedMenuItem::copy(app, Some(&tr("menu.copy")))?,
            &PredefinedMenuItem::paste(app, Some(&tr("menu.paste")))?,
            &PredefinedMenuItem::select_all(app, Some(&tr("menu.selectAll")))?,
        ],
    )?;
    let view_submenu = Submenu::with_items(
        app,
        tr("menu.view"),
        true,
        &[&PredefinedMenuItem::fullscreen(app, Some(&tr("menu.fullscreen")))?],
    )?;
    let window_submenu = Submenu::with_items(
        app,
        tr("menu.window"),
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some(&tr("menu.minimize")))?,
            &PredefinedMenuItem::maximize(app, Some(&tr("menu.zoom")))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some(&tr("menu.close")))?,
        ],
    )?;
    Menu::with_items(app, &[&app_submenu, &file_submenu, &edit_submenu, &view_submenu, &window_submenu])
}

/// 语言切换重建（locale-changed 事件，对齐 tray::rebuild_tray_labels 模式）。
pub fn rebuild(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = build_and_install(app) {
            log::warn!("[app-menu] 重建失败（保留旧菜单）: {}", e);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

/// 菜单事件入口（setup 经 `app.on_menu_event` 挂载；主线程回调）。
pub fn on_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    if event.id().as_ref() != "open-files" {
        return;
    }
    let app = app.clone();
    // blocking_pick_files 阻塞（用户选择可达数秒）——不能卡主线程
    std::thread::spawn(move || {
        use tauri_plugin_dialog::DialogExt;
        let picked = app
            .dialog()
            .file()
            .add_filter(tr("editor.openFilesFilter"), TEXT_IMAGE_EXTS)
            .blocking_pick_files();
        let paths: Vec<String> = picked
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.into_path().ok())
            .filter_map(|p| p.to_str().map(String::from))
            .collect();
        if paths.is_empty() {
            return; // 用户取消
        }
        let (tabs, errors) = crate::commands::compact_editor_commands::collect_open_tabs(paths);
        if !errors.is_empty() {
            // 菜单路径无前端入口——CompactEditor 若在开，emit 让它 toast；否则仅日志
            if app.get_webview_window(crate::commands::compact_editor_window::WINDOW_LABEL).is_some() {
                let _ = app.emit_to(
                    crate::commands::compact_editor_window::WINDOW_LABEL,
                    "open-files://errors",
                    &errors,
                );
            } else {
                log::warn!("[app-menu] 打开文件部分失败（CompactEditor 未开，无 toast）: {:?}", errors);
            }
        }
        let ah = app.clone();
        let _ = app.run_on_main_thread(move || {
            // create_compact_editor_window 含 set_dock_icon 需主线程
            crate::commands::compact_editor_commands::open_tabs_batched(tabs, &ah);
        });
    });
}
