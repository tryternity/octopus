//! 录屏 Tauri 命令（薄封装 octopus-record crate）。
//!
//! DB 访问模式：复用 octopus 既有 `octopus_infra::db::with_db(|conn| ...)` 全局函数，
//! 通过 ReentrantMutex 保护连接（参考 clipboard_commands.rs 模式）。
//! spawn_blocking 包裹避免长 DB 操作阻塞 tokio worker。
//!
//! 全模块 `#[cfg(target_os = "macos")]`：octopus-record 当前只实现了 macOS provider，
//! windows/linux provider 为占位（platform/windows.rs、platform/linux.rs）。
//! desktop crate 也仅在 macOS target 段引入 octopus-record 依赖，故此处整体 gate。

#![cfg(target_os = "macos")]

use octopus_record::platform::HelperProvider;
use octopus_record::{
    DisplayInfo, ListFilter, MicrophoneInfo, PermissionStatus, PrivacySection, RecordError,
    RecordSession, RecordStore, RecordingMeta, RecordingRequest, Source, StartedInfo, VideoConfig,
    AudioConfig, Outputs, WindowInfo,
};
use rusqlite::Connection;
use tauri::{command, AppHandle, Emitter, State};

// ── 辅助函数 ──────────────────────────────────────────────────

/// 把 RecordError 转 String 的统一出口；顺手 log::error 记录便于排查 helper 故障。
fn e2s<E: std::fmt::Display + std::fmt::Debug>(e: E) -> String {
    log::error!("[record] {e:?}");
    e.to_string()
}

/// 把 HelperProvider trait 方法（同步签名，内部 block_in_place）包成 Result<T, String>。
fn platform_helper<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&dyn HelperProvider) -> Result<T, RecordError>,
{
    f(&octopus_record::platform::provider()).map_err(e2s)
}

/// ISO8601 UTC 时间戳（DB 里 created_at / deleted_at 统一格式）。
fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// DB 操作 spawn_blocking 包裹（with_db 持全局 ReentrantMutex，长 DB 操作避免阻塞 tokio）。
///
/// 类型推导链：
/// 1. `f: F` 返回 `Result<T, RecordError>`
/// 2. 闭包内 `Ok(f(conn)?)` 把内层 RecordError 经 `?` 提升为 `anyhow::Error`
///    （RecordError: std::error::Error 满足 anyhow::From），外层包 Ok 变 anyhow::Result<T>
/// 3. `with_db` 直接返回该 anyhow::Result<T>
/// 4. spawn_blocking 再包一层 `Result<anyhow::Result<T>, JoinError>`
/// 5. `.await.map_err(join)` → `anyhow::Result<T>`
/// 6. `.map_err(e2s)` → `Result<T, String>`
async fn with_db_blocking<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&Connection) -> Result<T, RecordError> + Send + 'static,
    T: Send + 'static,
{
    let result = tokio::task::spawn_blocking(move || {
        // 用 Ok(f(conn)?) 把闭包的 Result<T, RecordError> 收敛成 anyhow::Result<T>
        // （RecordError 实现了 std::error::Error，? 自动 From）。
        octopus_infra::db::with_db(|conn| Ok(f(conn)?))
    })
    .await
    .map_err(|e| format!("join error: {e}"))?; // anyhow::Result<T>

    result.map_err(e2s)
}

/// HelperEvent 已 derive Serialize（与 helper stdout 同一份 schema，双向用），
/// emit 时直接传给 Tauri，前端按 `event` 字段（kebab-case tag）match 分支。
/// 前端示例：
///   { "event": "recording-started", "timestamp_ms": 1773, "width": 1920, "height": 1080 }
///   { "event": "warning", "code": "...", "message": "..." }

// ── A. 源枚举（6 个，录制前调用）──────────────────────────────

#[command]
pub async fn list_record_displays() -> Result<Vec<DisplayInfo>, String> {
    // provider::list_displays 内部 block_in_place（跑 helper --list-displays），
    // 但命令本身已 async，tokio runtime 调度不受影响。无需 spawn_blocking。
    platform_helper(|p| p.list_displays())
}

#[command]
pub async fn list_record_windows() -> Result<Vec<WindowInfo>, String> {
    platform_helper(|p| p.list_windows())
}

#[command]
pub async fn list_microphones() -> Result<Vec<MicrophoneInfo>, String> {
    platform_helper(|p| p.list_microphones())
}

#[command]
pub async fn check_record_permission() -> Result<PermissionStatus, String> {
    platform_helper(|p| p.check_permission())
}

#[command]
pub async fn request_screen_record_permission() -> Result<PermissionStatus, String> {
    platform_helper(|p| p.request_screen_permission())
}

/// 打开 macOS 系统偏好设置里的隐私面板。
///
/// 用 `x-apple.systempreferences:` URL scheme 直跳指定 section，
/// 比 opener crate 多一次进程 fork 但少一个依赖，与项目惯例（clipboard_commands /
/// search_commands 一律 std::process::Command::new("open")）一致。
#[command]
pub async fn open_privacy_settings(section: PrivacySection) -> Result<(), String> {
    let url = match section {
        PrivacySection::ScreenCapture => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        PrivacySection::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
    };
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── B. 录制控制（5 个）───────────────────────────────────────

/// 前端组装的录制配置（spec §0.3 决策的 7 个维度）。
///
/// 与 record::protocol::RecordingRequest 的差别：不包含 schema_version、recording_id、
/// outputs——这些由 start_recording 命令在调用时填充。仅前端能控制的部分走此 struct。
#[derive(serde::Deserialize)]
pub struct RecordConfig {
    pub source: Source,
    pub video: VideoConfig,
    pub audio: AudioConfig,
}

/// 启动录制（前端显式传 RecordConfig）。
///
/// 命令签名说明：`state: State<'_, RecordSession>`——RecordSession 内部已用
/// `Arc<tokio::sync::Mutex<SessionInner>>` 持有状态，外层不再包 std::sync::Mutex
/// （否则 await 持锁跨 .await 会触发 Send 边界失败 + std Mutex::lock 不可 async）。
///
/// **命名**：spec §4.1 原写 `start_recording`，但 desktop/src/coordinator.rs:807 已有
/// 同名 ASR 录音命令（旧功能），Tauri 宏生成的 `__cmd__start_recording` 符号冲突。
/// 改用 `record_start` / `record_stop` / `record_pause` / `record_resume` / `record_kill`
/// 5 个统一前缀，避免与 ASR 体系命名冲突且更具区分度。
#[command]
pub async fn record_start(
    state: State<'_, RecordSession>,
    app: AppHandle,
    config: RecordConfig,
) -> Result<StartedInfo, String> {
    start_with_config(&state, &app, config).await
}

/// 启动录制（用 DB app_config 默认配置 + ASR 麦克风）。
///
/// 用户决策（2026-07-25）：`Cmd+Shift+R` 直接开始录屏，不走 Settings 配置浮窗。
/// 本命令从 DB 读 `record_*` 配置项 + 复用 ASR 的 `microphone` 字段作麦克风设备，
/// 组装 RecordConfig 后调 `start_with_config`。
///
/// 默认配置（spec §5.4 seed）：
/// - 源：主屏（list_displays 找 is_primary=true）
/// - 视频：30fps / H264 / 主屏原生分辨率 / 不隐藏光标
/// - 音频：系统音频开（excludes_current_process=true）+ 麦克风按 record_microphone 配置
///   （麦克风设备名优先用 record_microphone_device，空则回退 ASR 的 microphone 配置）
#[command]
pub async fn record_start_default(
    state: State<'_, RecordSession>,
    app: AppHandle,
) -> Result<StartedInfo, String> {
    let config = build_default_config().await?;
    start_with_config(&state, &app, config).await
}

/// 组装默认 RecordConfig（从 DB 读 record_* + ASR microphone）。
///
/// 失败模式：
/// - 读 DB 失败 → 返回 Err（让调用方 toast 提示）
/// - 找不到主屏 → 返回 Err（多屏环境异常或 helper --list-displays 失败）
/// 其他字段用 spec §5.4 seed 默认值兜底（解析失败保留 seed）。
/// 组装默认 RecordConfig（从 DB 读 record_* + ASR microphone）。
///
/// `pub(crate)` 让 record_hotkey / tray 复用（与 record_start_default 命令同逻辑）。
pub(crate) async fn build_default_config() -> Result<RecordConfig, String> {
    use octopus_record::VideoCodec;

    // ── 源：主屏（list_displays 找 is_primary）──────────────────────
    let displays = platform_helper(|p| p.list_displays())?;
    let primary = displays
        .iter()
        .find(|d| d.is_primary)
        .or_else(|| displays.first())
        .ok_or_else(|| "找不到可用显示器（helper --list-displays 返回空）".to_string())?;
    let source = Source::Display {
        display_id: primary.id,
    };

    // ── 视频：record_fps / record_codec / 主屏原生分辨率 ────────────────
    let fps: u32 = octopus_infra::db::load_config_key("record_fps")
        .map_err(e2s)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let codec_str = octopus_infra::db::load_config_key("record_codec")
        .map_err(e2s)?
        .unwrap_or_else(|| "h264".into());
    let codec = match codec_str.as_str() {
        "hevc" => VideoCodec::Hevc,
        _ => VideoCodec::H264,
    };
    let hide_cursor = parse_bool_config("record_hide_cursor", false);

    let video = VideoConfig {
        fps,
        width: primary.width,
        height: primary.height,
        codec,
        bitrate: None, // None = helper 按分辨率×fps 自动算
        hide_system_cursor: hide_cursor,
    };

    // ── 音频：系统音频 + 麦克风（按 record_microphone + ASR microphone 配置）─────
    let system_audio_on = parse_bool_config("record_system_audio", true);
    let mic_on = parse_bool_config("record_microphone", false);
    // 麦克风设备名：优先 record_microphone_device，空则回退 ASR microphone（用户决策：
    // 「麦克风用 ASR 的配置」——ASR 已配的麦克风直接复用，避免用户在录屏再配一次）
    let mic_device = octopus_infra::db::load_config_key("record_microphone_device")
        .map_err(e2s)?
        .filter(|s| !s.is_empty())
        .or_else(|| {
            octopus_infra::db::load_config_key("microphone")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty())
        });

    let audio = AudioConfig {
        system: octopus_record::SystemAudioConfig {
            enabled: system_audio_on,
            excludes_current_process: true,
        },
        microphone: octopus_record::MicrophoneConfig {
            enabled: mic_on,
            device_id: None,
            device_name: mic_device,
        },
    };

    Ok(RecordConfig {
        source,
        video,
        audio,
    })
}

/// 读 DB bool 配置项，失败/不存在用 default。
fn parse_bool_config(key: &str, default: bool) -> bool {
    octopus_infra::db::load_config_key(key)
        .ok()
        .flatten()
        .map(|s| s == "true")
        .unwrap_or(default)
}

/// record_start / record_start_default 共用的核心启动逻辑。
///
/// 接 `&RecordSession` 而非 `State<'_, RecordSession>`——State 仅是 Tauri 的状态容器
/// 包装，deref 到内层 T。用裸引用让 hotkey / tray 等非命令路径也能复用（它们经
/// `app.try_state::<RecordSession>()` 拿到 `State`，deref 后传入）。
pub(crate) async fn start_with_config(
    session: &RecordSession,
    app: &AppHandle,
    config: RecordConfig,
) -> Result<StartedInfo, String> {
    use octopus_infra::paths::recordings_dir;

    // recording_id 用 UTC 毫秒时间戳——与 clipboard items 同 ID 体系（chrono_millis），
    // 且前端依赖此 id 后续 stop_recording 时回传写入 DB。
    let recording_id = chrono::Utc::now().timestamp_millis();
    let file_name = format!(
        "{}_{}.mp4",
        chrono::Local::now().format("%Y-%m-%d_%H.%M.%S"),
        recording_id,
    );
    let dir = recordings_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let abs_path = dir.join(&file_name);

    let request = RecordingRequest {
        schema_version: 1,
        recording_id,
        source: config.source,
        video: config.video,
        audio: config.audio,
        outputs: Outputs {
            screen_path: abs_path.to_string_lossy().to_string(),
        },
    };

    // 解析 helper 路径——开发期走 crates/desktop/binaries/，打包后走 resource_dir。
    // provider 的 resolve_helper_path(None) 不传 resource_dir，依赖开发期路径；
    // 打包路径解析需 app.handle().path().resource_dir()——MVP 简化，仅开发期可用。
    let helper_path = platform_helper(|p| p.resolve_helper_path(None))?;

    // 事件回调：helper 进程输出 Warning/Error/RecordingPaused 等非命令响应事件时，
    // 经 Tauri emit 推给前端（前端订阅 'record://event' 更新 UI 状态）。
    let app_clone = app.clone();
    session
        .start(&helper_path, request, move |e| {
            let _ = app_clone.emit("record://event", &e);
        })
        .await
        .map_err(e2s)
}

#[command]
pub async fn record_pause(state: State<'_, RecordSession>) -> Result<(), String> {
    state.pause().await.map_err(e2s)
}

#[command]
pub async fn record_resume(state: State<'_, RecordSession>) -> Result<(), String> {
    state.resume().await.map_err(e2s)
}

/// 停止录制并（非 discard 时）入库。
///
/// 入库需要的 recording_id/width/height/source_type/has_* 字段由前端透传——
/// 这些值在 record_start 时由前端配置决定，session.rs MVP 简化不存。
#[command]
pub async fn record_stop(
    state: State<'_, RecordSession>,
    discard: bool,
    recording_id: i64,
    width: u32,
    height: u32,
    source_type: String,
    has_system_audio: bool,
    has_microphone: bool,
) -> Result<Option<RecordingMeta>, String> {
    use octopus_infra::paths::octopus_config_home;

    // stop 返回的 StoppedInfo.screen_path 在 session.rs MVP 实现里是空 PathBuf
    // （session 不存 RecordingStopped 事件的 payload）——这是 Task 5 的已知简化，
    // Task 11+ 优化时会引入 event channel 回传真实路径。
    //
    // Fallback 策略（MVP）：在 recordings_dir 下找文件名含 recording_id 的 .mp4。
    // 比「用 Local::now() 推文件名」更稳——因为 record_start 用 Local 当时时间命名，
    // 长录制跨天后 stop 时 Local 已变；按 recording_id 后缀匹配则不受影响。
    let stopped = state.stop().await.map_err(e2s)?;

    let abs_path = if stopped.screen_path.as_os_str().is_empty() {
        let dir = octopus_infra::paths::recordings_dir();
        let suffix = format!("_{recording_id}.mp4");
        let mut found: Option<std::path::PathBuf> = None;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(&suffix) {
                        found = Some(entry.path());
                        break;
                    }
                }
            }
        }
        found.unwrap_or_else(|| dir.join(suffix))
    } else {
        stopped.screen_path
    };

    if !abs_path.exists()
        || std::fs::metadata(&abs_path).map(|m| m.len()).unwrap_or(0) == 0
    {
        return Err(format!("录制文件异常: {}", abs_path.display()));
    }

    if discard {
        let _ = std::fs::remove_file(&abs_path);
        return Ok(None);
    }

    let file_size =
        std::fs::metadata(&abs_path).map(|m| m.len()).unwrap_or(0);
    // DB 里 file_path 存相对路径（recordings/xxx.mp4），运行时 join octopus_config_home。
    // 用 octopus_config_home 而非 brief 写的 octopus_root（后者不存在，Bug 1）。
    let file_path_rel = abs_path
        .strip_prefix(octopus_config_home())
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .to_string();

    let meta = RecordingMeta {
        id: recording_id,
        file_path: file_path_rel,
        title: String::new(),
        duration_ms: stopped.duration_ms,
        width,
        height,
        fps: 30,
        codec: "h264".into(),
        has_system_audio,
        has_microphone,
        source_type,
        file_size,
        has_thumbnail: false,
        is_favorite: false,
        created_at: now_iso(),
        deleted_at: None,
    };

    let meta_clone = meta.clone();
    with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.insert(&meta_clone, None)
    })
    .await?;

    Ok(Some(meta))
}

#[command]
pub async fn record_kill(state: State<'_, RecordSession>) -> Result<(), String> {
    state.kill().await.map_err(e2s)
}

// ── C. 录屏历史（10 个）──────────────────────────────────────

/// list_recordings 的前端参数：单独定义而非直接收 ListFilter，
/// 因为 ListFilter 在 record/store.rs 只 derive Debug/Clone/Default，无 Deserialize。
/// serde 从前端 JSON 反序列化到此 struct，再转 ListFilter。
#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub struct ListRecordingsParams {
    pub limit: u32,
    pub offset: u32,
    pub include_deleted: bool,
    pub favorites_only: bool,
}

impl From<ListRecordingsParams> for ListFilter {
    fn from(p: ListRecordingsParams) -> Self {
        ListFilter {
            limit: p.limit,
            offset: p.offset,
            include_deleted: p.include_deleted,
            favorites_only: p.favorites_only,
        }
    }
}

#[command]
pub async fn list_recordings(
    filter: Option<ListRecordingsParams>,
) -> Result<Vec<RecordingMeta>, String> {
    let filter: ListFilter = filter.unwrap_or_default().into();
    with_db_blocking(move |conn| RecordStore::new(conn).list(&filter)).await
}

#[command]
pub async fn get_recording(id: i64) -> Result<RecordingMeta, String> {
    with_db_blocking(move |conn| {
        RecordStore::new(conn)
            .get(id)?
            .ok_or(RecordError::NotFound(id))
    })
    .await
}

#[command]
pub async fn get_recording_thumbnail(id: i64) -> Result<Option<Vec<u8>>, String> {
    with_db_blocking(move |conn| RecordStore::new(conn).get_thumbnail(id)).await
}

#[command]
pub async fn rename_recording(id: i64, title: String) -> Result<(), String> {
    with_db_blocking(move |conn| RecordStore::new(conn).rename(id, &title)).await
}

#[command]
pub async fn toggle_recording_favorite(id: i64) -> Result<(), String> {
    with_db_blocking(move |conn| RecordStore::new(conn).toggle_favorite(id)).await
}

#[command]
pub async fn delete_recording(id: i64, permanent: bool) -> Result<(), String> {
    use octopus_infra::paths::resolve_recording_path;

    if permanent {
        // 先查 meta 拿相对路径 → 删文件 → 删 DB 行（顺序避免删文件后 DB 失败留孤儿）
        let file_path = with_db_blocking(move |conn| {
            let store = RecordStore::new(conn);
            let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
            Ok::<_, RecordError>(meta.file_path)
        })
        .await?;
        let abs = resolve_recording_path(&file_path);
        if abs.exists() {
            std::fs::remove_file(&abs).map_err(|e| e.to_string())?;
        }
        with_db_blocking(move |conn| RecordStore::new(conn).delete_db_row(id)).await
    } else {
        // 软删：仅打 deleted_at 时间戳，回收站可还原
        let now = now_iso();
        with_db_blocking(move |conn| RecordStore::new(conn).soft_delete(id, &now)).await
    }
}

#[command]
pub async fn restore_recording(id: i64) -> Result<(), String> {
    with_db_blocking(move |conn| RecordStore::new(conn).restore(id)).await
}

/// 用系统默认应用打开录屏文件（QuickTime Player）。
///
/// 与 clipboard_commands::open_file_item 同模式：std::process::Command::new("open")，
/// 不用 opener crate（项目惯例）。
#[command]
pub async fn open_recording_file(id: i64) -> Result<(), String> {
    use octopus_infra::paths::resolve_recording_path;
    let file_path = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
        Ok::<_, RecordError>(meta.file_path)
    })
    .await?;
    let abs = resolve_recording_path(&file_path);
    std::process::Command::new("open")
        .arg(&abs)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 在 Finder 中定位录屏文件。
///
/// macOS: `open -R <file>` 让 Finder 高亮选中文件；与 search_commands::reveal_path 一致。
/// 不用 NSWorkspace activateFileViewerSelecting（spec §9.2 F13 推迟项）。
#[command]
pub async fn reveal_recording(id: i64) -> Result<(), String> {
    use octopus_infra::paths::resolve_recording_path;
    let file_path = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
        Ok::<_, RecordError>(meta.file_path)
    })
    .await?;
    let abs = resolve_recording_path(&file_path);
    let path_str = abs.to_string_lossy().to_string();
    std::process::Command::new("open")
        .args(["-R", &path_str])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}
