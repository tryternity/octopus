//! notify-rs 文件监听：app 目录变化时实时刷新索引。
//!
//! macOS FSEvents 对非用户所有文件（/System/Applications 等）可能漏事件——
//! main.rs 中 2 分钟数量轮询作为 fallback，本模块负责秒级响应。
//!
//! 关键生命周期约束：`RecommendedWatcher` 必须 keep-alive 才能持续收事件。
//! drop 后监听立即停止。因此 watcher 在注册完 `watch()` 后被 move 进
//! 事件循环线程（`let _watcher = watcher;` 保证存活到线程结束）。
//! 发送端 tx 被 watcher 内部持有，接收端 rx 在事件循环线程读取。

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;
use tauri::Emitter;
use std::time::{Duration, Instant};

/// 监听目录——与 app_index 扫描目录保持一致。
const WATCH_DIRS: &[&str] = &["/Applications", "/System/Applications", "/Applications/Utilities"];

/// debounce 窗口：安装/卸载 app 触发大量连续 Create/Remove 事件，避免逐个重扫。
const DEBOUNCE: Duration = Duration::from_secs(3);

/// 启动 app 目录监听。notify 收到 Create/Remove 事件 → debounce 3s → refresh_app_index。
///
/// 失败时静默返回（main.rs 的轮询线程会兜底）：
/// - watcher init 失败 → log warn + return
/// - 单个目录 watch 失败（如目录不存在）→ log debug 跳过
pub fn start_app_watcher() {
    let (tx, rx) = mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("[file_watcher] init failed: {}, fallback to polling only", e);
            return;
        }
    };

    // 注册监听目录。单个失败不影响其它目录（/System/Applications 在某些机器可能权限受限）。
    for dir in WATCH_DIRS {
        if let Err(e) = watcher.watch(std::path::Path::new(dir), RecursiveMode::Recursive) {
            log::debug!("[file_watcher] watch {} failed: {}", dir, e);
        }
    }
    // ~/Applications（用户级 app）单独处理，路径需 home_dir 解析。
    if let Some(home) = dirs::home_dir() {
        if let Err(e) = watcher.watch(&home.join("Applications"), RecursiveMode::Recursive) {
            log::debug!("[file_watcher] watch ~/Applications failed: {}", e);
        }
    }
    log::info!("[file_watcher] app 目录监听已启动");

    // watcher move 进线程保持存活——只要此线程在跑，watcher 就持续监听。
    // watcher 发事件到 tx（已被自身持有），rx 在此线程 for 循环读取。
    std::thread::spawn(move || {
        // _watcher 绑定后存在于线程栈，直到 rx 关闭（watcher drop）线程才退出。
        // 不能省略此绑定——否则 watcher 在闭包开头即 drop，无法收事件。
        let _watcher = watcher;
        let mut last_trigger = Instant::now();
        for ev in rx {
            if let Ok(e) = ev {
                if matches!(e.kind, EventKind::Create(_) | EventKind::Remove(_)) {
                    if last_trigger.elapsed() > DEBOUNCE {
                        last_trigger = Instant::now();
                        if let Some(engine) = octopus_search::get_engine() {
                            let n = engine.refresh_app_index();
                            log::info!("[file_watcher] app 目录变化，重扫: {} 个应用", n);
                        }
                    }
                }
            }
        }
        // rx 关闭（watcher drop）后循环自然退出。
        log::debug!("[file_watcher] 事件循环退出");
    });
}

/// prompt 文件监听的 debounce 窗口——外部编辑器保存可能触发多次 write 事件。
const PROMPT_DEBOUNCE: Duration = Duration::from_millis(500);

/// 启动 `~/.octopus/.sync/prompts/` 目录监听。文件修改时 emit `compact-editor://file-changed`
/// 事件（携带文件路径），前端据此自动 reload 或提示冲突。
/// watcher 线程持有 AppHandle，keep-alive 到 app 退出。
pub fn start_prompt_file_watcher(app: tauri::AppHandle) {
    let dir = octopus_infra::paths::octopus_config_home().join(".sync").join("prompts");
    if !dir.exists() {
        // 目录不存在先创建（首次使用）
        let _ = std::fs::create_dir_all(&dir);
    }

    let (tx, rx) = mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("[file_watcher] prompt watcher init failed: {}", e);
            return;
        }
    };
    if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
        log::debug!("[file_watcher] prompt watch {} failed: {}", dir.display(), e);
    }

    std::thread::spawn(move || {
        let _watcher = watcher; // keep-alive
        let mut last_trigger = Instant::now();
        for ev in rx {
            if let Ok(e) = ev {
                if matches!(e.kind, EventKind::Modify(_)) {
                    if last_trigger.elapsed() > PROMPT_DEBOUNCE {
                        last_trigger = Instant::now();
                        // 取变化的 .md 文件路径，emit 给前端
                        for path in &e.paths {
                            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                                let path_str = path.to_string_lossy().to_string();
                                log::debug!("[file_watcher] prompt 文件变化: {}", path_str);
                                let _ = app.emit("compact-editor://file-changed", &path_str);
                            }
                        }
                    }
                }
            }
        }
        log::debug!("[file_watcher] prompt 事件循环退出");
    });
}
