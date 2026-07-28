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

/// 拿当前平台的 provider（零成本，MacOSProvider 是 ZST）。
///
/// trait async 化（2026-07-26，`#[async_trait]`）后，原 `platform_helper` 闭包 wrapper
/// 因 async_trait 的 `Box<dyn Future + Send + 'static>` 与 `&dyn HelperProvider` 的
/// 生命周期冲突编译不过。改为直接拿 provider 实例调用——ZST 无成本，调用点更直观。
fn provider() -> impl HelperProvider {
    octopus_record::platform::provider()
}

/// ISO8601 UTC 时间戳（DB 里 created_at 统一格式）。
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
    // provider().list_displays() 已 async 化（2026-07-26），直接 .await helper 子进程，
    // 不再走 block_in_place——async Tauri 命令在 runtime worker 上不阻塞。
    provider().list_displays().await.map_err(e2s)
}

#[command]
pub async fn list_record_windows() -> Result<Vec<WindowInfo>, String> {
    provider().list_windows().await.map_err(e2s)
}

#[command]
pub async fn list_microphones() -> Result<Vec<MicrophoneInfo>, String> {
    provider().list_microphones().await.map_err(e2s)
}

#[command]
pub async fn check_record_permission() -> Result<PermissionStatus, String> {
    provider().check_permission().await.map_err(e2s)
}

#[command]
pub async fn request_screen_record_permission() -> Result<PermissionStatus, String> {
    provider().request_screen_permission().await.map_err(e2s)
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
    let displays = provider().list_displays().await.map_err(e2s)?;
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
    // 麦克风设备名：复用 ASR 配的麦克风（resolve_mic_device_name 三级回退）
    let mic_device = resolve_mic_device_name(None);

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

/// 解析麦克风设备名（用户决策 2026-07-26：复用 ASR 已配的麦克风）。
///
/// 优先级：
/// 1. 调用方显式传入（RecordConfig UI 未来加设备选择器 / 测试场景）
/// 2. DB `record_microphone_device`（录屏专用配置，目前默认空）
/// 3. DB `microphone`（ASR 语音识别配的麦克风——用户已精心选过，复用避免录屏再配）
/// 4. 都空 → None（helper 用 SCK 内部默认设备，通常系统默认输入）
///
/// 修 bug 背景：RecordConfig UI 当前只发 `device_name: null`（无设备选择器），
/// 导致 helper 回退系统默认麦（可能是 MacBook 内置麦，灵敏度低 → 录屏音量极低）。
/// 复用 ASR 配的麦克风（用户已验证可用）是最小成本修复。
fn resolve_mic_device_name(explicit: Option<&str>) -> Option<String> {
    if let Some(name) = explicit {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    octopus_infra::db::load_config_key("record_microphone_device")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            octopus_infra::db::load_config_key("microphone")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty())
        })
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
        // 麦克风设备名回退：RecordConfig UI 当前只发 device_name=null（无设备选择器），
        // 这里兜底从 DB 读 ASR 配的麦克风名（用户已精心选过，复用避免录屏再配）。
        // 否则 helper 收到 deviceName=nil → 回退 SCK 内部默认麦（可能选错 → 录屏音量极低）。
        audio: {
            let mut audio = config.audio;
            if audio.microphone.enabled {
                audio.microphone.device_name =
                    resolve_mic_device_name(audio.microphone.device_name.as_deref());
            }
            audio
        },
        outputs: Outputs {
            screen_path: abs_path.to_string_lossy().to_string(),
        },
    };

    // 解析 helper 路径——开发期走 crates/desktop/binaries/，打包后走 resource_dir。
    // provider 的 resolve_helper_path(None) 不传 resource_dir，依赖开发期路径；
    // 打包路径解析需 app.handle().path().resource_dir()——MVP 简化，仅开发期可用。
    //
    // resolve_helper_path 是 sync 方法（纯文件探测，不走子进程），不 .await。
    let helper_path = provider().resolve_helper_path(None).map_err(e2s)?;

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
    // mic_device_name：stop 时 session 没存 start 解析的设备名，重新调
    // resolve_mic_device_name(None)（幂等，读 DB 配置，与 start 默认路径一致）。
    let fields = MetaFields {
        recording_id,
        width,
        height,
        source_type,
        has_system_audio,
        has_microphone,
        mic_device_name: resolve_mic_device_name(None),
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
        // hotkey/tray 路径：req.audio.microphone.device_name 在 start 时已解析过
        // （start_with_config 行 327-330），但 resolve_mic_device_name 是幂等的，
        // 再调一次保证拿到当前 DB 配置（用户可能在录屏中改了 ASR 麦克风）。
        mic_device_name: resolve_mic_device_name(req.audio.microphone.device_name.as_deref()),
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
    /// 麦克风设备名（start 时解析的三级回退值）。
    ///
    /// stop 时 session 没存 start 解析的设备名，但 `resolve_mic_device_name` 是幂等的
    /// （读 DB 配置），stop 时重新调一次即可。两条构造路径：
    /// - 前端 `record_stop`：`resolve_mic_device_name(None)`（与 start 默认路径一致）
    /// - hotkey/tray：`resolve_mic_device_name(req.audio.microphone.device_name.as_deref())`
    pub mic_device_name: Option<String>,
}

async fn stop_and_store_inner(
    session: &RecordSession,
    discard: bool,
    fields: MetaFields,
) -> Result<Option<RecordingMeta>, String> {
    let MetaFields {
        recording_id,
        width,
        height,
        source_type,
        has_system_audio,
        has_microphone,
        mic_device_name,
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
    // 2026-07-27：file_path 直接存**绝对路径**（用户可配置保存目录，目录可能在 ~/.octopus/ 外）。
    // resolve_recording_path 对绝对路径原样返回，无需 strip_prefix。
    let file_path = abs_path.to_string_lossy().to_string();

    // 探测实际音轨元数据（ffprobe 读 mp4 → 配置交叉推断 source）。
    // 失败兜底空 vec，不阻断录制入库（probe_recording_audio_tracks 内部已吞错）。
    let audio_tracks = crate::record_audio_probe::probe_recording_audio_tracks(
        &abs_path,
        has_system_audio,
        has_microphone,
        mic_device_name.as_deref(),
    )
    .await;

    let meta = RecordingMeta {
        id: recording_id,
        file_path,
        title: String::new(),
        duration_ms: stopped.duration_ms,
        width,
        height,
        fps: 30,
        codec: "h264".into(),
        has_system_audio,
        has_microphone,
        audio_tracks: audio_tracks.clone(),
        source_type,
        file_size,
        has_thumbnail: false,
        is_favorite: false,
        created_at: now_iso(),
        is_deleted: false,
    };

    let meta_clone = meta.clone();
    with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.insert(&meta_clone, None)
    })
    .await?;

    // 入库成功后写 mp4 udta metadata（audio_tracks JSON）。
    // 失败不阻断——DB 已有 audio_tracks 兜底，mp4 metadata 是 nice-to-have
    // （合并单轨 Task 3.1 / 前端展示双轨详情时用到）。
    // probe_ffmpeg 在本文件（同 module），直接调用；write_audio_tracks_metadata 跨 module。
    if !audio_tracks.is_empty() {
        if let Some(ffmpeg) = probe_ffmpeg() {
            if let Err(e) = crate::record_audio_probe::write_audio_tracks_metadata(
                &ffmpeg, &abs_path, &audio_tracks,
            )
            .await
            {
                log::warn!("[record] mp4 metadata 写入失败（不影响录制）: {e}");
            }
        }
    }

    // 停止 + 入库成功 → tray menu 文案切回「开始录屏」（toggle 语义）
    #[cfg(target_os = "macos")]
    crate::tray::update_record_tray_label(false);

    // 录制完成自动在 Finder 高亮文件（用户决策 2026-07-26，record_reveal_after_stop
    // 配置项默认 true）。非 macOS 静默跳过。失败仅 log，不影响录制结果。
    if parse_bool_config("record_reveal_after_stop", true) {
        #[cfg(target_os = "macos")]
        {
            let path_str = abs_path.to_string_lossy().to_string();
            if let Err(e) = std::process::Command::new("open")
                .args(["-R", &path_str])
                .spawn()
            {
                log::warn!("[record] Finder reveal 失败（不影响录制）: {e}");
            }
        }
    }

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
        // 软删：仅打 is_deleted = 1，回收站可还原
        with_db_blocking(move |conn| RecordStore::new(conn).soft_delete(id)).await
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
///
/// `pub(crate)`：被同 crate 的 `record_audio_probe::write_audio_tracks_metadata` 调用
/// （Task 2.3 在 stop_and_store_inner 入库成功后写 mp4 udta metadata）。
pub(crate) fn probe_ffmpeg() -> Option<std::path::PathBuf> {
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

/// 查询当前录制状态 + 已录秒数。
///
/// 用途：RecordControl 浮窗 mount 时初始化——浮窗创建晚于 recording-started 事件，
/// 收不到事件，靠此命令拿当前 state + elapsed_secs 启动计时器。
/// 返回 {state: "idle"/"recording"/"paused"/..., elapsedSecs: u64}。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordStatus {
    pub state: String,
    pub elapsed_secs: u64,
}

#[command]
pub async fn get_record_status(state: State<'_, RecordSession>) -> Result<RecordStatus, String> {
    let s = state.state().await;
    let elapsed = state.elapsed_secs().await.unwrap_or(0);
    Ok(RecordStatus {
        state: format!("{:?}", s).to_lowercase(),
        elapsed_secs: elapsed,
    })
}

/// 把已录制的 MP4 转成 GIF。
///
/// 输出位置：源 MP4 同目录、同名换 `.gif`（`-y` 覆盖）。
/// ffmpeg 参数（spec §2.20）：`fps=15,scale=800:-1:flags=lanczos -loop 0`。
///
/// 进度反馈：emit `record://task` + `RecordTaskEvent::GifStarted { id }` → ffmpeg 跑完 →
/// 成功 emit `RecordTaskEvent::GifDone { id, path }` / 失败 emit `RecordTaskEvent::GifFailed { id, error }`。
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
    let _ = app.emit("record://task", RecordTaskEvent::GifStarted { id });

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
            "record://task",
            RecordTaskEvent::GifFailed { id, error: "ffmpeg 转 GIF 失败".into() },
        );
        return Err("ffmpeg 转 GIF 失败（退出码非 0）。可能原因：源文件损坏 / ffmpeg 版本过旧".into());
    }

    let path_str = output.to_string_lossy().to_string();
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::GifDone { id, path: path_str.clone() },
    );
    Ok(path_str)
}

/// `merge_audio_tracks` 的返回值——新 recording 的 id 与文件绝对路径。
///
/// 前端拿到后跳详情 / reveal in Finder；事件 `record://task` +
/// `RecordTaskEvent::MergeDone` 作为多窗口同步备用。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub new_id: i64,
    pub file_path: String,
}

/// 录屏异步任务（GIF 导出 / 音轨合并）的进度事件。
///
/// 统一替代原 `record://gif-{started,done,failed}` + `record://merge-{started,done,failed}`
/// 6 个事件名。前端未来如需监听，单个 `listen("record://task", ...)` + `switch(payload.event)` 即可。
///
/// 与 `HelperEvent`（`record://event`）同模式：内部 tagged enum。
/// 变体名 kebab-case（外层 `rename_all`）+ 字段 camelCase（变体级 `rename_all`）——
/// 遵循 `AGENTS.md`「序列化 casing 规范」。
#[derive(serde::Serialize, Clone, Debug)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum RecordTaskEvent {
    #[serde(rename_all = "camelCase")]
    GifStarted {
        id: i64,
    },
    #[serde(rename_all = "camelCase")]
    GifDone {
        id: i64,
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    GifFailed {
        id: i64,
        error: String,
    },
    #[serde(rename_all = "camelCase")]
    MergeStarted {
        id: i64,
    },
    /// `new_id` 指向合并后新生成的 recording 记录 id（序列化为 `newId`）。
    #[serde(rename_all = "camelCase")]
    MergeDone {
        id: i64,
        new_id: i64,
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    MergeFailed {
        id: i64,
        error: String,
    },
    #[serde(rename_all = "camelCase")]
    SubtitleStarted {
        id: i64,
    },
    #[serde(rename_all = "camelCase")]
    SubtitleProgress {
        id: i64,
        stage: octopus_record::SubtitleProgress,
    },
    #[serde(rename_all = "camelCase")]
    SubtitleDone {
        id: i64,
        cue_count: usize,
    },
    #[serde(rename_all = "camelCase")]
    SubtitleFailed {
        id: i64,
        error: String,
    },
}

/// 把双轨录屏 mp4（mic + system）用 ffmpeg `amix` 合并成单轨，另存为新文件 + INSERT 新 DB 记录。
///
/// **amix（非 amerge）**：spike 发现 mic 常为 mono、system 为 stereo，`amerge` 要求两输入
/// 声道数相同会失败；`amix` 自动处理声道差异（mono 自动 upmix 到 stereo 混音），更稳健。
///
/// 流程（仿 `export_gif:863` 模式）：
/// 1. 查 DB 拿原 recording meta（`with_db_blocking` + `RecordStore::get`）
/// 2. 校验 `audio_tracks.len() >= 2`（非多音轨直接报错，不浪费 ffmpeg 调用）
/// 3. `find_ffmpeg` + `merged_output_path` 算输出路径
/// 4. emit `record://task` + `RecordTaskEvent::MergeStarted { id }` → spawn ffmpeg → 成功 emit done / 失败 emit failed
/// 5. ffmpeg 参数：`-filter_complex [0:a:0][0:a:1]amix=inputs=2:duration=longest:dropout_transition=0[a]`
///    + `-map 0:v -map [a] -c:v copy -c:a aac -b:a 192k`（视频流拷贝不重编码，音频重编码 AAC）
/// 6. 失败删 merged.mp4（避免半残文件占空间 + DB 不入库）
/// 7. ffprobe 探测 merged 文件音轨（应单轨，source=Merged）；ffprobe 不可用兜底构造一个 Merged track
/// 8. 写 mp4 metadata（失败仅 warn，不阻断——DB 已有 audio_tracks 兜底）
/// 9. INSERT 新 recording 记录（file_path = merged.mp4 绝对路径，title 加 `(merged)` 后缀，新 id）
///
/// **失败删 merged.mp4**：与 export_gif 不同（gif 失败也删 gif 但 export_gif 没写——本命令显式删，
/// 因 merged 文件体积大且与源同目录，半残文件会混淆用户）。
///
/// **stderr piped 但暂不用**：与 export_gif 一致（export_gif 用 `Stdio::null()`）。Phase 5 e2e
/// 如需诊断 ffmpeg 错误细节，再改成 piped + 读 stderr 进 error 文案。当前先 null 保持简洁。
///
/// **new_id**：`chrono::Utc::now().timestamp_millis()`——与 `start_with_config:305` 的
/// `recording_id` 同体系（chrono 毫秒戳），desktop crate 已有 chrono 依赖（Cargo.toml）。
#[command]
pub async fn merge_audio_tracks(app: AppHandle, id: i64) -> Result<MergeResult, String> {
    use octopus_infra::paths::resolve_recording_path;
    use octopus_record::audio_tracks::{AudioTrack, AudioTrackSource};

    // 1. 查 DB 拿原 recording meta（连同 file_path 一起，省去第二次查 DB）
    let meta = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.get(id)?.ok_or(RecordError::NotFound(id))
    })
    .await?;

    // 2. 校验：非多音轨直接报错（不浪费 ffmpeg 调用）
    if meta.audio_tracks.len() < 2 {
        return Err("不是多音轨录屏，无需合并".into());
    }

    let input = resolve_recording_path(&meta.file_path);
    if !input.exists() {
        return Err(format!("源文件不存在: {}", input.display()));
    }

    // 3. ffmpeg + 输出路径
    let ffmpeg = find_ffmpeg().await?;
    let output = crate::record_audio_probe::merged_output_path(&input);

    // 4. emit 起点（前端切 loading 态）
    let _ = app.emit("record://task", RecordTaskEvent::MergeStarted { id });

    // 5. spawn ffmpeg amix
    let status = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-i")
        .arg(&input)
        .arg("-filter_complex")
        .arg("[0:a:0][0:a:1]amix=inputs=2:duration=longest:dropout_transition=0[a]")
        .arg("-map")
        .arg("0:v")
        .arg("-map")
        .arg("[a]")
        .arg("-c:v")
        .arg("copy")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg(&output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| format!("ffmpeg spawn 失败: {e}"))?;

    if !status.success() {
        // 6. 失败删 merged.mp4（避免半残文件占空间）
        let _ = std::fs::remove_file(&output);
        let _ = app.emit(
            "record://task",
            RecordTaskEvent::MergeFailed { id, error: "ffmpeg amix 失败".into() },
        );
        return Err("ffmpeg amix 失败（退出码非 0）".into());
    }

    // 7. 探测 merged 文件音轨（应单轨）。ffprobe 不可用 / 解析失败 → 兜底构造一个 Merged track。
    let merged_tracks: Vec<AudioTrack> =
        match crate::record_audio_probe::probe_ffprobe() {
            Some(ffprobe) => {
                match crate::record_audio_probe::probe_audio_tracks(&ffprobe, &output).await {
                    Ok(raw) if !raw.is_empty() => vec![AudioTrack {
                        index: 0,
                        source: AudioTrackSource::Merged,
                        codec: raw[0].codec.clone(),
                        sample_rate: raw[0].sample_rate,
                        channels: raw[0].channels,
                        device_name: None,
                    }],
                    _ => vec![AudioTrack {
                        index: 0,
                        source: AudioTrackSource::Merged,
                        codec: "aac".into(),
                        sample_rate: 48000,
                        channels: 2,
                        device_name: None,
                    }],
                }
            }
            None => vec![AudioTrack {
                index: 0,
                source: AudioTrackSource::Merged,
                codec: "aac".into(),
                sample_rate: 48000,
                channels: 2,
                device_name: None,
            }],
        };

    // 8. 写 mp4 metadata（失败不阻断——DB 已有 audio_tracks 兜底）
    if let Err(e) = crate::record_audio_probe::write_audio_tracks_metadata(
        &ffmpeg,
        &output,
        &merged_tracks,
    )
    .await
    {
        log::warn!("[record] merged mp4 metadata 写入失败: {e}");
    }

    // 9. INSERT 新 recording 记录
    let file_size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let new_id = chrono::Utc::now().timestamp_millis();
    let file_path_str = output.to_string_lossy().to_string();
    let title = if meta.title.is_empty() {
        "merged".to_string()
    } else {
        format!("{} (merged)", meta.title)
    };
    let new_meta = RecordingMeta {
        id: new_id,
        file_path: file_path_str.clone(),
        title,
        duration_ms: meta.duration_ms,
        width: meta.width,
        height: meta.height,
        fps: meta.fps,
        codec: meta.codec.clone(),
        has_system_audio: true,
        has_microphone: true,
        audio_tracks: merged_tracks,
        source_type: meta.source_type.clone(),
        file_size,
        has_thumbnail: false,
        is_favorite: false,
        created_at: now_iso(),
        is_deleted: false,
    };

    with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.insert(&new_meta, None)
    })
    .await?;

    let _ = app.emit(
        "record://task",
        RecordTaskEvent::MergeDone { id, new_id, path: file_path_str.clone() },
    );

    Ok(MergeResult {
        new_id,
        file_path: file_path_str,
    })
}

// ── 字幕生成（P3）──────────────────────────────────────────────

/// 生成字幕：选轨 → ffmpeg 抽 16k mono PCM → ASR 带时间戳转写 → cue + SRT → UPDATE DB。
///
/// **编排**（与 `merge_audio_tracks` 同模式，但 ASR 段为同步阻塞）：
/// 1. emit `SubtitleStarted { id }`
/// 2. 查 DB 拿 `RecordingMeta`
/// 3. `select_track` 按 preference 选轨（Auto/Microphone/System）
/// 4. emit `SubtitleProgress::ExtractingAudio { 10 }`
/// 5. **`spawn_blocking`** 包 `extract_audio_track_to_pcm`（ffmpeg 跑数秒，避免阻塞 tokio runtime）
/// 6. emit `SubtitleProgress::Recognizing { 40 }`
/// 7. `engine_manager.active_engine()` 拿 `Arc<dyn OfflineAsrEngine>`
/// 8. `transcribe_segments_with_timestamps` → `Vec<TimestampedSegment>`
/// 8.5. （可选）`polish=Some` 时 emit `Polishing { 50 }` + LLM 整段润色 + 文本回填
/// 9. emit `SubtitleProgress::Finalizing { 90 }`
/// 10. 转 `SubtitleCue` + `generate_srt` + `SubtitleResult`
/// 11. UPDATE DB（cues 序列化为 JSON 字符串）
/// 12. emit `SubtitleDone { id, cue_count }`
///
/// **错误降级**：任一步失败 → emit `SubtitleFailed { id, error }` + 返回 Err。
/// VAD 分段完全为空（无声）→ cues 为空 + srt_text=""（不报错，前端显示「无字幕」）。
///
/// **engine_manager 参数**：Tauri 2 要求 `State` 在前。`Arc<AsrEngineManager>` 在
/// `main.rs:1051` 通过 `app.manage` 注入（与 `runtime_config.rs:348` 同模式）。
///
/// **model 名**：用 `octopus_asr_local::config::resolve_active_engine("asr")`（与
/// `main.rs:980` 启动预热同模式），返回 `Result<ResolvedEngine>`，取 `.name`。
#[command]
pub async fn generate_subtitle(
    app: AppHandle,
    engine_manager: State<'_, std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>>,
    id: i64,
    track: Option<String>,
    polish: Option<crate::subtitle_polish::PolishOption>,
) -> Result<octopus_record::SubtitleResult, String> {
    // 内层 runner：返回 Result，外层 catch 后 emit SubtitleFailed。
    // 这样所有失败路径统一走一处 emit，避免漏发。
    match generate_subtitle_inner(&app, &engine_manager, id, track, polish).await {
        Ok(r) => Ok(r),
        Err(e) => {
            let _ = app.emit(
                "record://task",
                RecordTaskEvent::SubtitleFailed { id, error: e.clone() },
            );
            Err(e)
        }
    }
}

/// `generate_subtitle` 的实际编排（不带错误 emit，由外层统一处理）。
async fn generate_subtitle_inner(
    app: &AppHandle,
    engine_manager: &std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>,
    id: i64,
    track: Option<String>,
    polish: Option<crate::subtitle_polish::PolishOption>,
) -> Result<octopus_record::SubtitleResult, String> {
    use octopus_infra::paths::resolve_recording_path;

    log::info!("[subtitle] generate_subtitle_inner 开始 id={}", id);
    let _ = app.emit("record://task", RecordTaskEvent::SubtitleStarted { id });

    // 1. 查 DB 拿 RecordingMeta（连同 file_path 一起，省去第二次查 DB）
    let meta = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        store.get(id)?.ok_or(RecordError::NotFound(id))
    })
    .await?;
    log::info!("[subtitle] step1 查 DB 完成 file_path={} audio_tracks={}",
        meta.file_path, meta.audio_tracks.len());

    // 2. 解析 TrackPreference
    let pref = match track.as_deref() {
        Some("system") => octopus_record::TrackPreference::System,
        Some("microphone") => octopus_record::TrackPreference::Microphone,
        _ => octopus_record::TrackPreference::Auto,
    };

    // 3. 选轨
    let (track_idx, track_used) =
        octopus_record::select_track(&meta, pref).map_err(|e| e.to_string())?;
    log::info!("[subtitle] step3 选轨完成 track_idx={} track_used={:?}", track_idx, track_used);

    // 4. emit 进度：抽音轨
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::SubtitleProgress {
            id,
            stage: octopus_record::SubtitleProgress::ExtractingAudio { percent: 10 },
        },
    );

    // 5. ffmpeg 抽 PCM（spawn_blocking 包裹——ffmpeg 跑数秒，避免阻塞 tokio runtime）
    let ffmpeg = find_ffmpeg().await?;
    let input = resolve_recording_path(&meta.file_path);
    if !input.exists() {
        return Err(format!("源文件不存在: {}", input.display()));
    }
    log::info!("[subtitle] step5 开始抽 PCM mp4={} ffmpeg={}", input.display(), ffmpeg.display());
    let mp4_path_clone = input.clone();
    let pcm = tokio::task::spawn_blocking(move || {
        octopus_record::extract_audio_track_to_pcm(&mp4_path_clone, track_idx, &ffmpeg)
    })
    .await
    .map_err(|e| format!("extract join error: {e}"))?
    .map_err(|e| e.to_string())?;
    log::info!("[subtitle] step5 抽 PCM 完成 samples={} ({:.1}s)", pcm.len(), pcm.len() as f64 / 16000.0);

    // 6. emit 进度：识别中
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::SubtitleProgress {
            id,
            stage: octopus_record::SubtitleProgress::Recognizing { percent: 40 },
        },
    );

    // 7. 拿 active engine（State 注入的 AsrEngineManager）+ PipelineConfig
    let engine = engine_manager
        .active_engine()
        .map_err(|e| format!("获取 ASR 引擎失败: {e}"))?;
    log::info!("[subtitle] step7 拿到 active engine");
    let cfg = octopus_asr_local::pipeline::PipelineConfig::from_app_config("zh");

    // 8. ASR 带时间戳转写（同步阻塞 CPU 密集——但 engine.transcribe 内部已并发，
    //    且通常总耗时 < ffmpeg；为简化暂不再 spawn_blocking，如发现卡 UI 再包）。
    log::info!("[subtitle] step8 开始 ASR transcribe（这一步可能耗时几十秒）...");
    let mut timestamped =
        octopus_asr_local::pipeline::transcribe_segments_with_timestamps(engine.as_ref(), &pcm, &cfg)
            .map_err(|e| format!("ASR 失败: {e}"))?;
    log::info!("[subtitle] step8 ASR 完成 segments={}", timestamped.len());

    // 8.5. LLM 润色（可选，polish=Some 时触发）
    //     整段润色（保留 [[N]] 标记边界）→ 拆回 cue → 文本回填 timestamped（时间戳不变）。
    //     失败降级（NoLlmConfig/Failed/FallbackRatio）走 polish_subtitle_cues 内部，
    //     不会让整条字幕流程报错——润色失败 = 用 ASR 原文本 + 提示用户。
    let polish_outcome_str: Option<String> = if let Some(polish_opt) = polish.as_ref() {
        let _ = app.emit(
            "record://task",
            RecordTaskEvent::SubtitleProgress {
                id,
                stage: octopus_record::SubtitleProgress::Polishing { percent: 50 },
            },
        );
        let texts: Vec<String> = timestamped.iter().map(|t| t.text.clone()).collect();
        log::info!("[subtitle] step8.5 开始 LLM 润色 cues={}", texts.len());
        let (polished_texts, outcome) =
            crate::subtitle_polish::polish_subtitle_cues(texts, polish_opt, app).await;
        // 文本回填（时间戳不变）；长度一定一致（polish_subtitle_cues 保证）
        for (seg, new_text) in timestamped.iter_mut().zip(polished_texts) {
            seg.text = new_text;
        }
        log::info!("[subtitle] step8.5 润色完成 outcome={:?}", outcome);
        let outcome_str = match outcome {
            crate::subtitle_polish::PolishOutcome::Skipped => None,
            crate::subtitle_polish::PolishOutcome::Polished => Some("polished".into()),
            crate::subtitle_polish::PolishOutcome::FallbackRatio => Some("fallbackRatio".into()),
            crate::subtitle_polish::PolishOutcome::NoLlmConfig => Some("noLlmConfig".into()),
            crate::subtitle_polish::PolishOutcome::Failed(msg) => Some(format!("failed:{msg}")),
        };
        outcome_str
    } else {
        None
    };

    // 9. emit 进度：组装
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::SubtitleProgress {
            id,
            stage: octopus_record::SubtitleProgress::Finalizing { percent: 90 },
        },
    );

    // 10. 转 SubtitleCue + 生成 SRT（文本取自 step8.5 润色后的 timestamped）
    let cues: Vec<octopus_record::SubtitleCue> = timestamped
        .into_iter()
        .map(|t| octopus_record::SubtitleCue {
            start_ms: t.start_ms,
            end_ms: t.end_ms,
            text: t.text,
        })
        .collect();
    let model = octopus_asr_local::config::resolve_active_engine("asr")
        .map(|r| r.name)
        .unwrap_or_else(|_| "unknown".to_string());
    let srt_text = octopus_record::generate_srt(&cues);
    let result = octopus_record::SubtitleResult {
        cues: cues.clone(),
        srt_text: srt_text.clone(),
        model: model.clone(),
        track_used,
        polish_outcome: polish_outcome_str,
    };

    // 11. 写 SRT 文件到磁盘（v2：不存 DB，与 mp4 同目录同名 xxx.N.srt）
    let srt_path = octopus_record::next_srt_path(&input);
    log::info!("[subtitle] step11 写 SRT 文件 {} cues={}", srt_path.display(), cues.len());
    std::fs::write(&srt_path, &srt_text)
        .map_err(|e| format!("写 SRT 文件失败 {}: {e}", srt_path.display()))?;
    log::info!("[subtitle] step11 SRT 文件写入完成");

    // 12. emit done（cues 为空也走 Done——VAD 无声属于「正常无字幕」，不是错误）
    let _ = app.emit(
        "record://task",
        RecordTaskEvent::SubtitleDone { id, cue_count: cues.len() },
    );
    log::info!("[subtitle] step12 emit Done 完成，函数即将返回");

    Ok(result)
}

/// 导出 SRT 文件到指定路径。
///
/// 读取最新字幕（v2：从磁盘 .srt 文件解析，不查 DB）。
///
/// 扫描 mp4 同目录的 `<stem>.N.srt`，取 N 最大的（最新版本）→ 解析为 SubtitleResult。
/// 不存在 → None（前端显示「生成字幕」按钮）。
/// `track_used` 从 audio_tracks 第一条 source 推（仅前端 fallback 提示用）。
#[command]
pub async fn read_subtitle(
    id: i64,
) -> Result<Option<octopus_record::SubtitleResult>, String> {
    // 查 DB 拿 file_path + audio_tracks（用于解析 mp4 路径 + track_used 推断）
    let meta_opt: Option<octopus_record::RecordingMeta> = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        Ok(store.get(id)?)
    })
    .await?;
    let meta = match meta_opt {
        Some(m) => m,
        None => return Ok(None),
    };
    let mp4 = octopus_infra::paths::resolve_recording_path(&meta.file_path);
    let srt_path = match octopus_record::latest_srt_path(&mp4) {
        Some(p) => p,
        None => return Ok(None),
    };
    let srt_text = std::fs::read_to_string(&srt_path)
        .map_err(|e| format!("读 SRT 文件失败 {}: {e}", srt_path.display()))?;
    let cues = octopus_record::parse_srt(&srt_text);
    let track_used = meta
        .audio_tracks
        .first()
        .map(|t| t.source)
        .unwrap_or(octopus_record::AudioTrackSource::Unknown);
    // model 字段用 srt 文件名占位（v2 不再持久化 model 名——序号即版本）
    let model = srt_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(Some(octopus_record::SubtitleResult {
        cues,
        srt_text,
        model,
        track_used,
        polish_outcome: None,
    }))
}

/// 在 Finder 显示录屏对应的最新 SRT 文件（v2：替代 export_subtitle）。
///
/// 找最新 .srt 文件，`open -R` 在 Finder 高亮。不存在 → Err。
#[command]
pub async fn reveal_subtitle(id: i64) -> Result<String, String> {
    let meta = with_db_blocking(move |conn| {
        let store = RecordStore::new(conn);
        let meta = store.get(id)?.ok_or(RecordError::NotFound(id))?;
        Ok::<_, RecordError>(meta.file_path)
    })
    .await?;
    let mp4 = octopus_infra::paths::resolve_recording_path(&meta);
    let srt_path = octopus_record::latest_srt_path(&mp4)
        .ok_or_else(|| "字幕未生成".to_string())?;
    let status = tokio::process::Command::new("open")
        .arg("-R")
        .arg(&srt_path)
        .status()
        .await
        .map_err(|e| format!("open -R 失败: {e}"))?;
    if !status.success() {
        return Err(format!("open -R 退出码非 0: {}", srt_path.display()));
    }
    Ok(srt_path.to_string_lossy().to_string())
}

// ── 字幕 LLM 润色：列出可用 LLM（弹框下拉填充）──────────────────

/// LLM 下拉选项（前端弹「润色」对话框时填 select）。
///
/// - `key`：`{provider}:{model_name}`（如 `openai:gpt-4o`），传回 `generate_subtitle` 的
///   `polish.llmKey` 字段（subtitle_polish 暂按默认 LLM 走，key 仅记录用户选择）。
/// - `label`：`{model_name} ({Provider})`（如 `GPT-4o (Openai)`），provider 首字母大写。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmOption {
    pub key: String,
    pub label: String,
}

/// 列出可用 LLM（弹框下拉填充）。
///
/// 查 `models` 表 `domain='llm' AND is_available=1`（文件就绪/配置完整即列出，
/// 不要求 is_enabled——用户可选用任一已配置的 LLM 润色）。
/// 按 `model_name` 字母序排序，方便前端展示。
#[command]
pub async fn list_subtitle_llms() -> Result<Vec<LlmOption>, String> {
    with_db_blocking(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT provider, model_name FROM models WHERE domain='llm' AND is_available=1 ORDER BY model_name",
        )?;
        let rows = stmt.query_map([], |r| {
            let provider: String = r.get(0)?;
            let model_name: String = r.get(1)?;
            let key = format!("{}:{}", provider, model_name);
            let label = format!("{} ({})", model_name, capitalize(&provider));
            Ok(LlmOption { key, label })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    })
    .await
}

/// 首字母大写（用于 LLM label 的 provider 显示：`openai` → `Openai`）。
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：调用方显式传入非空设备名时，直接返回（trim 空白），不读 DB。
    /// 这是 RecordConfig UI 未来加设备选择器后的路径——用户主动选的设备优先。
    #[test]
    fn resolve_mic_device_name_explicit_non_empty_wins() {
        assert_eq!(resolve_mic_device_name(Some("  UGREEN USB MIC  ")), Some("UGREEN USB MIC".to_string()));
    }

    /// 回归：显式传入空字符串 / 纯空白 → 视为 None，走 DB 回退路径（不返回空串）。
    /// 前端可能传 `""`（空串）而非 `null`，需正确识别为「未指定」。
    /// 注意：空串会 fallback 到 DB——若 DB 有 microphone 配置则返回该值（这是预期行为）。
    /// 本测试仅验证不返回空串/纯空白（避免 helper 收到 deviceName="" 当作有效设备名）。
    #[test]
    fn resolve_mic_device_name_explicit_empty_does_not_return_empty() {
        // 无论 DB 是否有配置，空串输入都不应直接返回空串
        let result1 = resolve_mic_device_name(Some("   "));
        let result2 = resolve_mic_device_name(Some(""));
        assert!(result1.is_none() || !result1.as_deref().unwrap_or("").trim().is_empty(),
            "空串输入不应返回空串/纯空白设备名");
        assert!(result2.is_none() || !result2.as_deref().unwrap_or("").trim().is_empty(),
            "空串输入不应返回空串/纯空白设备名");
    }

    /// 回归：explicit=None 时走 DB 三级回退（record_microphone_device → microphone）。
    /// 这是 RecordConfig UI 当前的实际路径（前端发 device_name=null）。
    /// 此测试需要 ~/.octopus/octopus.db 存在——用 `cargo test -- --ignored` 手动跑。
    #[test]
    #[ignore = "需要 ~/.octopus/octopus.db 全局 DB（CI 环境无）；本地手动验证用 --ignored"]
    fn resolve_mic_device_name_falls_back_to_asr_config() {
        // 用户 DB 里 microphone = "UGREEN USB MIC-CM769"（ASR 配的）
        let result = resolve_mic_device_name(None);
        assert!(result.is_some(), "应回退到 ASR microphone 配置，不应返回 None");
        // 不强断言具体设备名（因机器而异），只验证回退逻辑生效
    }
}
