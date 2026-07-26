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

    // move 前克隆 source（start 后创建标注 overlay 用，request 会被 move 进 session.start）
    let source_clone = request.source.clone();
    // 事件回调：helper 进程输出 Warning/Error/RecordingPaused 等非命令响应事件时，
    // 经 Tauri emit 推给前端（前端订阅 'record://event' 更新 UI 状态）。
    let app_clone = app.clone();
    let result = session
        .start(&helper_path, request, move |e| {
            let _ = app_clone.emit("record://event", &e);
        })
        .await;
    // 启动成功 → 更新 tray menu 文案为「停止录屏」（toggle 语义）
    if result.is_ok() {
        #[cfg(target_os = "macos")]
        crate::tray::update_record_tray_label(true);
        // Source::Area 时创建标注 overlay（普通 level，SCK 录到选区内 overlay 内容）
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = crate::record_annotation_window::create_annotation_window(app, &source_clone) {
                log::warn!("[record] 标注 overlay 创建失败（不影响录制）: {e}");
            }
            // 控制浮窗（display/window 录制用；area 已有 RecordAnnotation，create 内部过滤）
            crate::record_control_window::create_control_window(app, &source_clone);
            // ESC stop 全局快捷键按需注册——非录制态不注册，避免吞掉 Screenshot /
            // RecordConfig 等 DOM 级 ESC。详见 record_hotkey::register_stop_hotkey。
            if let Err(e) = crate::record_hotkey::register_stop_hotkey(app) {
                log::warn!("[record] ESC stop 快捷键注册失败（不影响录制）: {e}");
            }
        }
    }
    result.map_err(e2s)
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
    app_handle: AppHandle,
    discard: bool,
    recording_id: i64,
    width: u32,
    height: u32,
    source_type: String,
    has_system_audio: bool,
    has_microphone: bool,
) -> Result<Option<RecordingMeta>, String> {
    // 前端显式传字段路径：直接用前端给的值组装 MetaFields。
    let fields = MetaFields {
        recording_id,
        width,
        height,
        source_type,
        has_system_audio,
        has_microphone,
    };
    // State<'_, RecordSession> deref 到 &RecordSession，stop_and_store 接裸引用。
    stop_and_store(&state, &app_handle, discard, Some(fields)).await
}

/// hotkey / tray stop 复用的入库逻辑。
///
/// 读 `session.last_start_request()` 拿 start 时的 recording_id / source / video / audio
/// （这些字段 hotkey/tray 路径无法直接掌握，靠 session.rs 存的快照），组装 MetaFields
/// 后调 `stop_and_store_inner`。
///
/// `explicit_fields`：前端 `record_stop` 命令路径已显式传字段，直接用；None 则从
/// session 快照读（hotkey/tray 路径）。
pub(crate) async fn stop_and_store(
    session: &RecordSession,
    app: &AppHandle,
    discard: bool,
    explicit_fields: Option<MetaFields>,
) -> Result<Option<RecordingMeta>, String> {
    let fields = match explicit_fields {
        Some(f) => f,
        None => {
            // hotkey/tray 路径：从 session 快照读 start 时的字段
            let req = session.last_start_request().await.ok_or_else(|| {
                "stop_and_store: session 无 last_start_request（未 start 过？）".to_string()
            })?;
            derive_fields_from_request(&req)?
        }
    };
    let result = stop_and_store_inner(session, discard, fields).await;
    // 录制结束（无论入库成功失败）→ 注销 ESC stop 快捷键。
    // 失败也注销：异常状态下 ESC 不应残留（下次 Screenshot ESC 仍需正常）。
    // 详见 record_hotkey::unregister_stop_hotkey 的设计说明。
    #[cfg(target_os = "macos")]
    crate::record_hotkey::unregister_stop_hotkey(app);
    result
}

/// 从 RecordingRequest 推导入库需要的 MetaFields。
///
/// 源类型从 Source enum 推（Display/Window/Area → "display"/"window"/"area"），
/// 宽高从 VideoConfig 取，audio flags 从 AudioConfig 取。
fn derive_fields_from_request(req: &RecordingRequest) -> Result<MetaFields, String> {
    let source_type = match &req.source {
        Source::Display { .. } => "display",
        Source::Window { .. } => "window",
        Source::Area { .. } => "area",
    };
    Ok(MetaFields {
        recording_id: req.recording_id,
        width: req.video.width,
        height: req.video.height,
        source_type: source_type.to_string(),
        has_system_audio: req.audio.system.enabled,
        has_microphone: req.audio.microphone.enabled,
    })
}

/// 入库需要的字段（前端显式传 或 从 session 快照推）。
pub(crate) struct MetaFields {
    pub recording_id: i64,
    pub width: u32,
    pub height: u32,
    pub source_type: String,
    pub has_system_audio: bool,
    pub has_microphone: bool,
}

async fn stop_and_store_inner(
    session: &RecordSession,
    discard: bool,
    fields: MetaFields,
) -> Result<Option<RecordingMeta>, String> {
    use octopus_infra::paths::octopus_config_home;

    let MetaFields {
        recording_id,
        width,
        height,
        source_type,
        has_system_audio,
        has_microphone,
    } = fields;

    // StoppedInfo：reader task 收到 RecordingStopped 时存精确 payload（screen_path /
    // duration_ms / file_size）。session.stop() take 返回；正常路径下字段齐全。
    // Fallback（异常退出 / kill 路径未收到事件）：按 recording_id 扫 recordings_dir 找文件。
    let stopped = session.stop().await.map_err(e2s)?;

    let abs_path = if stopped.screen_path.as_os_str().is_empty() {
        // Fallback：未收到 RecordingStopped 事件（异常退出），按文件名 suffix 查
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

    let file_size = std::fs::metadata(&abs_path).map(|m| m.len()).unwrap_or(0);
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

    // 停止 + 入库成功 → tray menu 文案切回「开始录屏」（toggle 语义）
    #[cfg(target_os = "macos")]
    crate::tray::update_record_tray_label(false);

    Ok(Some(meta))
}

#[command]
pub async fn record_kill(
    state: State<'_, RecordSession>,
    app_handle: AppHandle,
) -> Result<(), String> {
    let r = state.kill().await.map_err(e2s);
    // 强杀路径：注销 ESC + 关闭浮窗（control + annotation），避免窗口泄漏残留。
    // 无论 kill 成功失败都清理（与 stop_and_store 一致）。
    #[cfg(target_os = "macos")]
    {
        crate::record_hotkey::unregister_stop_hotkey(&app_handle);
        crate::record_annotation_window::close_annotation_window(&app_handle);
        crate::record_control_window::close_control_window(&app_handle);
    }
    r
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

// ── F20 GIF 导出（P3）─────────────────────────────────────────

/// 同步探测 ffmpeg 是否存在（不报错，返回 Option）。
///
/// 查找顺序：`~/.octopus/bin/ffmpeg`（用户手动放/dlp 下载缓存）→ 系统 PATH（which）。
/// 复制自 `crates/dlp/src/main.rs:42-73` 的 `get_binary_path`（dlp 是 binary crate 无 lib，
/// 无法 use 导入；desktop crate 已有 4 处 which 内联副本，容忍此模式）。
fn probe_ffmpeg() -> Option<std::path::PathBuf> {
    // 1. ~/.octopus/bin/ffmpeg
    let home_bin = octopus_infra::octopus_config_home().join("bin").join("ffmpeg");
    if home_bin.exists() {
        return Some(home_bin);
    }
    // 2. 系统 PATH（which，与 agent_adapter.rs / paste.rs 同模式）
    let on_path = std::process::Command::new("which")
        .arg("ffmpeg")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if on_path {
        return Some(std::path::PathBuf::from("ffmpeg"));
    }
    None
}

/// 查找 ffmpeg 二进制路径（报错版本，给 export_gif 用）。
///
/// 未找到时返回错误，文案引导多种安装方式（brew + bash 下载 + 手动）。
/// 前端 GIF 按钮 disabled 时 tooltip 也用同一文案（通过 check_ffmpeg 命令拿）。
async fn find_ffmpeg() -> Result<std::path::PathBuf, String> {
    if let Some(p) = probe_ffmpeg() {
        return Ok(p);
    }
    Err(ffmpeg_missing_hint())
}

/// ffmpeg 缺失时的安装引导文案（多种方式）。
///
/// 不只 brew——用户可能没装 brew，提供 bash 直接下载静态二进制 + 手动放置两种。
/// 文案是多语言 key 的 fallback（i18n 加载前/英文化场景），前端通过 check_ffmpeg
/// 命令拿 bool 后用自己的 i18n key 渲染 tooltip。
fn ffmpeg_missing_hint() -> String {
    "ffmpeg 未找到。安装方式：\n\
     1. brew install ffmpeg\n\
     2. curl -L https://evermeet.cx/ffmpeg/getrelease/zip -o /tmp/ffmpeg.zip && unzip /tmp/ffmpeg.zip -d ~/.octopus/bin/\n\
     3. 手动下载放到 ~/.octopus/bin/ffmpeg"
        .into()
}

/// 探测 ffmpeg 是否可用（前端 GIF 按钮据此决定是否灰禁）。
///
/// 返回 bool：true=可用，false=未找到（前端显示 tooltip 引导安装）。
#[command]
pub async fn check_ffmpeg() -> bool {
    probe_ffmpeg().is_some()
}

/// 把已录制的 MP4 转成 GIF。
///
/// 输出位置：源 MP4 同目录、同名换 `.gif`（`-y` 覆盖）。
/// ffmpeg 参数（spec §2.20）：`fps=15,scale=800:-1:flags=lanczos -loop 0`。
///
/// 进度反馈：emit `record://gif-started {id}` → ffmpeg 跑完 →
/// 成功 emit `record://gif-done {id, path}` / 失败 emit `record://gif-failed {id, error}`。
/// 前端 invoke 返回值也能拿到路径/错误，事件作为多窗口同步备用。
#[command]
pub async fn export_gif(app: AppHandle, id: i64) -> Result<String, String> {
    use octopus_infra::paths::resolve_recording_path;

    // 1. 查 DB 拿 file_path
    let file_path = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
        Ok::<_, RecordError>(meta.file_path)
    })
    .await?;
    let input = resolve_recording_path(&file_path);
    if !input.exists() {
        return Err(format!("源文件不存在: {}", input.display()));
    }

    // 2. 解析 ffmpeg
    let ffmpeg = find_ffmpeg().await?;

    // 3. 输出路径：源文件同目录、同名换 .gif
    let output = input.with_extension("gif");

    // 4. emit 起点
    let _ = app.emit("record://gif-started", serde_json::json!({ "id": id }));

    // 5. spawn ffmpeg
    let status = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-i").arg(&input)
        .arg("-vf").arg("fps=15,scale=800:-1:flags=lanczos")
        .arg("-loop").arg("0")
        .arg(&output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| format!("ffmpeg spawn 失败: {e}"))?;

    if !status.success() {
        let _ = app.emit(
            "record://gif-failed",
            serde_json::json!({ "id": id, "error": "ffmpeg 转 GIF 失败" }),
        );
        return Err("ffmpeg 转 GIF 失败（退出码非 0）。可能原因：源文件损坏 / ffmpeg 版本过旧".into());
    }

    let path_str = output.to_string_lossy().to_string();
    let _ = app.emit(
        "record://gif-done",
        serde_json::json!({ "id": id, "path": path_str }),
    );
    Ok(path_str)
}
