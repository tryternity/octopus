//! 应用 setup 初始化（从 main.rs::run() 的 setup 闭包提取，2026-07-29 拆分第二步）。
//!
//! 结构体模式：AppSetup 持有 app 引用 + config + 子任务间共享状态，
//! 每个子任务段是一个方法，run() 串联调用。
//!
//! 第三步（2026-07-29）把原 setup_all 587 行按 12 段注释分节抽成 12 个 `&mut self` 方法，
//! setup_all 变成 ~15 行串联调用。跨段共享变量提升为 Option 字段（&self → &mut self）。

use std::sync::Arc;
use log::info;
use tauri::{Emitter, Listener, Manager};
use octopus_infra::config::AppConfig;

#[cfg(not(feature = "cloud"))]
use crate::engine::engine_embedded::EmbeddedEngine;

/// 应用 setup 初始化器——把原 setup 闭包的 587 行逻辑按职责分段。
///
/// 子任务间共享的状态作为字段（逐步创建），各方法通过 &self 访问。
pub(crate) struct AppSetup<'a> {
    app: &'a tauri::App,
    config: &'a AppConfig,
    /// 跨段共享：init_clipboard 创建（manage 到 State），init_input watcher/worker 复用。
    clipboard_handle: Option<Arc<octopus_clipboard::ClipboardHandle>>,
    /// 跨段共享：init_engine 创建（manage 到 State），init_coordinator build_local_engine / DispatchEngine 复用。
    engine_manager: Option<Arc<octopus_asr_local::engine::AsrEngineManager>>,
}

impl<'a> AppSetup<'a> {
    pub(crate) fn run(app: &'a tauri::App, config: &'a AppConfig) -> Result<(), Box<dyn std::error::Error>> {
        let mut setup = AppSetup {
            app,
            config,
            clipboard_handle: None,
            engine_manager: None,
        };
        setup.setup_all()
    }

    fn setup_all(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // macOS GUI app（.app 从 Finder 启动）的 PATH 只有 /usr/bin:/bin:/usr/sbin:/sbin，
        // 不含 homebrew（/opt/homebrew/bin）、nvm（~/.nvm/...）、cargo（~/.cargo/bin）等。
        // 导致 which claude / which pi / which ffmpeg 等全部失败（agent adapter 检测不到、
        // ffmpeg/ffprobe 找不到）。cargo run 不受影响（继承终端 shell PATH）。
        // 修正：从 login shell 拿用户真实 PATH 注入进程环境。
        #[cfg(target_os = "macos")]
        fix_path_for_gui_app();

        self.init_clipboard()?;
        self.init_cleanup();
        self.init_scheduler();
        self.init_watchers();
        self.init_input();
        self.create_windows();
        self.init_engine()?;
        self.init_vault();
        self.init_coordinator();
        self.init_pty();
        self.init_tray();
        self.create_result_window();
        self.register_shortcuts();
        Ok(())
    }

    /// onboarding 引导页 + clipboard_handle 创建/管理 + 方言/热词装载 + 图片迁移。
    /// 填充 `clipboard_handle` 字段（后续 init_input 消费）。
    fn init_clipboard(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // 首次启动检测：onboarding_completed == false → 弹权限引导页。
        // 引导页内用户逐一授权 3 个权限（麦克风/辅助功能/屏幕录制），点「完成」后
        // complete_onboarding 命令写 flag + 关窗。非首次启动跳过。
        // 引导页替代了原来的「启动直接弹 AX 系统对话框」——避免多个系统弹窗同时出现。
        let onboarding_needed = octopus_infra::config::load_config()
            .map(|c| !c.onboarding_completed)
            .unwrap_or(true); // config 加载失败也弹引导页（首次启动常见）
        if onboarding_needed {
            log::info!("[startup] 首次启动，打开权限引导页");
            crate::ui::onboarding_window::open_onboarding(self.app.handle());
        }

        // Initialize clipboard handle (clipboard-rs, replaces tauri-plugin-clipboard-manager)
        let clipboard_handle = Arc::new(
            octopus_clipboard::ClipboardHandle::new()
                .map_err(|e| format!("Failed to init clipboard handle: {e}"))?,
        );
        self.app.manage(clipboard_handle.clone());

        // 启动时把 DB 的 clipboard_enabled 同步到运行时 flag。
        // ClipboardHandle::new() 默认 recording_enabled = true，而运行时改开关走的是
        // set_config 热重载——若不在此补一次性同步，用户关掉「剪贴板监听」并重启后，
        // watcher 又恢复录制（flag 回 true），但 DB 仍是 false，设置形同虚设。
        clipboard_handle.set_recording_enabled(self.config.clipboard_enabled);

        // 确保 extensions 目录存在
        let ext_dir = crate::core::extensions::extensions_dir();
        if !ext_dir.exists() {
            let _ = std::fs::create_dir_all(&ext_dir);
        }

        // 2026-07-21 perf：移除启动时无条件 rebuild FTS5 索引。
        // 原代码每次冷启动都跑 `INSERT INTO clipboard_history_fts VALUES('rebuild')`，
        // 在 10MB DB 上耗时 50-200ms。但触发器（clip_fts_ai/ad/au）在事务内执行，
        // 事务原子性保证 FTS 与主表一致——除非 DB 文件物理损坏，rebuild 也救不回来。
        // cleanup.rs 删除行时仍会条件 rebuild（`if deleted > 0 || reclaimed > 0`），
        // 首次启动的 populate 由 schema 初始化时的种子数据触发器自动处理。

        // 启动时应用方言模糊规则（须先于热词装载：规则影响索引 key 归一化，
        // 先 reload_fuzzy_dialect 从 DB 读规则设缓存，再 reload_hotwords 建索引）。
        octopus_asr_local::corrector::reload_fuzzy_dialect();

        // 启动时装载 active 热词到 corrector（force init + reload 索引）。
        // 之后所有引擎纠错自动用上热词（候选有界，空热词即 no-op 零过纠）。
        match octopus_asr_local::db::list_active_words() {
            Ok(entries) => octopus_asr_local::corrector::reload_hotwords(entries),
            Err(e) => log::warn!("[hotword] 启动装载失败，纠错以空热词运行: {}", e),
        }

        self.clipboard_handle = Some(clipboard_handle);
        Ok(())
    }

    /// 启动时按配置执行自动清理（剪贴板超期/超量非收藏 + 录屏孤儿）。
    fn init_cleanup(&self) {
        // 启动时按配置执行自动清理（删除超期/超量非收藏记录 + 回收孤立图片文件 + DB 行）。
        // clipboard_max_items / clipboard_max_age_days 此前是无处调用的摆设；
        // 此处接入让设置页"最大保留条数 / 自动清理天数"真正生效。
        // run_cleanup 在有删除时内部重建 FTS。
        {
            let max_age = self.config.clipboard_max_age_days as u32;
            let max_items = self.config.clipboard_max_items as u32;
            if let Err(e) = octopus_infra::db::with_db(|conn| {
                octopus_clipboard::cleanup::run_cleanup(conn, max_age, max_items)
            }) {
                log::warn!("Startup clipboard cleanup failed: {}", e);
            }
        }

        // 录屏孤儿清理（2026-07-25 screen record MVP，Task 11）：
        // 上次 crash / kill 残留的 .mp4 没入库，启动时扫 recordings/ 删掉。
        // 仅 macOS 编译（record crate 暂只 mac provider）。
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = octopus_infra::db::with_db(|conn| {
                cleanup_orphan_recordings(conn);
                Ok::<_, anyhow::Error>(())
            }) {
                log::warn!("Startup orphan recording cleanup failed: {}", e);
            }
        }
    }

    /// 通用调度器：每 10 分钟醒一次，CPU 空闲时执行注册的任务。
    /// 统一管所有剪贴板定时清理 + vault 自动同步。
    fn init_scheduler(&self) {
        // 通用调度器：每 10 分钟醒一次，CPU 空闲时执行注册的任务。
        // 统一管所有剪贴板定时清理（2026-07-22 合并了原每小时固定线程）。
        //
        // 任务 — clipboard_cleanup：按天数 + 按数量清理（用户可配的 max_age_days / max_items）。
        //   全部物理删（容量管理不走软删）。voice 软删的 100 条回收站上限
        //   已在 delete_item / clear_history 等入口实时 enforce（2026-07-29），无需后台任务。
        {
            let mut scheduler = octopus_scheduler::Scheduler::new();
            scheduler.register_task("clipboard_cleanup", 600, Box::new(|| {
                let cfg = octopus_infra::config::load_config().unwrap_or_default();
                let max_age = cfg.clipboard_max_age_days as u32;
                let max_items = cfg.clipboard_max_items as u32;
                if let Err(e) = octopus_infra::db::with_db(|conn| {
                    octopus_clipboard::cleanup::run_cleanup(conn, max_age, max_items)
                }) {
                    log::warn!("Scheduled clipboard cleanup failed: {}", e);
                }
            }));
            // vault 自动同步（Phase 2，2026-07-22）：
            // scheduler 每 10 分钟 tick，本任务 interval=3600（1 小时）——
            // scheduler 自带「距上次执行超过 1 小时才跑」语义。
            // sync_now 是阻塞操作（10-30s），起子线程避免阻塞 scheduler tick。
            // 结果存 last_auto_sync.json，SyncPanel 展示，不弹 toast。
            #[cfg(feature = "vault")]
            scheduler.register_task("vault_sync", 3600, Box::new(|| {
                std::thread::spawn(|| {
                    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
                    match octopus_vault::sync::sync_now() {
                        Ok(report) => {
                            octopus_sync::store::write_last_auto_sync(
                                &octopus_sync::store::LastAutoSync {
                                    timestamp: now,
                                    success: true,
                                    message: report.message,
                                },
                            );
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            log::warn!("[sync] 自动同步失败：{}", msg);
                            octopus_sync::store::write_last_auto_sync(
                                &octopus_sync::store::LastAutoSync {
                                    timestamp: now,
                                    success: false,
                                    message: msg,
                                },
                            );
                        }
                    }
                });
            }));
            scheduler.spawn();
        }
    }

    /// app/prompt 文件监听 + 应用索引后台校准 + 命令索引 LLM 关键字生成。
    fn init_watchers(&self) {
        // 启动 notify-rs 文件监听：app 目录变化时秒级刷新索引。
        // macOS FSEvents 对 /System 等非用户目录可能漏事件——下面的轮询作为 fallback。
        crate::core::file_watcher::start_app_watcher();

        // 启动 prompt 文件监听：~/.octopus/.sync/prompts/ 下文件外部变化时
        // emit compact-editor://file-changed，CompactEditor 自动 reload 或提示冲突。
        crate::core::file_watcher::start_prompt_file_watcher(self.app.handle().clone());

        // 应用索引后台自动刷新（mtime 轮询）：用户装卸应用后无需重启即可搜到。
        // 启动后延迟 30s（避开 ASR 预热等重活），之后每 10 分钟检测 /Applications 等
        // 目录 mtime，变化时才触发全量重扫（扫盘耗时数秒，仅在真实变化时发生）。
        // 内存索引通过 SearchEngine.app_index 的 RwLock 热替换，搜索走读锁零阻塞。
        std::thread::spawn(move || {
            // 启动后 30s 首次校准（检查 DB 缓存是否过期——新装/卸载 app），之后每 2 分钟。
            // 原方案靠目录 mtime 检测，但直接拷 .app 进 /Applications 不一定改目录 mtime，
            // 导致新装 app 搜不到。改用"文件系统 .app 数量 vs 索引数量"对比——数量变了就 rescan。
            std::thread::sleep(std::time::Duration::from_secs(30));
            let watch_dirs = ["/Applications", "/System/Applications", "/Applications/Utilities"];
            let home_apps = dirs::home_dir().map(|h| h.join("Applications"));
            // 快速计数：递归列出各目录下的 .app 数量（不提取 icon，毫秒级）
            let count_apps = || -> usize {
                let mut total = 0;
                for dir in &watch_dirs {
                    total += count_apps_in_dir(std::path::Path::new(dir), 0);
                }
                if let Some(ref home) = home_apps {
                    total += count_apps_in_dir(home, 0);
                }
                total
            };
            let mut last_count = count_apps();
            // 启动首次校准：DB 缓存的 app 数量 vs 文件系统实际数量
            if let Some(e) = octopus_search::get_engine() {
                let cached = e.cached_app_count();
                if cached != last_count {
                    log::info!("[search] 启动校准：DB 缓存 {} 个 app vs 文件系统 {} 个，重扫", cached, last_count);
                    let n = e.refresh_app_index();
                    log::info!("[search] 启动校准重扫完成: {} 个应用", n);
                }
            }
            loop {
                std::thread::sleep(std::time::Duration::from_secs(120));
                let now_count = count_apps();
                if now_count != last_count {
                    last_count = now_count;
                    log::info!("[search] 应用数量变化 ({}), 后台重扫", now_count);
                    if let Some(e) = octopus_search::get_engine() {
                        let n = e.refresh_app_index();
                        log::info!("[search] 后台重扫完成: {} 个应用", n);
                    }
                }
            }
        });

        // 命令索引后台 LLM 关键字生成（独立 OS 线程，blocking HTTP 不阻塞 main）。
        // 扫描 PATH 产生的命令只有英文 description（whatis/brew desc），中文用户搜不到——
        // 这里逐个调 LLM 生成中英文关键字，写回 DB 缓存 + 内存索引。增量：每生成一条立即落盘，
        // 崩溃不丢全部进度。LLM 是 reqwest::blocking，但本线程本身就是独立 OS 线程，直接同步调即可
        // （无 async runtime，不能 spawn_blocking）。
        //
        // config 每轮从 DB 重读（与 cleanup 线程同模式）——用户可能运行时改 polish_llm 配置。
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(60)); // 启动 60s 后开始（避开 ASR 预热等重活）
            loop {
                let engine = match octopus_search::get_engine() {
                    Some(e) => e,
                    None => {
                        std::thread::sleep(std::time::Duration::from_secs(300));
                        continue;
                    }
                };
                let pending = engine.commands_needing_keywords();
                if pending.is_empty() {
                    std::thread::sleep(std::time::Duration::from_secs(600)); // 无待生成，10 分钟后再查
                    continue;
                }
                // 每轮重读 config：polish_llm 可能在运行时被改过。
                let _config = octopus_infra::config::load_config().unwrap_or_default();
                let llm_config = match crate::core::config::llm_config_ignore_mode() {
                    Some(c) => c,
                    None => {
                        std::thread::sleep(std::time::Duration::from_secs(600)); // LLM 未配置，10 分钟后重试
                        continue;
                    }
                };
                let system = "你是命令行工具专家。为给定命令生成简短的中英文搜索关键字，用空格分隔。只输出关键字，不要解释。包含：命令功能、同义词、中文翻译。限 30 字以内。";
                let mut generated = 0;
                for (name, path, desc) in pending.iter().take(20) { // 每轮最多 20 个
                    let user = format!("命令: {}\n英文描述: {}", name, desc);
                    match octopus_llm::chat_text_with_prompt(system, &user, &llm_config, None) {
                        Ok(keywords) => {
                            let keywords = keywords.trim();
                            if !keywords.is_empty() {
                                engine.update_command_keywords(path, keywords);
                                generated += 1;
                            }
                        }
                        Err(e) => log::warn!("[search] 命令 LLM 关键字生成失败 ({}): {}", name, e),
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500)); // 防限流
                }
                log::info!("[search] 命令 LLM 关键字: 本轮生成 {} 个", generated);
                std::thread::sleep(std::time::Duration::from_secs(30)); // 轮间隔
            }
        });
    }

    /// focus_tracker + AX watcher + clipboard 队列 worker。
    /// 消费 `clipboard_handle` 字段（init_clipboard 填充）。
    fn init_input(&mut self) {
        // Start focus tracker (macOS no-op, Windows/Linux TODO)
        let focus_tracker = std::sync::Arc::new(crate::platform::focus_tracker::FocusTracker::new());
        if let Err(e) = focus_tracker.start() {
            log::warn!("Focus tracker not available: {}", e);
        }
        self.app.manage(focus_tracker);

        // Start clipboard watcher (background thread, clipboard-rs)
        {
            let app_handle_for_watcher = self.app.handle().clone();
            let clipboard_handle = self.clipboard_handle.clone().expect(
                "init_clipboard must run before init_input (clipboard_handle not set)"
            );

            // 启动后台 worker：watcher 回调只 enqueue（<1μs），编码/入库在 worker 异步做。
            // 避免 watcher 线程被 WebP 编码 + DB 写阻塞（连续复制时延迟入库）。
            let emit_handle = app_handle_for_watcher.clone();
            crate::clipboard::clipboard_queue::start_clipboard_worker(
                clipboard_handle.clone(),
                Arc::new(move || {
                    let _ = emit_handle.emit("clipboard://changed", ());
                }),
            );

            match octopus_clipboard::ClipboardWatcher::start(clipboard_handle.clone(), move || {
                // 旧代码：直接在 watcher 线程同步处理（阻塞）
                // octopus_clipboard::watcher::handle_clipboard_change(...);
                // let _ = app_handle_for_watcher.emit("clipboard://changed", ());
                //
                // 新代码：只 enqueue 信号，worker 异步处理。
                // suppress 检查仍在这里做（watcher 的 ChangeHandler 已处理），
                // 到这里说明是"用户真实复制"——enqueue 让 worker 处理。
                crate::clipboard::clipboard_queue::enqueue();
            }) {
                Ok(watcher) => { self.app.manage(watcher); }
                Err(e) => log::error!("Failed to start clipboard watcher: {}", e),
            }
        }

        // Register clipboard window global shortcut (from config)
        if !self.config.clipboard_shortcut.is_empty() {
            if let Err(e) = crate::clipboard::clipboard_window::register_clipboard_shortcut(self.app.handle(), &self.config.clipboard_shortcut) {
                log::error!("Failed to register clipboard shortcut: {}", e);
            }
        }

        // Register screenshot global shortcut (from config)
        if !self.config.screenshot_shortcut.is_empty() {
            if let Err(e) = crate::record::screenshot_commands::register_screenshot_shortcut(self.app.handle(), &self.config.screenshot_shortcut) {
                log::error!("Failed to register screenshot shortcut: {}", e);
            }
        }
    }

    /// download/action_bar/overlay 窗口 + builtin 模型缺失检测 + 兜底引擎自动激活 + 录屏快捷键。
    fn create_windows(&self) {
        // Builtin 模型缺失检测（spec 2026-07-22-builtin-models.md §3）：
        // is_available 同步已在顶层 preheat 前完成（sync_builtin_models_availability）。
        // 此处仅检测缺失 → 弹下载窗（需要 self.app.handle，所以放 setup 钩子）。
        let missing = crate::commands::builtin_models::check_builtin_models_missing();
        if !missing.is_empty() {
            log::info!(
                "[startup] builtin 模型缺失（{} 个），打开下载页: {:?}",
                missing.len(),
                missing.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()
            );
            crate::ui::download_window::create_download_window(self.app.handle());
        } else {
            log::info!("[startup] builtin 模型全部就绪");
        }

        // ASR 兜底引擎自动激活（2026-07-28）：ASR 域无激活模型（is_enabled=1）
        // + zipformer-small 文件就绪（is_available=1）→ 自动激活。
        // 场景：全新库 db.sql seed 把 is_enabled 全设 0，用户没去设置页激活时，
        // 兜底引擎虽可用（resolve_active_engine runtime fallback）但 DB 显示未激活，
        // 且部分流程依赖 is_enabled=1（如 tray 引擎名显示）。激活后 DB 反映真实状态。
        if let Err(e) = crate::commands::builtin_models::auto_activate_fallback_asr() {
            log::warn!("[startup] ASR 兜底引擎自动激活失败（不阻断启动）：{e}");
        }

        // Create + register action bar window (AI command palette)
        crate::action_bar::action_bar_window::create_action_bar_window(self.app.handle());
        crate::ui::overlay_window::create_overlay_window(self.app.handle());
        crate::action_bar::action_hotkey::register_action_hotkeys(self.app.handle());
        // 录屏快捷键（config-driven，与 screenshot 同模式）：
        // 失败仅 warn 不阻断启动——录屏不是核心 ASR 功能，可用 tray menu 代替。
        // 仅注册 toggle（Cmd+Shift+R）；ESC stop 按需注册（录制开始时，
        // 见 record_commands::start_with_config），避免吞掉其他窗口的 DOM 级 ESC。
        #[cfg(target_os = "macos")]
        {
            if !self.config.record_shortcut.is_empty() {
                if let Err(e) = crate::record::record_hotkey::register_toggle_hotkey(
                    self.app.handle(),
                    &self.config.record_shortcut,
                ) {
                    log::warn!("[record] 快捷键注册失败: {e}");
                }
            }
        }
        if !self.config.action_bar_shortcut.is_empty() {
            if let Err(e) = crate::action_bar::action_bar_window::register_action_bar_shortcut(self.app.handle(), &self.config.action_bar_shortcut) {
                log::error!("Failed to register action bar shortcut: {}", e);
            }
        }

        // vault Auto-Type 热键（默认 CmdOrCtrl+Shift+S）—— Task 19
        // follow-up #10: vault feature gate——feature off 时整段跳过（命令模块不存在）。
        #[cfg(feature = "vault")]
        {
            if !self.config.vault_autotype_shortcut.is_empty() {
                if let Err(e) = crate::vault::vault_commands::register_vault_autotype_shortcut(
                    self.app.handle(),
                    &self.config.vault_autotype_shortcut,
                ) {
                    log::warn!("注册 vault autotype 热键失败: {}", e);
                }
            }
        }
    }

    /// ASR 引擎管理器创建 + 激活引擎解析 + preheat 预热 + SystemStatusSampler 注入。
    /// 填充 `engine_manager` 字段（后续 init_coordinator 消费）。
    fn init_engine(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Initialize engine manager
        let engine_manager = Arc::new(octopus_asr_local::engine::AsrEngineManager::new());

        // 一次性解析激活 ASR 引擎 → ResolvedEngine，用于 preheat 判定。
        let resolved_engine = octopus_asr_local::config::resolve_active_engine("asr");

        // 云引擎判定（仅用于 preheat 守卫）：启动时激活引擎为 Aliyun → 跳过本地预热。
        // 运行时引擎路由由 DispatchEngine 按 spec 动态分发，不依赖此判定。
        #[cfg(feature = "cloud")]
        let is_cloud_aliyun = resolved_engine.as_ref()
            .map(|r| r.as_engine_category() == Some(octopus_asr_local::config::EngineCategory::Aliyun))
            .unwrap_or(false);

        // Preheat 仅本地 embedded 离线引擎：
        // - 云引擎 AliyunEngine 无需本地预热（跳过避免 switch_model 对 aliyun bail）；
        // - 流式引擎（is_streaming）走 StreamingSessionManager，录制时不经过离线 AsrEngineManager，
        //   若预热离线版会把同一模型的离线 ONNX Session 常驻在 AsrEngineManager 里却从不使用，
        //   与流式 Session 并存 → 双重加载浪费内存（~100-300MB）。流式引擎在首次录音时由
        //   prepare_streaming_session 懒加载进 StreamingSessionManager，无需启动预热。
        let do_preheat = self.config.engine_mode == "embedded"
            && !crate::core::config::is_streaming_engine();
        #[cfg(feature = "cloud")]
        let do_preheat = do_preheat && !is_cloud_aliyun;

        // 系统状态页：创建 registry + sampler，manage 为 State，启动采样循环 + 注入模型 probe。
        // 必须在 preheat spawn 之前——set_probe 同步完成后，预加载模型的加载才会被探针捕获，
        // 否则启动预热的 ASR/VAD 可能抢在注入前加载而漏进 registry。
        {
            let registry = Arc::new(crate::commands::system_status_commands::ModelMemoryRegistry::new());
            let sampler = Arc::new(crate::commands::system_status_commands::SystemStatusSampler::new(registry));
            self.app.manage(sampler.clone());
            sampler.start(self.app.handle().clone());
        }

        if do_preheat {
            let resolved_model = match &resolved_engine {
                Ok(r) => r.name.clone(),
                Err(_) => "zipformer-small-ctc".to_string(),
            };
            info!("Preheating active ASR model in desktop: {}", resolved_model);
            let em = engine_manager.clone();
            let active_model = resolved_model;
            std::thread::spawn(move || {
                if let Err(e) = em.switch_model(&active_model) {
                    log::error!("Failed to preheat active ASR model {}: {}", active_model, e);
                } else {
                    info!("Active ASR model {} preheated successfully", active_model);
                }
                // 预加载 VAD session 到全局缓存：首次 Toggle 命中缓存，消除录音启动延迟。
                // 失败不影响启动（首次录音时 new() 会懒加载重试）。
                match octopus_asr_local::config::create_silero_vad() {
                    Ok(_) => info!("VAD session preheated"),
                    Err(e) => log::warn!(
                        "VAD 预加载失败（不影响启动，首次录音懒加载）: {}", e
                    ),
                }
            });
        }

        self.engine_manager = Some(engine_manager);
        Ok(())
    }

    /// vault session 创建（SharedVaultSession + app_key bootstrap + 全局 session 注入）。
    /// vault_session 为方法内局部变量（set_global_session move 消费后不再跨段使用）。
    fn init_vault(&self) {
        // vault AppState：进程内持有解锁态的 user_vault_key / app_key。
        // 先 bootstrap_app_key（用 K_machine 尝试解 app_key）再 manage——
        // 这样从 Tauri State 取到 session 时 app_key 已就位（若本机已初始化）。
        //
        // follow-up #10: vault feature gate——feature off 时整段跳过：
        //   - 不 manage SharedVaultSession（vault_state 模块未编入）
        //   - 不 set_global_session（try_global_session 返回 None →
        //     vault_secret_access::try_decrypt_secret_global 退化为 raw passthrough）
        #[cfg(feature = "vault")]
        {
            let vault_session: crate::vault::vault_state::SharedVaultSession = std::sync::Arc::new(
                parking_lot::RwLock::new(crate::vault::vault_state::VaultSession::default()),
            );
            crate::vault::vault_state::bootstrap_app_key(&vault_session);
            self.app.manage(vault_session.clone());
            // VaultPicker URL 缓存：热键触发时（show 浮窗之前）抓 URL 存入，
            // vault_detect_and_match 优先读此缓存（修 e2e 发现的抢前台 bug）。
            let picker_url_cache: crate::vault::vault_state::SharedPickerUrlCache =
                std::sync::Arc::new(std::sync::Mutex::new(None));
            self.app.manage(picker_url_cache);
            // follow-up #7：注入进程级全局 session 句柄，供 cloud 推理热路径
            // （AliyunEngine::transcribe / crate::core::config::llm_config_ignore_mode / 云端翻译）
            // 解密 v1: 前缀的 secret_key。
            crate::vault::vault_state::set_global_session(vault_session);
        }
    }

    /// 运行时配置 + Coordinator + RecordSession + 录屏窗口 + stop-requested listener。
    /// 消费 `engine_manager` 字段（init_engine 填充）；runtime_config / vault_session 为局部变量。
    fn init_coordinator(&mut self) {
        // Create engine —— aliyun feature 下用 DispatchEngine（持有本地 + 云端两个实例，
        // 每次 transcribe 按 spec 动态路由），解决运行时切换云/本地引擎不匹配的问题。
        // 非 aliyun feature 仅本地引擎（embedded/websocket/grpc）。
        let engine_manager = self.engine_manager.clone().expect(
            "init_engine must run before init_coordinator (engine_manager not set)"
        );
        let engine: Arc<dyn crate::engine::engine::TranscriptionEngine> = {
            #[cfg(feature = "cloud")]
            {
                Arc::new(crate::engine::engine_dispatch::DispatchEngine::new(engine_manager.clone()))
            }
            #[cfg(not(feature = "cloud"))]
            {
                build_local_engine(&self.config, &engine_manager)
            }
        };

        // 暴露 engine_manager 为 State（审查 三2）：switch_asr_engine / set_config 切引擎时
        // 后台 switch_model 预热需要它。DispatchEngine 持有的是 clone，此处再 clone 托管。
        self.app.manage(engine_manager);

        // 流式引擎复用 manager（②）：desktop 录音 reset() 复用常驻 StreamingSession，
        // 避免每次录音重载 ONNX Session。对齐离线 engine_manager 的注入方式。
        let streaming_manager = Arc::new(
            octopus_asr_local::streaming_engine::StreamingSessionManager::new(),
        );
        self.app.manage(streaming_manager);

        // 2. Create AudioRecorder and open the device (graceful fallback if mic is missing)
        let audio_state = match crate::engine::audio::AudioRecorder::new(&self.config.microphone) {
            Ok(mut recorder) => {
                if let Err(e) = recorder.open() {
                    log::error!("Failed to open audio device '{}': {}. Audio input will be silent.", self.config.microphone, e);
                }
                recorder.shared()
            }
            Err(e) => {
                log::error!("Failed to initialize AudioRecorder: {}. Audio input will be silent.", e);
                std::sync::Arc::new(crate::engine::audio::SharedAudioState::new(&self.config.microphone))
            }
        };

        // 运行时共享配置——唯一真相源（Arc<RwLock<AppConfig>>）
        let runtime_config: crate::core::runtime_config::SharedRuntimeConfig =
            std::sync::Arc::new(parking_lot::RwLock::new(self.config.clone()));
        self.app.manage(runtime_config.clone());

        // 3. Create Coordinator
        let coordinator = crate::engine::coordinator::Coordinator::new(
            engine,
            audio_state,
            self.config.clone(),
            self.app.handle().clone(),
            runtime_config.clone(),
        );
        self.app.manage(coordinator);

        // 录屏会话状态（Task 10，2026-07-25 screen record MVP）：
        // RecordSession 内部已用 Arc<tokio::sync::Mutex<...>> 持有 helper 子进程句柄，
        // 这里直接 manage 不再外层包 Mutex。仅 macOS 编译（windows/linux provider 待适配）。
        #[cfg(target_os = "macos")]
        self.app.manage(octopus_record::RecordSession::new());
        // 录屏配置浮窗预创建（visible=false，Cmd+Shift+R 触发时 show）。
        // 与 overlay_window 同模式——启动时建好窗口壳，触发时只 set_position + show，
        // 避免按需创建的 ~200ms 启动延迟（用户期望快捷键立即响应）。
        #[cfg(target_os = "macos")]
        crate::record::record_window::create_record_window(self.app.handle());

        // 标注 overlay 的「停止录制」按钮 emit record://stop-requested。
        // 监听后调 stop_and_store（与 ESC/tray 同路径，读 session 快照入库）。
        #[cfg(target_os = "macos")]
        {
            let app_handle = self.app.handle().clone();
            let _ = self.app.handle().listen("record://stop-requested", move |_event| {
                log::info!("[record] stop-requested from annotation overlay");
                let ah = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    use octopus_record::SessionState;
                    let session = match ah.try_state::<octopus_record::RecordSession>() {
                        Some(s) => s,
                        None => {
                            log::warn!("[record] stop-requested: RecordSession 未找到");
                            return;
                        }
                    };
                    let st = session.state().await;
                    if st != SessionState::Recording && st != SessionState::Paused {
                        log::info!("[record] stop-requested 在非录制态忽略（state={:?}）", st);
                        return;
                    }
                    match crate::record::record_commands::stop_and_store(&session, &ah, false, None).await {
                        Ok(Some(meta)) => {
                            log::info!("[record] 停止入库成功: id={} file={}", meta.id, meta.file_path);
                            crate::record::record_annotation_window::close_annotation_window(&ah);
                            crate::record::record_control_window::close_control_window(&ah);
                            let _ = ah.emit("record://stopped", &meta);
                        }
                        Ok(None) => log::info!("[record] stop 返回 None"),
                        Err(e) => {
                            log::error!("[record] stop + 入库失败: {e}");
                            let _ = ah.emit("record://stop-failed", &e);
                        }
                    }
                });
            });
        }
    }

    /// 内嵌终端 PTY session 注册表挂载到 Tauri State。
    ///
    /// PtyState 是空 HashMap，pty_open 时填充。无重型初始化（不像 engine 要预热），
    /// 纯 manage 即可——session 在 pty_open 按需 spawn。
    fn init_pty(&self) {
        self.app.manage(octopus_pty::PtyState::new());
    }

    /// i18n 初始化 + tray 创建 + 麦克风子菜单预热 + locale 变化 listener。
    fn init_tray(&self) {
        // 4. Initialize i18n + Create Tray
        crate::ui::i18n::init(&self.config.ui_language);
        if let Err(e) = crate::ui::tray::create_tray(self.app.handle(), &self.config) {
            log::error!("Tray init failed ({}), running without tray menu", e);
        }
        // 麦克风子菜单设备项后台预热：cpal 枚举放后台线程，避免阻塞主线程
        // 导致 WKWebView 内容进程启动超时被杀（web content process terminated）。
        crate::ui::tray::preheat_microphone_submenu(self.app.handle(), &self.config.microphone);

        // 4.1 Listen for locale changes → rebuild tray menu labels
        {
            let app_handle = self.app.handle().clone();
            self.app.listen("locale-changed", move |_event| {
                let cfg = octopus_infra::config::load_config().unwrap_or_default();
                crate::ui::i18n::reload(&cfg.ui_language);
                crate::ui::tray::rebuild_tray_labels(&cfg);
                let _ = app_handle; // keep handle alive
            });
        }
    }

    /// 结果窗创建（启动时预创建壳，触发时只 set_position + show）。
    fn create_result_window(&self) {
        // 5. Create Result Window
        crate::ui::result_window::create_result_window(self.app.handle());
    }

    /// ASR + edit 全局快捷键注册。
    fn register_shortcuts(&self) {
        // 6.1 Register global edit shortcut（跨应用唤起结果窗 + toggle 编辑）
        if let Err(e) = crate::ui::result_window::register_edit_global_shortcut(self.app.handle(), &self.config.edit_global_shortcut) {
            log::error!("Failed to register global edit shortcut: {}", e);
        }

        // 单键三模式：注册 asr_shortcut 键监听（handy-keys）。值不合法时 fallback OptRight。
        let asr_key = if ["OptRight", "CmdRight", "CtrlRight", "ShiftRight", "Fn"].contains(&self.config.asr_shortcut.as_str()) {
            &self.config.asr_shortcut
        } else {
            log::warn!("[setup] asr_shortcut '{}' 不合法，fallback OptRight", self.config.asr_shortcut);
            "OptRight"
        };
        if let Err(e) = crate::platform::ptt::register_ptt(self.app.handle(), asr_key) {
            log::warn!("[ptt] 注册失败: {}", e);
        }

        info!("octopus-desktop initialized");
    }
}

// ============================================================================
// 启动期工具函数（2026-07-29 从 main.rs 搬入，仅被 AppSetup 方法调用）
// ============================================================================

/// 启动时孤儿录屏文件清理。
///
/// 场景：上次录制 crash / 强制 kill 后，helper 写出的 .mp4 没入库就成了孤儿。
/// 启动时扫 `recordings/`，DB 不认得的文件直接删除（避免占用磁盘）。
///
/// 与 clipboard cleanup 同模式：通过 `octopus_infra::db::with_db` 拿连接，
/// 调 `RecordStore::list_all_file_paths` 取 DB 已知 file_path 集合，
/// 再扫目录比对。失败不阻塞启动（log::warn 继续）。
#[cfg(target_os = "macos")]
fn cleanup_orphan_recordings(conn: &rusqlite::Connection) {
    let store = octopus_record::RecordStore::new(conn);
    let known_files = match store.list_all_file_paths() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[record] 孤儿清理查询失败: {e}");
            return;
        }
    };

    let recordings_dir = octopus_infra::paths::recordings_dir();
    let entries = match std::fs::read_dir(&recordings_dir) {
        Ok(e) => e,
        Err(_) => return, // 目录不存在是正常的（首次启动或从未录制过）
    };

    // ⚠️ file_path 在 DB 里存的是绝对路径（2026-07-27 保存目录可配置后改），
    // list_all_file_paths 直接返回 DB 原值（绝对路径）。磁盘文件用 entry.path() 也是绝对路径，
    // 两者都是绝对路径，直接 to_string_lossy 比较即可。
    //
    // 曾有 bug（2026-07-28 e2e 发现）：旧代码 strip_prefix(octopus_root) 把磁盘文件转成相对路径
    // 再与 DB 的绝对路径比较 → 永远不匹配 → 所有录屏文件被当孤儿删掉（数据丢失）。
    for entry in entries.flatten() {
        let path = entry.path();
        let abs = path.to_string_lossy().to_string();
        if octopus_record::RecordStore::is_orphan(&abs, &known_files) {
            log::warn!("[record] 孤儿文件清理: {abs}");
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// 按 `config.engine_mode` 构建本地 ASR 引擎（embedded / websocket / grpc）。
///
/// 仅在未启用 `cloud` feature 时使用（cloud 下由 DispatchEngine 统一路由）。
#[cfg(not(feature = "cloud"))]
fn build_local_engine(
    config: &octopus_infra::config::AppConfig,
    engine_manager: &std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>,
) -> std::sync::Arc<dyn crate::engine::engine::TranscriptionEngine> {
    match config.engine_mode.as_str() {
        "embedded" => std::sync::Arc::new(EmbeddedEngine::new(engine_manager.clone())),
        #[cfg(feature = "remote-ws")]
        "websocket" => std::sync::Arc::new(crate::engine::engine_ws::WsRemoteEngine::new(&config.remote_url)),
        #[cfg(feature = "remote-grpc")]
        "grpc" => std::sync::Arc::new(crate::engine::engine_grpc::GrpcRemoteEngine::new(&config.grpc_endpoint)),
        other => {
            log::warn!("Unknown engine_mode '{}', falling back to embedded", other);
            std::sync::Arc::new(EmbeddedEngine::new(engine_manager.clone()))
        }
    }
}

/// 递归计数目录下的 .app 数量（深度 ≤2，不进入 .app 包内部）。
/// 用于后台轮询快速检测新装/卸载的 app（不提取 icon，毫秒级）。
fn count_apps_in_dir(dir: &std::path::Path, depth: u32) -> usize {
    const MAX_DEPTH: u32 = 2;
    if depth > MAX_DEPTH {
        return 0;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("app") {
            count += 1;
        } else if path.is_dir() {
            count += count_apps_in_dir(&path, depth + 1);
        }
    }
    count
}

/// macOS GUI app PATH 修正。
///
/// 从 Finder 启动的 .app 不继承 login shell 的 PATH（只有 /usr/bin:/bin:/usr/sbin:/sbin），
/// 导致 `which claude` / `which pi` / `which ffmpeg` 等找不到 homebrew/nvm/cargo 装的工具。
/// `cargo run` 不受影响（继承终端 shell 的完整 PATH）。
///
/// 两步策略：
/// 1. 尝试 `zsh -l -c 'echo $PATH'` 拿 login shell 完整 PATH（含所有用户自定义路径）
/// 2. 兜底：直接追加常见路径（homebrew / .local/bin / cargo / fnm / nvm）
/// 仅 macOS 需要（Linux GUI app 通常通过 /etc/profile 或 desktop session 继承 PATH）。
#[cfg(target_os = "macos")]
fn fix_path_for_gui_app() {
    let current = std::env::var("PATH").unwrap_or_default();

    // 策略 1：用户默认 shell 的 login shell PATH（含 ~/.profile / ~/.zprofile / ~/.bash_profile）
    // 用 $SHELL 拿用户实际 shell（zsh / bash / fish），不硬编码 zsh——没装 zsh 的用户也能工作。
    // 失败/超时/没装该 shell 时静默 fallback 到策略 2（兜底路径）。
    let shell_path = std::env::var("SHELL").ok()
        .filter(|s| !s.is_empty())
        .and_then(|shell| {
            std::process::Command::new(&shell)
                .args(["-l", "-c", "echo $PATH"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });

    // 策略 2：兜底常见路径（login shell 失败/超时时仍能找到主流工具）
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/Shared".into());
    let mut fallback_dirs: Vec<String> = vec![
        "/opt/homebrew/bin".into(), "/opt/homebrew/sbin".into(),
        "/usr/local/bin".into(),
        format!("{}/.local/bin", home),
        format!("{}/.cargo/bin", home),
        format!("{}/.bun/bin", home),
    ];

    // fnm / nvm 的 node 版本路径含动态版本号，无法硬编码——扫目录通配。
    // fnm: ~/.local/share/fnm/node-versions/*/installation/bin
    // nvm: ~/.nvm/versions/node/*/bin
    // glob 展开（取最新版本——目录名是版本号，排序后取最后一个）
    for (pattern_base, suffix) in [
        (format!("{}/.local/share/fnm/node-versions", home), "installation/bin"),
        (format!("{}/.nvm/versions/node", home), "bin"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&pattern_base) {
            let mut versions: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect();
            // 按目录名排序取最新版本（fnm/nvm 版本号字符串排序够用）
            versions.sort_by_key(|e| e.file_name());
            if let Some(latest) = versions.last() {
                fallback_dirs.push(latest.path().join(suffix).to_string_lossy().into_owned());
            }
        }
    }

    let mut merged = shell_path.clone().unwrap_or_default();
    // 追加兜底路径（去重：merged 里已有的不加）
    for dir in &fallback_dirs {
        if !merged.split(':').any(|p| p == dir.as_str()) {
            if !merged.is_empty() { merged.push(':'); }
            merged.push_str(dir);
        }
    }
    // 追加当前 PATH（保留 GUI 默认 /usr/bin 等）
    if !current.is_empty() {
        merged.push(':');
        merged.push_str(&current);
    }

    if merged != current {
        std::env::set_var("PATH", &merged);
        log::info!(
            "[startup] PATH 修正（GUI app 继承 login shell + 兜底路径）source={}",
            if shell_path.is_some() { "login_shell+fallback" } else { "fallback_only" }
        );
    }
}
