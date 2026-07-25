# 屏幕录制功能 — 设计规格（spec）

> **本 spec 范围**：MVP（P0）阶段——最小可用录屏。基于调研文档（`docs/superpowers/specs/research/2026-07-25-screen-record-survey.md`）和功能点分解文档（`docs/superpowers/specs/2026-07-25-screen-record-features.md`），经 brainstorming 流程确认。
>
> **不在本 spec 范围**：P1（核心增强）、P2（字幕差异化）、P3（编辑器/光标/摄像头）、Windows/Linux 平台 helper 实现。

---

## 0. 决策回顾

### 0.1 选型决策

| 决策项 | 结论 | 来源 |
|---|---|---|
| 录屏技术路线 | **D-Swift**——vendor openscreen macOS Swift helper（673 行 SCK+AVAssetWriter）作为 sidecar 子进程 | 调研文档 §2 |
| Helper 打包位置 | **`Contents/Resources/binaries/`**（方式 A） | features 文档 §2.1 决策记录 |
| Helper 维护策略 | vendor 后独立维护，不跟进 openscreen 上游 | features 文档 Q3 |
| Helper 许可证 | MIT（openscreen），attribution 见 `THIRD_PARTY_LICENSES.md` | features 文档 Q1 |

### 0.2 跨平台策略

octopus 是跨平台产品（mac/win/linux），部分功能用条件编译做平台独特实现。录屏的跨平台策略：

| 层级 | 跨平台性 | MVP 实现 |
|---|---|---|
| **Rust crate**（`crates/record/`）| ✅ 跨平台（spawn helper + JSON 协议） | 完整实现 |
| **Helper 二进制**（`crates/record/native/`）| ❌ 平台专属（macOS Swift / Windows C++ / Linux 未来调研）| 仅 macOS |
| **Tauri 命令** | ✅ 跨平台 | 完整实现 |
| **DB schema** | ✅ 跨平台 | 完整实现 |
| **前端 UI** | ✅ 跨平台 | 完整实现 |

**MVP 只发 macOS 版本**——Windows helper（vendor openscreen C++ `wgc-capture`）作为 P1；Linux helper 待调研（PipeWire/X11 方案）。

### 0.3 MVP 范围确认（brainstorming 阶段）

经 brainstorming 流程确认的 MVP 范围**比调研文档原 P0 更丰满**：

| 维度 | 决策 |
|---|---|
| 音频范围 | **画面 + 系统音频 + 麦克风**（3 条 track）|
| 录制源 | **全屏 + 窗口 + 区域** |
| 入口 | 快捷键 → 配置浮窗 → 点开始 |
| 配置浮窗 | 双态型（默认紧凑 + 高级可展开）|
| 录制控制 | 菜单栏图标 + 下拉 + 快捷键辅助 |
| 历史列表 | 双视图切换（默认网格 + 可切列表）|
| 快捷键 | `Cmd+Shift+R`（呼出/暂停-恢复 toggle）+ `Esc`（停止）|

### 0.4 跨模块依赖（不属于本 spec）

| 依赖项 | 归属 | 说明 |
|---|---|---|
| **麦克风权限启动时统一申请** | 独立「权限基础设施」spec | 跨 ASR/录屏/未来模块共享；当前 octopus 无主动申请（依赖 cpal 自动弹 TCC），需升级为显式启动时申请 |
| 截图选区浮窗复用 | 既有截图模块 | F11 area 录制复用，无需改截图模块 |

---

## 1. 架构总览

### 1.1 进程架构

```
┌─────────────────────────────────────────────────────────────────┐
│ octopus 主进程（Rust + Tauri 2，crates/desktop）                  │
│  ├─ record_commands.rs     Tauri 命令（start/stop/list/...）      │
│  ├─ 调用 crates/record/   跨平台纯逻辑库                           │
│  │   ├─ session.rs         Helper 进程生命周期 + state machine    │
│  │   ├─ protocol.rs        JSON schema（RecordingRequest/Event） │
│  │   ├─ store.rs           录屏元数据入库                         │
│  │   └─ platform/macos.rs  helper 路径解析（条件编译）             │
│  └─ emit("record://event") 事件回推前端                            │
└──────────────┬──────────────────────────────────────────────────┘
               │ std::process::Command::new(helper_path)
               │   .arg(JSON_CONFIG) .stdin(命令) .stdout(事件)
               ▼
┌──────────────────────────────────────────────────────────────────┐
│ helper 子进程（独立二进制，~2MB，打包到 Contents/Resources/）       │
│  └─ octopus-sck-helper    Swift（vendor 自 openscreen，673 行）    │
│      ├─ argv[1] = RecordingRequest JSON（启动配置）                │
│      ├─ stdin  = pause/resume/stop 命令流                         │
│      ├─ stdout = JSON 事件流（ready/started/paused/stopped/error）│
│      └─ SCStream + AVAssetWriter → 直接写文件到 ~/.octopus/...    │
│         ↑ 帧数据在 helper 内部闭环，不经过 IPC                     │
└──────────────────────────────────────────────────────────────────┘
```

**关键设计**：
- **帧数据不经过 IPC**——helper 内部 SCStream → AVAssetWriter 直接写文件，主进程只收发控制信号
- **JSON IO 总量 < 5KB/录制**——性能零瓶颈（详见调研文档「Helper 协议性能分析」）

### 1.2 目录骨架

```
crates/record/                              ← 新 Rust crate（跨平台，纯逻辑）
├── Cargo.toml
├── src/
│   ├── lib.rs                              ← 公共 API
│   ├── session.rs                          ← Helper 进程生命周期 + state machine
│   ├── protocol.rs                         ← JSON schema
│   ├── store.rs                            ← 录屏元数据入库
│   ├── error.rs                            ← RecordError
│   └── platform/
│       ├── mod.rs                          ← HelperProvider trait + provider() 工厂
│       ├── macos.rs                        ← macOSProvider（MVP 实现）
│       ├── windows.rs                      ← WindowsProvider（占位，返回 NotImplemented）
│       └── linux.rs                        ← LinuxProvider（占位，待调研）
└── native/                                 ← 平台 helper 源码（不是 Rust）
    ├── README.md                           ← 来源、版本、修改声明
    └── macos/
        ├── Package.swift                   ← Swift Package
        ├── Sources/OctopusSckHelper/
        │   └── main.swift                  ← vendor 自 openscreen（673 行 + octopus 定制）
        └── LICENSE                         ← openscreen MIT + octopus 修改声明

scripts/
└── build-macos-helper.sh                   ← swift build -c release universal

crates/desktop/
├── src/record_commands.rs                  ← Tauri 命令
├── build.rs                                ← 调 build-macos-helper.sh
├── tauri.conf.json                         ← bundle.resources + Info.plist + entitlements
├── Info.plist                              ← 新增 NSScreenCaptureUsageDescription 等
├── octopus.entitlements                    ← 新增 device.audio-input/screen-capture
├── capabilities/default.json               ← windows 数组加 record_*_window
└── binaries/                               ← gitignored，编译产物
    └── octopus-sck-helper

crates/infra/
└── src/
    ├── db.sql                              ← 追加 recordings / recordings_thumbnails 表
    ├── db.rs                               ← SCHEMA_VERSION 50→51，upgrade_to_v51
    └── paths.rs                            ← 追加 recordings_dir() / resolve_recording_path()

~/.octopus/recordings/                      ← 录屏输出（不进 git sync）
└── 2026-07-25_14.30.22_1773.mp4            ← 文件命名：日期_时间_毫秒戳.mp4
```

---

## 2. Helper 协议（JSON-over-stdio）

### 2.1 传输层

```
主进程 ──argv[1]──→ helper（启动配置，JSON 字符串）
       ←─stdout─── helper（事件流，每行一个 JSON）
       ──stdin───→ helper（命令流，每行一个命令字符串）
       ←─stderr── helper（debug 日志，主进程透传到 logs/record-helper.log）
```

### 2.2 RecordingRequest schema（argv[1]，主进程 → helper）

```rust
// crates/record/src/protocol.rs

#[derive(Serialize, Deserialize)]
pub struct RecordingRequest {
    pub schema_version: u32,           // 协议版本，当前 = 1
    pub recording_id: i64,             // 毫秒戳 id（与 recordings 表主键一致）

    pub source: Source,
    pub video: VideoConfig,
    pub audio: AudioConfig,
    pub outputs: Outputs,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    Display { display_id: u32 },
    Window  { window_id: u32 },
    Area    { display_id: u32, x: i32, y: i32, width: u32, height: u32 },
}

#[derive(Serialize, Deserialize)]
pub struct VideoConfig {
    pub fps: u32,                      // 15 / 30 / 60
    pub width: u32,                    // 主进程算好（含 HiDPI 缩放）
    pub height: u32,
    pub codec: VideoCodec,
    pub bitrate: Option<u32>,          // None = helper 按分辨率×fps 自动算
    pub hide_system_cursor: bool,      // MVP 默认 false；P3 可编辑光标改 true
}

#[derive(Serialize, Deserialize, rename_all = "lowercase")]
pub enum VideoCodec { H264, Hevc }

#[derive(Serialize, Deserialize)]
pub struct AudioConfig {
    pub system: SystemAudioConfig,
    pub microphone: MicrophoneConfig,
}

#[derive(Serialize, Deserialize)]
pub struct SystemAudioConfig {
    pub enabled: bool,
    pub excludes_current_process: bool,   // 避免录到 octopus 自己的提示音
}

#[derive(Serialize, Deserialize)]
pub struct MicrophoneConfig {
    pub enabled: bool,
    pub device_id: Option<String>,        // None = 系统默认麦克风
    pub device_name: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Outputs {
    pub screen_path: String,              // 绝对路径（主进程解析好 ~）
}
```

### 2.3 HelperEvent schema（helper stdout → 主进程）

```rust
#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum HelperEvent {
    Ready { schema_version: u32 },

    RecordingStarted { timestamp_ms: i64, width: u32, height: u32 },
    RecordingPaused  { timestamp_ms: i64 },
    RecordingResumed { timestamp_ms: i64 },
    RecordingStopped { screen_path: String, duration_ms: i64, file_size: u64 },

    Warning { code: String, message: String },
    Error   { code: String, message: String },
}
```

### 2.4 命令（stdin → helper）

```
pause\n     → emit RecordingPaused
resume\n    → emit RecordingResumed
stop\n      → flush 文件 → emit RecordingStopped → exit(0)
```

纯字符串命令（无参数，不需 JSON）。未知命令忽略（向前兼容）。

### 2.5 协议版本协商

- `RecordingRequest.schema_version` 传当前版本（=1）
- `HelperEvent::Ready.schema_version` 回传 helper 支持版本
- 不一致时主进程 emit `Warning { code: "schema-mismatch" }` 并优雅停止

### 2.6 `RecordingStopped` 的字段来源

helper 通过 AVAssetWriter 的 API 获取这些值（参考 openscreen `main.swift` 的 `SCRecordingOutput`/`AVAssetWriter.recordedDuration/recordedFileSize`）：
- `duration_ms`：`writer.asset.duration` 转毫秒
- `file_size`：`fs::metadata(screen_path).len()` 或 `writer.recordedTotalFileSize`
- `screen_path`：与 `RecordingRequest.outputs.screen_path` 一致（回传便于主进程校验）

主进程收到 `RecordingStopped` 后**仍会独立验证文件存在 + 大小 > 0**（不盲信 helper 报告）。

### 2.7 错误码字典

| code | 触发场景 | 主进程反应 |
|---|---|---|
| `permission-denied` | 屏幕录制权限被拒 | 弹引导对话框（跳系统设置）|
| `microphone-unavailable` | 麦克风权限未授权或 macOS < 15 | 降级为只录系统音频，emit Warning |
| `source-not-found` | display_id/window_id 失效 | 提示并停止 |
| `writer-failed` | AVAssetWriter 写文件失败 | 提示具体错误 |
| `capture-stopped-with-error` | SCStream 异常停止 | 提示并尝试保存已录部分 |
| `sample-retime-failed` | 暂停/恢复后 PTS 重写失败 | Warning，不影响录制 |
| `schema-mismatch` | 协议版本不一致 | Warning 并停止 |

---

## 3. Rust crate API（`crates/record/`）

### 3.1 模块结构

```rust
// crates/record/src/lib.rs
pub mod protocol;
pub mod session;
pub mod store;
pub mod error;
mod platform;

pub use protocol::*;
pub use session::{RecordSession, SessionState, StartedInfo, StoppedInfo};
pub use store::{RecordingMeta, RecordStore, ListFilter};
pub use error::{RecordError, RecordResult};
```

### 3.2 RecordSession — 录制会话控制器

```rust
pub struct RecordSession {
    handle: Option<SessionHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle, Starting, Recording, Paused, Stopping,
}

impl RecordSession {
    pub fn new() -> Self;

    pub fn state(&self) -> SessionState;

    /// 启动录制。spawn helper，等到 RecordingStarted 事件后返回。
    pub async fn start(
        &mut self,
        request: RecordingRequest,
        on_event: impl Fn(HelperEvent) + Send + 'static,
    ) -> RecordResult<StartedInfo>;

    pub async fn pause(&mut self) -> RecordResult<()>;
    pub async fn resume(&mut self) -> RecordResult<()>;

    /// 优雅停止：发 stop，等 RecordingStopped，返回文件元数据。
    pub async fn stop(&mut self) -> RecordResult<StoppedInfo>;

    /// 强制 kill（用户取消/helper 卡死）。文件可能损坏。
    pub async fn kill(&mut self) -> RecordResult<()>;
}

pub struct StartedInfo { pub width: u32, pub height: u32 }
pub struct StoppedInfo { pub screen_path: PathBuf, pub duration_ms: i64, pub file_size: u64 }
```

**设计要点**：
- `on_event` 回调让上层自由选择事件分发（emit Tauri event / log），session 本身不耦合 Tauri
- `state()` 只读快照，所有状态变更由 helper 事件驱动
- `kill()` 与 `stop()` 分离：`stop` 优雅（等 helper flush），`kill` 强制（SIGKILL，可能损坏文件）

### 3.3 RecordStore — 元数据入库

```rust
pub struct RecordStore<'a> { conn: &'a rusqlite::Connection }

pub struct RecordingMeta {
    pub id: i64,
    pub file_path: String,          // 相对 ~/.octopus/ 的路径
    pub title: String,
    pub duration_ms: i64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: String,              // "h264" | "hevc"
    pub has_system_audio: bool,
    pub has_microphone: bool,
    pub source_type: String,        // "display" | "window" | "area"
    pub file_size: u64,
    pub has_thumbnail: bool,        // 缩略图懒加载（recordings_thumbnails 表）
    pub is_favorite: bool,
    pub created_at: String,
    pub deleted_at: Option<String>,
}

impl<'a> RecordStore<'a> {
    pub fn new(conn: &'a rusqlite::Connection) -> Self;
    pub fn insert(&self, meta: &RecordingMeta, thumbnail: Option<&[u8]>) -> RecordResult<()>;
    pub fn get(&self, id: i64) -> RecordResult<Option<RecordingMeta>>;
    pub fn list(&self, filter: &ListFilter) -> RecordResult<Vec<RecordingMeta>>;
    pub fn rename(&self, id: i64, title: &str) -> RecordResult<()>;
    pub fn soft_delete(&self, id: i64) -> RecordResult<()>;
    pub fn restore(&self, id: i64) -> RecordResult<()>;
    pub fn permanent_delete(&self, id: i64) -> RecordResult<()>;
    pub fn toggle_favorite(&self, id: i64) -> RecordResult<()>;
    pub fn get_thumbnail(&self, id: i64) -> RecordResult<Option<Vec<u8>>>;
    pub fn list_all_file_paths(&self) -> RecordResult<HashSet<String>>;  // 孤儿清理用
}

pub struct ListFilter {
    pub limit: u32,
    pub offset: u32,
    pub include_deleted: bool,
    pub favorites_only: bool,
}
```

**MVP 不预留** `transcript` / `srt_path` 字段——P2 字幕功能上线时 `ALTER TABLE ADD COLUMN`。

### 3.4 platform 模块 — helper 二进制查找

```rust
// crates/record/src/platform/mod.rs
pub trait HelperProvider: Send + Sync {
    fn resolve_helper_path(&self, app_resource_dir: Option<&Path>) -> RecordResult<PathBuf>;
    fn list_displays(&self) -> RecordResult<Vec<DisplayInfo>>;
    fn list_windows(&self) -> RecordResult<Vec<WindowInfo>>;
    fn list_microphones(&self) -> RecordResult<Vec<MicrophoneInfo>>;
    fn check_permission(&self) -> RecordResult<PermissionStatus>;
    fn request_screen_permission(&self) -> RecordResult<PermissionStatus>;
}

#[cfg(target_os = "macos")]
pub fn provider() -> impl HelperProvider { macOSProvider }
#[cfg(target_os = "windows")]
pub fn provider() -> impl HelperProvider { WindowsProvider }  // 占位
#[cfg(target_os = "linux")]
pub fn provider() -> impl HelperProvider { LinuxProvider }    // 占位
```

- `list_displays` / `list_windows` / `list_microphones` 走 helper `--list-*` 子命令模式（不主进程链接 SCK）
- `request_screen_permission` 走 helper `--request-permission` 模式
- `check_permission` 走 helper `--check-permission` 模式
- Windows/Linux provider 返回 `Err(PlatformNotImplemented)`

### 3.5 RecordError

```rust
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("helper binary not found at {0}")]
    HelperNotFound(PathBuf),
    #[error("helper spawn failed: {0}")]
    SpawnFailed(#[from] std::io::Error),
    #[error("helper error: {code} - {message}")]
    HelperError { code: String, message: String },
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("invalid state: expected {expected}, actual {actual}")]
    InvalidState { expected: SessionState, actual: SessionState },
    #[error("timeout waiting for {event}")]
    Timeout { event: &'static str },
    #[error("platform not implemented: {0}")]
    PlatformNotImplemented(&'static str),
    #[error("not found: recording {0}")]
    NotFound(i64),
    #[error("DB error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type RecordResult<T> = Result<T, RecordError>;
```

### 3.6 crate 依赖

```toml
[package]
name = "octopus-record"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["process", "sync", "io-util", "time", "rt"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
log = "0.4"
rusqlite = { workspace = true }
octopus-infra = { path = "../infra" }
```

**不依赖 Tauri**——纯逻辑库（未来 octopus-cli 也能用，如 `octopus record --display 1`）。

---

## 4. Tauri 命令（`crates/desktop/src/record_commands.rs`）

### 4.1 命令清单

#### A. 源枚举（录制前）

```rust
#[tauri::command] pub async fn list_record_displays() -> Result<Vec<DisplayInfo>, String>;
#[tauri::command] pub async fn list_record_windows() -> Result<Vec<WindowInfo>, String>;
#[tauri::command] pub async fn list_microphones() -> Result<Vec<MicrophoneInfo>, String>;
#[tauri::command] pub async fn check_record_permission() -> Result<PermissionStatus, String>;
#[tauri::command] pub async fn request_screen_record_permission() -> Result<PermissionStatus, String>;
#[tauri::command] pub async fn open_privacy_settings(section: PrivacySection) -> Result<(), String>;
```

#### B. 录制控制（运行中）

```rust
#[tauri::command]
pub async fn start_recording(
    state: tauri::State<'_, Mutex<RecordSession>>,
    app: tauri::AppHandle,
    config: RecordConfig,
) -> Result<StartedInfo, String>;

#[tauri::command]
pub async fn pause_recording(state: tauri::State<'_, Mutex<RecordSession>>) -> Result<(), String>;

#[tauri::command]
pub async fn resume_recording(state: tauri::State<'_, Mutex<RecordSession>>) -> Result<(), String>;

#[tauri::command]
pub async fn stop_recording(
    state: tauri::State<'_, Mutex<RecordSession>>,
    db_state: tauri::State<'_, Mutex<Connection>>,
    discard: bool,                  // true = 丢弃不入库；false = 入库
) -> Result<Option<RecordingMeta>, String>;

#[tauri::command]
pub async fn kill_recording(state: tauri::State<'_, Mutex<RecordSession>>) -> Result<(), String>;
```

#### C. 录屏历史（录制后）

```rust
#[tauri::command] pub async fn list_recordings(db_state, filter: Option<ListFilter>) -> Result<Vec<RecordingMeta>, String>;
#[tauri::command] pub async fn get_recording(db_state, id: i64) -> Result<RecordingMeta, String>;
#[tauri::command] pub async fn get_recording_thumbnail(db_state, id: i64) -> Result<Option<Vec<u8>>, String>;
#[tauri::command] pub async fn rename_recording(db_state, id: i64, title: String) -> Result<(), String>;
#[tauri::command] pub async fn toggle_recording_favorite(db_state, id: i64) -> Result<(), String>;
#[tauri::command] pub async fn delete_recording(db_state, id: i64, permanent: bool) -> Result<(), String>;
#[tauri::command] pub async fn restore_recording(db_state, id: i64) -> Result<(), String>;
#[tauri::command] pub async fn open_recording_file(db_state, id: i64) -> Result<(), String>;
#[tauri::command] pub async fn reveal_recording(db_state, id: i64) -> Result<(), String>;
```

### 4.1.1 时序说明

`start_recording` / `stop_recording` / `pause_recording` / `resume_recording` 都是**异步等待 helper 事件**后才返回：

- `start_recording`：spawn helper → 等 `Ready` → 等 `RecordingStarted` → 返回 `StartedInfo`
- `pause_recording`：stdin 写 `pause\n` → 等 `RecordingPaused` → 返回
- `resume_recording`：stdin 写 `resume\n` → 等 `RecordingResumed` → 返回
- `stop_recording`：stdin 写 `stop\n` → 等 `RecordingStopped`（含 `duration_ms`/`file_size`）→ 等 helper 进程退出 → 入库 → 返回 `RecordingMeta`

事件等待有超时保护（`RecordError::Timeout`），默认 10 秒——避免 helper 卡死时命令永久挂起。

### 4.2 事件回推

```rust
// start_recording 的 on_event 回调内
app.emit("record://event", &event)?;

// 前端
listen<HelperEvent>('record://event', ({ payload }) => {
  switch (payload.event) {
    case 'recording-started': /* ... */; break;
    case 'recording-paused':  /* ... */; break;
    case 'recording-stopped': /* ... */; break;
    case 'error':             /* ... */; break;
    case 'warning':           /* ... */; break;
  }
});
```

**单事件通道**——前端 switch payload.event 分发，简化订阅。

### 4.3 命令注册

```rust
// crates/desktop/src/main.rs invoke_handler 追加
.invoke_handler(tauri::generate_handler![
    // ... 既有命令 ...
    record_commands::list_record_displays,
    record_commands::list_record_windows,
    record_commands::list_microphones,
    record_commands::check_record_permission,
    record_commands::request_screen_record_permission,
    record_commands::open_privacy_settings,
    record_commands::start_recording,
    record_commands::pause_recording,
    record_commands::resume_recording,
    record_commands::stop_recording,
    record_commands::kill_recording,
    record_commands::list_recordings,
    record_commands::get_recording,
    record_commands::get_recording_thumbnail,
    record_commands::rename_recording,
    record_commands::toggle_recording_favorite,
    record_commands::delete_recording,
    record_commands::restore_recording,
    record_commands::open_recording_file,
    record_commands::reveal_recording,
])
.manage(Mutex::new(RecordSession::new()))
```

### 4.4 错误处理

所有命令统一 `Result<T, String>`，把 `RecordError` 转 String：

```rust
.map_err(|e| { log::error!("[record] {e:?}"); e.to_string() })?
```

不暴露内部错误结构给前端，前端按文本内容判断（如包含 "permission denied" 则弹授权引导）。

---

## 5. DB Schema（schema v50 → v51）

### 5.1 `recordings` 表

```sql
CREATE TABLE IF NOT EXISTS recordings (
    id                INTEGER PRIMARY KEY,         -- 毫秒戳
    file_path         TEXT    NOT NULL,            -- 相对 ~/.octopus/ 的路径
    title             TEXT    NOT NULL DEFAULT '',
    duration_ms       INTEGER NOT NULL,
    width             INTEGER NOT NULL,
    height            INTEGER NOT NULL,
    fps               INTEGER NOT NULL,
    codec             TEXT    NOT NULL,            -- 'h264' | 'hevc'
    has_system_audio  INTEGER NOT NULL DEFAULT 0,
    has_microphone    INTEGER NOT NULL DEFAULT 0,
    source_type       TEXT    NOT NULL,            -- 'display' | 'window' | 'area'
    file_size         INTEGER NOT NULL,
    has_thumbnail     INTEGER NOT NULL DEFAULT 0,
    is_favorite       INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT    NOT NULL,
    deleted_at        TEXT DEFAULT NULL
);

CREATE INDEX IF NOT EXISTS idx_rec_created   ON recordings(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_rec_favorite  ON recordings(is_favorite);
CREATE INDEX IF NOT EXISTS idx_rec_deleted   ON recordings(deleted_at);
CREATE INDEX IF NOT EXISTS idx_rec_source    ON recordings(source_type);
```

### 5.2 `recordings_thumbnails` 表（BLOB 分离）

```sql
CREATE TABLE IF NOT EXISTS recordings_thumbnails (
    recording_id INTEGER PRIMARY KEY,           -- = recordings.id（1:1）
    blob         BLOB NOT NULL,                 -- PNG（240×135 Lanczos resize）
    width        INTEGER NOT NULL,
    height       INTEGER NOT NULL,
    created_at   TEXT NOT NULL,
    FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
);
```

分离原因：`list_recordings` 不拖 BLOB，缩略图懒加载（`get_recording_thumbnail`）。

### 5.3 FTS5 全文索引（MVP 不启用，P2 字幕功能上线时追加）

MVP 不建 `recordings_fts` 表——无可索引内容（transcript 字段还没加）。P2 字幕功能上线时一起加。

### 5.4 配置项 seed（`app_config`）

```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
    ('record_shortcut',          'CmdOrCtrl+Shift+R', '录屏快捷键（呼出/暂停-恢复 toggle）'),
    ('record_stop_shortcut',     'Escape',            '停止录屏快捷键'),
    ('record_fps',               '30',                '录屏帧率（15/30/60）'),
    ('record_codec',             'h264',              '录屏编码（h264/hevc）'),
    ('record_resolution',        'original',          '录屏输出分辨率（original/1080p/720p）'),
    ('record_system_audio',      'true',              '默认是否录制系统音频'),
    ('record_microphone',        'false',             '默认是否录制麦克风（false=首启不申请麦克风权限）。注意：false 不代表 MVP 不支持麦克风，只是默认不开启——用户在配置浮窗主动切换时才生效'),
    ('record_microphone_device', '',                  '麦克风设备名（空=系统默认）'),
    ('record_hide_cursor',       'false',             '是否隐藏系统光标（P3 用）'),
    ('record_default_source_type', 'display',         '默认录制源类型'),
    ('record_output_dir',        'recordings',        '输出目录（相对 ~/.octopus/）'),
    ('record_history_view',      'grid',              '历史列表默认视图（grid/list）');
```

### 5.5 升级函数

```rust
const SCHEMA_VERSION: u32 = 51;   // 50 → 51

fn upgrade_to_v51(conn: &Connection) -> Result<()> {
    // db.sql 用 CREATE TABLE IF NOT EXISTS 自动建表
    // app_config seed 用 INSERT OR IGNORE 自动补齐
    // 新表无数据迁移
    log::info!("schema v51: 新增 recordings / recordings_thumbnails 表 + 12 条 app_config seed");
    Ok(())
}
```

### 5.6 与 `clipboard_history` 的一致性对照

| 字段类别 | clipboard_history | recordings |
|---|---|---|
| 主键（毫秒戳）| ✅ | ✅ |
| 内容引用（相对路径/hash）| ✅ | ✅ |
| 类型字段（TEXT enum）| ✅ | ✅ |
| 收藏 / 创建时间 / 软删 | ✅ | ✅ |
| BLOB 分离表 | `image_data` | `recordings_thumbnails` |
| FTS5 表 + 触发器 | ✅ | P2 才加 |

---

## 6. 文件存储与路径策略

### 6.1 目录结构

```
~/.octopus/recordings/
├── 2026-07-25_14.30.22_1773.mp4    ← 录屏文件
├── 2026-07-25_14.30.22_1773.png    ← 缩略图（同名不同后缀）
└── ...
```

**文件命名规则**：`{YYYY-MM-DD_HH.MM.SS}_{recording_id}.{ext}`
- `recording_id` 毫秒戳，保证跨设备唯一
- 时间用 `.` 不用 `:`（避免 macOS Finder 显示问题）

### 6.2 路径解析（`crates/infra/src/paths.rs` 追加）

```rust
pub fn recordings_dir() -> PathBuf { octopus_root().join("recordings") }
pub fn resolve_recording_path(relative: &str) -> PathBuf { octopus_root().join(relative) }
pub fn record_helper_log() -> PathBuf { logs_dir().join("record-helper.log") }
```

主进程算好绝对路径传给 helper（helper 不解析 `~`）。

### 6.3 一致性策略（先文件后 DB + 启动孤儿清理）

**写入顺序**（`stop_recording` 命令内）：
1. helper 写文件完成（emit RecordingStopped）
2. 主进程验证文件存在 + 大小 > 0
3. 抽取缩略图（可选，失败不阻塞）
4. INSERT recordings 行（file_path 为相对路径）
5. INSERT recordings_thumbnails 行（如有缩略图）

**启动时孤儿清理**（`main.rs` setup hook）：
- 扫描 `recordings/` 目录
- 文件不在 DB（孤儿）→ 删除
- 这是预期行为——`recordings/` 是 octopus 私有目录，用户不应手动放文件

### 6.4 删除策略

| 操作 | 行为 |
|---|---|
| 软删（`delete_recording(permanent=false)`）| 只标 `deleted_at`，文件保留（回收站）|
| 物理删除（`delete_recording(permanent=true)`）| 删 DB 行 + 删磁盘文件 + 删缩略图（不可恢复）|
| 物理删除失败 | 文件已部分删除时 DB 行不回滚，用户可再次尝试 |

**物理删除顺序**：先删文件后删 DB（反过来会产生孤儿）。

### 6.5 不进 git sync

录屏文件不出本机，DB 行也不进 `~/.octopus/.sync/`。与 vault/hotword 的 git sync 模式不同（vault 是加密文本，体积小；录屏是视频，体积大，git sync 不适合）。未来若需跨设备访问，走对象存储（S3/R2），不是 git sync。

### 6.6 不做磁盘配额

MVP 不做自动清理。前端显示 `recordings/` 目录总大小，由用户手动管理。未来规则（P1+ 决策）：超 N 天自动软删非收藏 / 总大小超阈值按最旧优先软删。

---

## 7. 权限请求流程

### 7.1 权限触发时机（最小权限原则）

| 权限 | 触发条件 | 申请方 |
|---|---|---|
| **麦克风** | app 启动 | **独立「权限基础设施」spec**（不在本 spec）|
| **屏幕录制** | 用户点"开始录制"且 helper 检测未授权 | 录屏 helper（`--request-permission` 模式）|
| 辅助功能 | MVP 不需要 | — |

**不在启动时申请屏幕录制权限**——避免「装完 app 第一次启动就连环弹窗」。屏幕录制只与录屏相关，按需申请。

### 7.2 屏幕录制权限流程

```
状态                check_permission()      行为
─────────────────────────────────────────────────────────
NotDetermined       NotDetermined           helper --request-permission → 跳系统设置
Granted             Granted                 正常录制
Denied              Denied                  弹引导对话框（"打开系统设置"按钮）
```

- helper `--check-permission` 模式：调 `CGPreflightScreenCaptureAccess()` 返回状态
- helper `--request-permission` 模式：调 `CGRequestScreenCaptureAccess()` 触发系统对话框
- 权限归主 app 的 bundle id（helper 是主 app 子进程）

### 7.3 麦克风权限流程（依赖独立权限基础设施）

录屏 helper **不主动申请麦克风权限**——只检查状态：

```swift
// helper main.swift（vendor 自 openscreen main.swift:321-337 简化）
if request.audio.microphone.enabled {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: break                  // 正常路径
    case .notDetermined:
        // 防御性兜底：理论上 app 启动时已申请
        let granted = await withCheckedContinuation { ... }
        if !granted { emit Warning(...) ; 降级 }
    case .denied, .restricted:
        emit Warning { code: "microphone-unavailable" }
        降级为只录系统音频（不阻塞录制）
    @unknown default: break
    }
}
```

**降级策略**：麦克风是可选增强，拒绝时仍录画面+系统音频，比直接失败好。

### 7.4 配置浮窗的权限状态展示

| 屏幕录制权限 | 配置浮窗展示 |
|---|---|
| `NotDetermined` | 「开始录制」按钮禁用，文案「点击授权屏幕录制权限」，点击触发 `request_screen_record_permission` |
| `Granted` | 按钮正常 |
| `Denied` | 按钮禁用，文案「需要屏幕录制权限」，下方「打开系统设置」链接 |

### 7.5 运行中权限被吊销

录制运行中权限被吊销 → helper SCStream 停止 → emit `Error { code: "capture-stopped-with-error" }` → 主进程强制 `kill_recording` + 尝试保存已录部分 → 前端显示错误（用户重启 app 后才能再次录制）。

### 7.6 Info.plist + Entitlements

#### `crates/desktop/Info.plist`（新建）

```xml
<plist version="1.0">
<dict>
    <key>NSScreenCaptureUsageDescription</key>
    <string>用于屏幕录制功能，捕获屏幕画面</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>用于录制麦克风音频，录屏时同步录制讲解声音</string>
</dict>
</plist>
```

**关键**：`NSScreenCaptureUsageDescription` 必填——缺失会让 helper 调 `CGRequestScreenCaptureAccess` 时 app 被系统终止。

#### `crates/desktop/octopus.entitlements`（新建）

```xml
<plist version="1.0">
<dict>
    <!-- Tauri WKWebView 必需 -->
    <key>com.apple.security.cs.allow-jit</key><true/>
    <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
    <key>com.apple.security.cs.disable-library-validation</key><true/>

    <!-- 录屏 helper 子进程所需的设备访问 -->
    <key>com.apple.security.device.audio-input</key><true/>
    <key>com.apple.security.device.camera</key><true/>
    <key>com.apple.security.device.screen-capture</key><true/>
</dict>
</plist>
```

#### `tauri.conf.json` 引用

```json
{
  "bundle": {
    "macOS": {
      "infoPlist": "Info.plist",
      "entitlements": "octopus.entitlements",
      "signingIdentity": null
    },
    "resources": ["binaries/octopus-sck-helper"]
  }
}
```

---

## 8. UI 设计

### 8.1 配置浮窗（双态型）

```
快捷键 Cmd+Shift+R → 配置浮窗弹出

┌─────────────────────────────────┐
│ 录制设置              高级 ▾    │
│                                 │
│ [全屏] [窗口] [区域]            │  ← 源选择
│ ┌─────────────────────────────┐ │
│ │ ▢  主屏 · 2560×1600         │ │  ← 源预览（display 缩略图）
│ │    30fps · H.264            │ │  ← 参数摘要（小字）
│ └─────────────────────────────┘ │
│                                 │
│ ● 系统声音  ● 麦克风             │  ← 音频开关
│                                 │
│ [开始录制]                      │
└─────────────────────────────────┘

点"高级 ▾"展开完整参数（fps/codec/分辨率/质量下拉）。
默认紧凑，需要时可见。
```

### 8.2 录制控制（菜单栏图标 + 下拉 + 快捷键）

```
菜单栏（始终可见，录屏时显示）
  ● 00:12  ⏺                      ← 红点+时长+图标

点击菜单栏图标 → 下拉面板
┌──────────────────┐
│ ● 录制中   00:12 │
│ [⏸ 暂停]         │
│ [⏹ 停止]         │
└──────────────────┘

快捷键辅助（无需点下拉）：
  Cmd+Shift+R  → 暂停/恢复 toggle
  Esc          → 停止
```

完全不影响桌面布局，录全屏也不会录到（菜单栏在 SCK 捕获范围外）。与 octopus 既有菜单栏图标风格一致。

### 8.3 历史列表（双视图切换）

```
右上角切换按钮
  [▦ 网格] [☰ 列表]              ← 默认网格

网格视图：
┌────────┬────────┬────────┐
│ ▢ 缩略 │ ▢ 缩略 │ ▢ 缩略 │
│ 会议录屏│ 产品演示│ bug 复现│
│ 00:32  │ 12:04  │ 03:21  │
├────────┼────────┼────────┤
│ ▢ 缩略 │ ▢ 缩略 │ ▢ 缩略 │
│ 教程   │ 屏幕共享│ 测试   │
│ 01:15  │ 05:48  │ 00:45  │
└────────┴────────┴────────┘

列表视图：
┌──────────────────────────────────────────┐
│ ▢ 会议录屏         00:32  2026-07-25 14:30│
│ ▢ 产品演示         12:04  2026-07-25 11:22│
│ ▢ bug 复现         03:21  2026-07-24 18:05│
└──────────────────────────────────────────┘

每条录屏右键：[播放] [重命名] [收藏] [删除]
顶部：[搜索框]（MVP 灰禁用，P2 启用 FTS5 全文搜索）
```

### 8.4 字幕按钮占位（F15）

历史列表每条录屏的右键菜单 + 详情页都有「转字幕」按钮：
- **MVP 状态**：禁用（灰色）+ tooltip「需下载 ASR 模型，点击跳转」
- 点击跳转：模型下载页（复用 octopus 既有 ASR 模型管理）
- P2 启用：模型已下载时按钮可点，触发 `transcribe_recording`

---

## 9. MVP 实现边界

### 9.1 MVP 必做清单

| ID | 功能点 |
|---|---|
| F1 | Helper 二进制获取与打包（vendor openscreen Swift → universal binary → `Contents/Resources/binaries/`）|
| F2 | 屏幕录制权限请求（helper `--check-permission` / `--request-permission` 模式）|
| F3 | 全屏录制（display capture）|
| F4 | 开始/停止/暂停/恢复控制（4 个 Tauri 命令 + stdin/stdout）|
| F5 | 录制状态实时回推（helper 事件 → `record://event`）|
| F6 | 录屏元数据入库（schema v51 + `recordings` / `recordings_thumbnails`）|
| F7 | 录屏历史列表 UI（双视图）|
| F8 | 录屏快捷键（`Cmd+Shift+R` + `Esc`）|
| F9 | 系统音频内录（helper 配置 `capturesAudio=true`）|
| F10 | 麦克风录制（helper 配置麦克风，依赖权限基础设施）|
| F11 | 录制源选择（display/window/area，area 复用截图选区浮窗）|

### 9.2 MVP 明确不做

| 推迟到 | 功能点 |
|---|---|
| **P1** | F12 编码参数 UI（MVP 全用 `app_config` 默认值，配置浮窗「高级」是占位）|
| **P1** | F13 完整文件管理（MVP 只做双击播放 + 右键软删，重命名/物理删/P2）|
| **P1** | F14 完整录制控制浮窗（MVP 用菜单栏图标 + 简单下拉）|
| **P1** | Windows helper（vendor openscreen C++ `wgc-capture`）|
| **P2** | F15 ASR 字幕（MVP 灰按钮占位）|
| **P2** | F16 字幕翻译 |
| **P2** | F17 全文搜索（FTS5 表推迟到 F15 一起加）|
| **P2+** | Linux helper（待调研 PipeWire/X11 方案）|
| **P3** | F18 可编辑光标、F19 编辑器、F20 GIF 导出、F21 摄像头 |

### 9.3 MVP 验收标准

1. **首启**：app 启动 → 申请麦克风权限（依赖权限基础设施）→ 用户授权
2. **首次录屏**：按 `Cmd+Shift+R` → 配置浮窗 → 检测屏幕录制权限 `NotDetermined` → 点按钮触发 helper `--request-permission` → 跳系统设置 → 用户授权 → 重启 app
3. **正常录制**：按 `Cmd+Shift+R` → 配置浮窗 → 选「全屏 + 系统音频 + 麦克风」→ 点开始 → 浮窗消失，菜单栏出现红点+时长 → 录 30 秒 → 按 `Cmd+Shift+R` 暂停 → 再按恢复 → 按 `Esc` 停止
4. **入库回看**：录制完成 → MP4 落盘 `~/.octopus/recordings/` → DB 入库 → 前端跳转历史列表 → 双击默认播放器播放
5. **基础管理**：右键删除（软删）→ 回收站
6. **窗口录制**：选「窗口」→ 弹窗口列表 → 选某窗口 → 录制只录该窗口内容
7. **区域录制**：选「区域」→ 复用截图选区浮窗拖框 → 录制只录框内

### 9.4 测试策略（TDD 优先）

| 测试目标 | 测试方式 | 覆盖范围 |
|---|---|---|
| `protocol.rs` | 单元测试：序列化/反序列化往返 | RecordingRequest/HelperEvent 各变体 |
| `session.rs` | 单元测试：mock helper（fake stdin/stdout）驱动 state 转换 | Idle/Starting/Recording/Paused/Stopping 各路径 |
| `store.rs` | 单元测试：内存 SQLite + fixture | insert/list/get/rename/soft_delete/permanent_delete |
| `platform::resolve_helper_path` | 单元测试：mock resource_dir + 开发期 target dir | 路径解析逻辑 |
| Helper 二进制（Swift）| 无法 TDD（无 xctest 栈）| 手动 e2e + openscreen 上游已验证 |
| Tauri 命令层 | 集成测试：mock `RecordSession` trait | 命令参数解析 + 错误返回 |
| 端到端录制 | 手动 e2e（真实显示器 + 真实权限）| 录 30 秒 → 文件存在 → 元数据正确 |

### 9.5 风险与缓解

| 风险 | 缓解 |
|---|---|
| helper 子进程崩溃 | `kill_recording` 兜底 + 启动孤儿清理 + MVP 不做 helper 重启（靠用户重试）|
| 未签名版 TCC 权限重弹（helper 路径变化）| MVP 接受；正式签名版稳定（P1 DMG 签名一起做）|
| HiDPI 分辨率计算错误 | 主进程把 `width/height` 算好后传给 helper（不在 helper 内算）|
| 暂停/恢复后 PTS 不连续 | 完全依赖 openscreen `main.swift:509-571` 已验证实现，不自己重写 |
| Swift 编译链缺失（贡献者机器）| `build-macos-helper.sh` 检测 swift 命令，缺失给清晰错误指引 |
| 录屏文件撑爆磁盘 | MVP 不做磁盘配额；显示目录大小，用户手动清理 |

---

## 10. 跨模块对接清单

| octopus 既有 | 录屏对接方式 | 复用程度 |
|---|---|---|
| `crates/dlp/`（yt-dlp sidecar spawn）| helper spawn + JSON 协议参考 | 高 |
| `crates/infra/db.sql` + schema 升级 | `recordings` 表 + schema v51 | 高 |
| `crates/asr-local/streaming_runner.rs` | F15 字幕（P2）：`StreamingEngine::accept_samples` | 极高 |
| `crates/clipboard/`（FTS5 + image_data + 软删）| 元数据管理模式范本 | 高 |
| `crates/capx/`（xcap 截图）| F11 area 录制复用截图选区浮窗 | 中 |
| `crates/desktop/src/overlay_window.rs` | F14 配置浮窗窗口创建范本 | 高 |
| `crates/desktop/src/action_hotkey.rs` | F8 快捷键 global-shortcut 注册 | 高 |
| `crates/desktop/src/clipboard_commands.rs` | Tauri 命令组织范本 | 高 |
| `crates/desktop/capabilities/default.json` | 新增 record 窗口 + 权限 | 直接修改 |
| `scripts/build-macos-dmg.sh` | helper 打包集成 | 直接扩展 |
| `THIRD_PARTY_LICENSES.md` | openscreen helper vendor 时追加条目 | 直接修改 |

---

## 11. 不变量（Invariants）

实现与重构过程中必须遵守的硬约束：

1. **录制运行中最多只有一个 helper 子进程**——`RecordSession.handle` 同时只能持有一个
2. **`recordings` 表的 `id` 与 `RecordingRequest.recording_id` 一致**——主进程分配后贯穿全链路
3. **`file_path` 永远存相对路径**（相对 `~/.octopus/`）——绝对路径只在运行时 join
4. **helper 二进制路径由主进程解析后传给 helper**——helper 不自己找文件
5. **帧数据不经过 IPC**——SCStream → AVAssetWriter 在 helper 内部闭环
6. **麦克风权限由独立权限基础设施申请**——录屏 helper 只检查不主动申请
7. **`recordings/` 目录是 octopus 私有**——启动时清理孤儿文件，用户不应手动放文件

---

## 12. 降级路径

当 MVP 假设不成立时的应对策略：

| 场景 | 降级 |
|---|---|
| 麦克风权限被拒 | 仍录画面 + 系统音频，emit `Warning { code: "microphone-unavailable" }` |
| macOS < 15（不支持 SCK 原生麦克风捕获）| 同上，降级到 AVCaptureDevice；如仍失败则只录系统音频 |
| helper 崩溃 | `kill_recording` 兜底；已录部分尽可能保留（依赖 helper 的 `finishWriting`）|
| AVAssetWriter 失败（磁盘满/权限）| emit `Error { code: "writer-failed" }`；停止录制，不入库 |
| 屏幕录制权限被吊销（运行中）| emit `Error { code: "capture-stopped-with-error" }`；强制 kill，提示用户重启 |
| helper 二进制缺失（打包错误）| `start_recording` 直接返回 `HelperNotFound` 错误 |
| 协议版本不匹配 | emit `Warning { code: "schema-mismatch" }`，优雅停止 |
| 缩略图抽取失败 | 入库 `has_thumbnail=0`，UI 显示占位图（不阻塞录制成果）|

---

## 13. 参考资料

### 13.1 上游调研文档

- `docs/superpowers/specs/research/2026-07-25-screen-record-survey.md`——4 仓库源码勘读调研（screencapturekit-rs / snow-shot / QuickRecorder / openscreen）
- `docs/superpowers/specs/2026-07-25-screen-record-features.md`——21 功能点分解 + 5 待决策问题（已收敛）
- `THIRD_PARTY_LICENSES.md`——第三方许可清单（openscreen vendor 时补录）

### 13.2 关键源码参考

| 源码 | 价值 |
|---|---|
| `openscreen/electron/native/screencapturekit/Sources/OpenScreenScreenCaptureKitHelper/main.swift`（673 行）| helper 实现蓝本 |
| `openscreen/electron/native/screencapturekit/Sources/OpenScreenMacOSCursorHelper/main.swift`（352 行）| P3 可编辑光标参考 |
| `openscreen/electron/ipc/nativeBridge.ts`（239 行）| IPC 协议设计参考 |
| `openscreen/macos.entitlements` | Hardened Runtime 模板 |
| `QuickRecorder/QuickRecorder/RecordEngine.swift:140-303` | SCK 配置 + 启动参考 |
| `QuickRecorder/QuickRecorder/RecordEngine.swift:489-622` | SCK 帧回调（complete 过滤/startSession/PTS 重写）|
| `snow-shot/src-tauri/src-crates/app-services/src/video_record_service.rs` | ffmpeg CLI 路线参考（路线 C 对照）|
| `screencapturekit-rs/examples/22_tauri_app/` | Tauri 2 + SCK 集成范本 |

### 13.3 octopus 既有范本

| 文件 | 复用方式 |
|---|---|
| `crates/desktop/src/clipboard_commands.rs` | Tauri 命令风格范本 |
| `crates/desktop/src/overlay_window.rs` | 浮窗创建范本 |
| `crates/desktop/src/action_hotkey.rs` | global-shortcut 注册范本 |
| `crates/dlp/src/main.rs` | 外部进程 spawn + JSON IO 范本 |
| `crates/infra/src/db.sql` | DB schema 模式范本 |
| `scripts/build-macos-dmg.sh` | 打包脚本扩展点 |

---

**Spec 结束。下一步：自审 + 用户审查 + 调用 writing-plans skill 生成实施计划。**
