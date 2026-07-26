# 录屏功能 — 功能点分解（D-Swift 选型）

> **本文档定位**：基于已确认的 **D-Swift 选型**（复用 openscreen 的原生 helper 子进程架构），列出 octopus 需要实现的功能点及其实现方式。这是 spec 草稿的前置工作——先把"做什么 + 怎么做"梳理清楚，再正式走 `superpowers:brainstorming` → spec → plan。
>
> **选型决策回顾**：D-Swift = 复用 openscreen 的 Swift helper（`openscreen-screencapturekit-helper` 673 行 + `openscreen-macos-cursor-helper` 352 行）作为 octopus 的 sidecar 二进制，主进程（Rust + Tauri 2）通过 JSON-over-stdio 协议调度。打包体积增加 ~2MB（可接受）。

---

## 0. 选型带来的架构定型

```
┌─────────────────────────────────────────────────────────────────┐
│ octopus 主进程（Rust + Tauri 2，crates/desktop）                  │
│  ├─ record_commands.rs     新增 Tauri 命令（start/stop/pause/...）│
│  ├─ record_session.rs      Helper 进程生命周期 + JSON-stdio 协议  │
│  ├─ record_store.rs        录屏元数据入库（recordings 表）        │
│  └─ record_window.rs       录制控制浮窗（参考 overlay_window）    │
└──────────────┬──────────────────────────────────────────────────┘
               │ std::process::Command::new(helper_path)
               │   .arg(JSON_CONFIG)
               │   .stdin(pause/resume/stop 命令)
               │   .stdout(JSON 事件流)
               ▼
┌──────────────────────────────────────────────────────────────────┐
│ helper 子进程（独立二进制，随 app 打包，~2MB）                      │
│  ├─ octopus-sck-helper     录屏（fork openscreen，改名/定制）      │
│  │     SCStream + AVAssetWriter → 直接写 MP4 到 ~/.octopus/...    │
│  └─ octopus-cursor-helper  光标采样（fork openscreen，可选阶段）   │
│        CGEvent tap + NSCursor → JSON 协议（为后期编辑预留）        │
└──────────────────────────────────────────────────────────────────┘
```

**关键边界**：
- helper 是**独立编译产物**（Swift Package，`swift build -c release`），不进 Cargo workspace
- 主进程通过 `std::process::Command` 调用，**不链接 Swift 运行时**
- helper 二进制路径通过 Tauri `resourceDir` / `app_handle.path().resource_dir()` 解析（参考 `seeds_dir()` 模式）

---

## 1. 核心功能点清单（按优先级分层）

### P0 — MVP（最小可用录屏）

| # | 功能点 | 实现方式 |
| --- | --- | --- |
| **F1** | Helper 二进制获取与打包 | 见 §2.1（vendor openscreen helper + Tauri resources 配置 + DMG 脚本扩展）|
| **F2** | 屏幕录制权限请求 | 见 §2.2（首次启动检测 + 引导用户授权，复用 openscreen 的 `CGPreflightScreenCaptureAccess` 模式）|
| **F3** | 全屏录制（display capture） | 见 §2.3（启动 helper，传 display 配置，helper 内部 SCStream + AVAssetWriter 写 MP4）|
| **F4** | 开始/停止/暂停/恢复控制 | 见 §2.4（4 个 Tauri 命令 + stdin 写 pause/resume/stop）|
| **F5** | 录制状态实时回推（时长/文件大小） | 见 §2.5（helper stdout 事件流 → Tauri event → 前端）|
| **F6** | 录屏元数据入库 | 见 §2.6（新增 `recordings` 表，仿 clipboard_history）|
| **F7** | 录屏历史列表 UI | 见 §2.7（前端页面，复用 clipboard 列表组件模式）|
| **F8** | 录屏快捷键 | 见 §2.8（注册 `CmdOrCtrl+Shift+R`，参考 action_hotkey）|

**MVP 验收标准**：用户按快捷键 → 浮窗显示 → 点开始 → 录制中显示时长 → 点停止 → MP4 落盘到 `~/.octopus/recordings/` → 历史列表可见 → 双击播放。

### P1 — 核心增强（录屏工具完整形态）

| # | 功能点 | 实现方式 |
| --- | --- | --- |
| **F9** | 系统音频内录 | 见 §2.9（helper 配置 `capturesAudio=true`，macOS 13+ 原生支持，无需虚拟驱动）|
| **F10** | 麦克风录制 | 见 §2.10（helper 配置麦克风，独立音轨；需要 `NSMicrophoneUsageDescription`）|
| **F11** | 录制源选择（display/window/区域） | 见 §2.11（前端选择器 + helper 传不同 source type）|
| **F12** | 编码参数（codec/分辨率/fps/码率） | 见 §2.12（前端设置 + helper 配置透传）|
| **F13** | 录屏文件管理（重命名/删除/打开目录） | 见 §2.13（Tauri 命令 + 文件系统操作）|
| **F14** | 录屏浮窗（带暂停/停止按钮） | 见 §2.14（新建 record_control_window，仿 overlay_window）|

### P2 — 差异化能力（octopus 独有，复用既有栈）

| # | 功能点 | 实现方式 |
| --- | --- | --- |
| **F15** | 录屏自动字幕（ASR 转写） | 见 §2.15（从 MP4 抽音轨 → `octopus-asr-local::StreamingEngine` → 生成 SRT）|
| **F16** | 字幕翻译 | 见 §2.16（复用 `octopus-translation`，本地 OPUS-MT 或云端 API）|
| **F17** | 录屏历史全文搜索 | 见 §2.17（`recordings_fts` 表，索引 ASR 转写文本，仿 clipboard_history_fts）|

### P3 — 高级（中长期，openscreen 招牌特性）

| # | 功能点 | 实现方式 |
| --- | --- | --- |
| **F18** | 可编辑光标（隐藏系统光标 + 后期重放） | 见 §2.18（vendor cursor-helper，录屏时 `showsCursor=false`，存储光标轨迹）|
| **F19** | 录后编辑器（修剪/缩放动画） | 见 §2.19（前端时间线 UI + ffmpeg_sidecar 修剪；PixiJS 缩放动画参考 openscreen）|
| **F20** | GIF 导出 | 见 §2.20（ffmpeg_sidecar MP4 → GIF）|
| **F21** | 摄像头画中画 | 见 §2.21（helper 扩展 webcam capture，或 AVFoundation 独立进程）|

---

## 2. 功能点实现方式详解

### §2.1 F1 — Helper 二进制获取与打包

**问题**：openscreen 的 helper 是 Swift Package（`electron/native/screencapturekit/Package.swift`），产物是独立可执行文件。需要：(a) 拿到源码，(b) 改名/定制，(c) 编译，(d) 随 app 打包。

**实现步骤**：

1. **Vendor helper 源码**到 `crates/desktop/native/screencapturekit-helper/`：
   - 拷贝 openscreen 的 `Package.swift` + `Sources/OpenScreenScreenCaptureKitHelper/main.swift`
   - 重命名 product/target：`openscreen-screencapturekit-helper` → `octopus-sck-helper`
   - 改 `emit` 事件前缀：`openscreen` → `octopus`（避免日志混淆）
   - 调整 `RecordingRequest` schema：保留核心字段（source/video/audio/outputs），删除 octopus 不需要的（webcam/cursor 可选）

2. **编译脚本** `scripts/build-sck-helper.sh`：
   ```bash
   cd crates/desktop/native/screencapturekit-helper
   swift build -c release --arch arm64 --arch x86_64  # universal binary
   # 产物：.build/release/octopus-sck-helper
   cp .build/release/octopus-sck-helper ../../../binaries/octopus-sck-helper
   ```

3. **Tauri resources 配置**（`crates/desktop/tauri.conf.json`）：
   ```json
   {
     "bundle": {
       "resources": ["binaries/octopus-sck-helper"],
       "macOS": { "signingIdentity": null }  // 未签名（自用/内测）
     }
   }
   ```
   打包后路径：`octopus.app/Contents/Resources/binaries/octopus-sck-helper`

   > **决策记录：为何选 `Resources/` 而非 `MacOS/`**（2026-07-25 讨论）
   >
   > macOS bundle 有两个候选位置放辅助二进制：`Contents/Resources/`（方式 A）和 `Contents/MacOS/`（方式 B，与主 binary 并列）。**选 A**，理由按重要性排序：
   >
   > 1. **签名简单是决定性因素**——Resources/ 的 helper 作为「资源」被主 bundle 签名覆盖；MacOS/ 的 helper 会被 Gatekeeper 当作「独立可执行文件」单独检查，未签名触发警告。octopus 当前 DMG 是未签名自用版，方式 A 可直接跑。
   > 2. **Tauri 一等支持**——`bundle.resources` 配置 + `app.path().resource_dir()` API 是现成的；方式 B 要绕过 Tauri 用 `externalBin`（会加 target triple 后缀）或 build hook 手动 cp，是和框架对着干。
   > 3. **符合 Apple 惯例**——openscreen / Cap / 1Password 等同类产品都把 helper 放 Resources/。
   > 4. **entitlements 自动继承**——录屏 helper 需要 `screen-capture` / `audio-input` 权限，Resources/ 下 entitlements 跟主 app 走，不用维护两套。
   > 5. **跨平台一致**——`resource_dir()` 是 Tauri 抽象（macOS=Resources/、Linux=/usr/lib、Windows=app 目录），未来若跨平台零迁移；方式 B 只对 macOS 有意义。
   >
   > 方式 B 唯一合适的场景是「helper 需被系统其他进程直接调用（如 launchd 拉起）」；录屏 helper 是被主 app spawn 的子进程，不满足。

4. **DMG 脚本扩展**（`scripts/build-macos-dmg.sh` 追加）：
   ```bash
   # 在 cargo tauri build 之前
   ./scripts/build-sck-helper.sh
   ```

5. **运行时路径解析**（Rust 端）：
   ```rust
   fn helper_path(app: &AppHandle) -> Result<PathBuf> {
       use tauri::Manager;
       let resource_dir = app.path().resource_dir()?;
       let helper = resource_dir.join("binaries").join("octopus-sck-helper");
       if !helper.exists() {
           anyhow::bail!("SCK helper not found at {helper:?}");
       }
       Ok(helper)
   }
   ```

**风险**：
- Swift 编译链依赖（用户开发环境需 `xcode-select --install`，但**打包后的 app 不需要**——helper 已编译为二进制）
- macOS universal binary 需要 Xcode（开发机），但用户机不需要
- 未签名 helper 在 Hardened Runtime + Gatekeeper 下可能被拦——需要在 entitlements 里放行（见 §2.2）

### §2.2 F2 — 屏幕录制权限请求

**问题**：SCK 录屏需要 TCC（Transparency, Consent, and Control）授权。首次启动如果没权限，helper 会直接失败。

**实现步骤**：

1. **Info.plist 添加 UsageDescription**（octopus 当前**没有** Info.plist，需要新建 `crates/desktop/Info.plist`）：
   ```xml
   <?xml version="1.0" encoding="UTF-8"?>
   <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "...">
   <plist version="1.0">
   <dict>
     <key>NSScreenCaptureUsageDescription</key>
     <string>用于屏幕录制功能</string>
     <key>NSMicrophoneUsageDescription</key>
     <string>用于录制麦克风音频（可选）</string>
   </dict>
   </plist>
   ```
   并在 `tauri.conf.json` 的 `bundle.macOS` 里引用（或通过 build.rs 注入）。

2. **Entitlements**（`crates/desktop/octopus.entitlements`，参考 openscreen）：
   ```xml
   <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
   <key>com.apple.security.cs.disable-library-validation</key><true/>
   <key>com.apple.security.device.audio-input</key><true/>
   <key>com.apple.security.device.screen-capture</key><true/>
   ```

3. **首次启动检测**（Tauri 命令 `check_screen_record_permission`）：
   - helper 暴露 `--check-permission` 模式（fork openscreen 的 `CGPreflightScreenCaptureAccess`）
   - 主进程启动 helper 子进程跑一次检查，返回 `{ granted: bool }`
   - 前端首次进入录屏页 / 首次按快捷键时调用，未授权则弹引导对话框（含"打开系统设置"按钮）

4. **授权状态监听**：helper 在录制启动时如果权限被吊销会 emit `error` 事件，主进程转发给前端重新引导。

### §2.3 F3 — 全屏录制

**问题**：启动 helper 子进程，传 display 配置，让它录。

**实现步骤**：

1. **枚举显示器**（Tauri 命令 `list_record_displays`）：
   - 调用 helper 的 `--list-displays` 模式（fork openscreen 的 `SCShareableContent` 遍历）
   - 返回 `[{ id, name, width, height, is_primary }]`

2. **启动录制**（Tauri 命令 `start_recording`）：
   ```rust
   #[tauri::command]
   async fn start_recording(
       state: tauri::State<'_, RecordState>,
       app: tauri::AppHandle,
       config: RecordConfig,  // { source_type, source_id, fps, codec, ... }
   ) -> Result<RecordingStarted, String>
   ```
   主进程构造 JSON config（`RecordingRequest`），spawn helper：
   ```rust
   let mut cmd = tokio::process::Command::new(helper_path);
   cmd.arg(serde_json::to_string(&request)?);
   cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
   let mut child = cmd.spawn()?;
   // 启动 stdout reader task，按行解析 JSON 事件
   // 启动 stdin writer task，接收 pause/resume/stop 命令
   ```

3. **输出路径**：`~/.octopus/recordings/{timestamp}_{id}.mp4`（不进 git sync，体积大）。

### §2.4 F4 — 开始/停止/暂停/恢复

**实现方式**：4 个 Tauri 命令薄封装 record_session：

```rust
#[tauri::command] async fn start_recording(...)  // §2.3
#[tauri::command] async fn stop_recording(state) -> Result<RecordResult> {
    state.session.send_stop().await?;  // stdin 写 "stop\n"
    state.session.wait_exit().await?;  // 等 helper 退出 + 最终事件
    let meta = state.store.finalize(...).await?;  // 入库
    Ok(meta)
}
#[tauri::command] async fn pause_recording(state) -> Result<()> {
    state.session.send("pause\n").await
}
#[tauri::command] async fn resume_recording(state) -> Result<()> {
    state.session.send("resume\n").await
}
```

**关键点**：`stop` 后 helper 会 emit `recording-stopped { screenPath }`，主进程读到后才认为录制结束，避免文件未 flush 就关闭。

### §2.5 F5 — 录制状态实时回推

**实现方式**：helper stdout 事件流 → Tauri event emit → 前端 listen。

```rust
// record_session.rs 里读 stdout 的 task
while let Some(line) = child.stdout.next_line().await? {
    let event: HelperEvent = serde_json::from_str(&line)?;
    match event {
        HelperEvent::Ready => app.emit("record://ready", ())?,
        HelperEvent::Started { width, height, .. } => {
            app.emit("record://started", StartedPayload { width, height })?;
        }
        HelperEvent::Stopped { screen_path } => { ... }
        HelperEvent::Error { code, message } => app.emit("record://error", ...)?,
    }
}
```

**时长/文件大小**：helper 不主动报告，主进程用两个机制补：
- 时长：record_session 记录 `started_at`，前端定时器 tick 计算
- 文件大小：可选——helper 扩展一个 `tick` 事件每秒报告 `recorded_file_size`（fork openscreen 已有的 `recording_output.rs` 里的 `recorded_file_size()` / `recorded_duration()`）

### §2.6 F6 — 录屏元数据入库

**实现方式**：在 `crates/infra/src/db.sql` 追加表，仿 `clipboard_history`：

```sql
-- schema v51
CREATE TABLE IF NOT EXISTS recordings (
    id            INTEGER PRIMARY KEY,       -- 毫秒戳（与 clipboard_history 一致）
    file_path     TEXT    NOT NULL,          -- 相对 ~/.octopus/ 的路径（如 recordings/2026-07-25_143022_123.mp4）
    title         TEXT    NOT NULL DEFAULT '',-- 用户可编辑的标题（默认文件名）
    duration_ms   INTEGER NOT NULL,          -- 时长毫秒
    width         INTEGER NOT NULL,
    height        INTEGER NOT NULL,
    fps           INTEGER NOT NULL,
    codec         TEXT    NOT NULL,          -- 'h264' | 'hevc'
    has_audio     INTEGER NOT NULL DEFAULT 0,-- 是否含音轨
    has_mic       INTEGER NOT NULL DEFAULT 0,
    file_size     INTEGER NOT NULL,          -- 字节
    thumbnail     BLOB,                       -- 首帧缩略图（PNG，240×135 Lanczos resize，仿 image_data.thumb）
    transcript    TEXT,                       -- ASR 转写全文（F15，NULL=未转写）
    srt_path      TEXT,                       -- SRT 文件相对路径（F15）
    is_favorite   INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    deleted_at    TEXT DEFAULT NULL           -- 软删（仿 clipboard_history）
);

CREATE INDEX IF NOT EXISTS idx_rec_created   ON recordings(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_rec_favorite  ON recordings(is_favorite);
CREATE INDEX IF NOT EXISTS idx_rec_deleted   ON recordings(deleted_at);

-- FTS5 全文索引（索引 transcript，F17）
CREATE VIRTUAL TABLE IF NOT EXISTS recordings_fts USING fts5(
    transcript,
    content='recordings',
    content_rowid='id',
    tokenize='trigram'
);
-- 三个 FTS 触发器（ai/ad/au），仿 clipboard_history_fts
```

**schema 升级**：`db.rs` 里 `SCHEMA_VERSION` 从 50 → 51，新增 `upgrade_to_v51()` 函数（参考既有升级函数模式）。

### §2.7 F7 — 录屏历史列表 UI

**实现方式**：前端新增页面，复用 clipboard 列表组件模式。

- 路由：`/recordings`（前端 React Router）
- 组件层级：`RecordingsPage` → `RecordingList` → `RecordingCard`（缩略图 + 标题 + 时长 + 创建时间）
- 数据获取：`invoke('list_recordings', { limit, offset, filter })`
- 操作：双击播放（系统默认播放器 `opener::open`）、右键菜单（重命名/删除/导出/转字幕）

**Tauri 命令**（`crates/desktop/src/record_commands.rs`）：
```rust
#[tauri::command] async fn list_recordings(state, filter: Option<RecordFilter>) -> Result<Vec<Recording>>
#[tauri::command] async fn get_recording(state, id: i64) -> Result<Recording>
#[tauri::command] async fn rename_recording(state, id: i64, title: String) -> Result<()>
#[tauri::command] async fn delete_recording(state, id: i64, permanent: bool) -> Result<()>  // 软删/硬删
#[tauri::command] async fn open_recording_file(state, id: i64) -> Result<()>  // 系统播放器
#[tauri::command] async fn reveal_recording(state, id: i64) -> Result<()>     // Finder 显示
```

### §2.8 F8 — 录屏快捷键

**实现方式**：复用 `tauri-plugin-global-shortcut` + `action_hotkey.rs` 模式。

- 默认快捷键：`CmdOrCtrl+Shift+R`（在 db.sql 的 `app_config` 里加 seed：`('record_shortcut', 'CmdOrCtrl+Shift+R', '录屏快捷键')`）
- 注册流程：参考 `action_hotkey.rs:57` 的 `shortcut.parse()` + `register`
- 触发动作：toggle 录制控制浮窗（不是直接开始录制——让用户选源/参数）

### §2.9 F9 — 系统音频内录

**实现方式**：helper 配置层，**macOS 13+ 原生支持**，无需虚拟音频驱动（这是 SCK 的杀手特性）。

- `RecordingRequest.audio.system.enabled = true`
- helper 内部：`configuration.capturesAudio = true; configuration.sampleRate = 48000; configuration.channelCount = 2;`（fork openscreen `main.swift:387-390`）
- 输出：MP4 第二条音轨（AAC 192kbps）
- 录屏时**排除自身进程音频**：`configuration.excludesCurrentProcessAudio = true`（避免录到自己的提示音，fork openscreen `main.swift:389`）

### §2.10 F10 — 麦克风录制

**实现方式**：helper 配置层，独立音轨。

- `RecordingRequest.audio.microphone.enabled = true`
- helper 内部用 macOS 15+ 的 SCK 原生麦克风捕获（私有 KVC `captureMicrophone`），低版本降级到 `AVCaptureDevice`（fork openscreen `main.swift:392-410`）
- 设备枚举：Tauri 命令 `list_microphones` → helper `--list-microphones` 模式
- 权限：首次启动申请麦克风权限（`AVCaptureDevice.requestAccess(for: .audio)`，fork openscreen `main.swift:321-337`）
- 输出：MP4 第三条音轨（AAC 128kbps）

### §2.11 F11 — 录制源选择

**实现方式**：前端选择器 + helper 传不同 source type。

- `source.type = "display"` → 全屏（F3）
- `source.type = "window"` → 窗口录制（helper 用 `SCContentFilter(desktopIndependentWindow:)`，fork openscreen `main.swift:356-373`）
- `source.type = "area"` → 区域录制（helper 用 `SCContentFilter(display:excludingWindows:)` + `sourceRect` 配置，参考 QuickRecorder `RecordEngine.swift:242-255`）

**前端 UI**：
- display 选择：缩略图列表（用 capx 截每个屏的当前帧作为预览）
- window 选择：窗口列表（helper `--list-windows` 模式，返回 `[{ id, title, app_name, bounds }]`）
- area 选择：复用 octopus 已有的截图选区浮窗（`screenshot_*` capability + capx）

### §2.12 F12 — 编码参数

**实现方式**：前端设置页 + helper 配置透传。

- codec：`h264` / `hevc`（helper 的 `AVVideoCodecKey`）
- 分辨率：原分辨率 / 1080p / 720p（超出则 `scale` filter）
- fps：15 / 30 / 60（helper 的 `minimumFrameInterval`）
- 码率：自动（按分辨率×fps 计算，fork QuickRecorder `RecordEngine.swift:380-394` 的 `targetBitrate` 公式）/ 手动

**设置存储**：`app_config` 表新增 `('record_codec', 'h264', ...)`、`('record_fps', '30', ...)` 等 seed。

### §2.13 F13 — 录屏文件管理

**实现方式**：Tauri 命令 + `std::fs`。

- 重命名：`update recordings set title = ? where id = ?`（不 rename 文件，title 是元数据）
- 软删：`update recordings set deleted_at = ? where id = ?`（仿 clipboard_history 软删回收站）
- 永久删除：`delete from recordings` + `std::fs::remove_file`（双写一致性：先删 DB 行再删文件，失败补偿）
- 打开目录：`opener::open(~/.octopus/recordings/)`

### §2.14 F14 — 录屏控制浮窗

**实现方式**：新建 `record_control_window.rs`，仿 `overlay_window.rs`。

- 窗口属性：always-on-top / skip-taskbar / transparent / fixed position（屏幕右下角）
- 内容：录制中红色圆点 + 时长 + 暂停/停止按钮
- 触发：录制开始时 show，停止时 hide
- capability：`capabilities/default.json` 的 `windows` 数组加 `"record_control_window"`

### §2.15 F15 — 录屏自动字幕（核心差异化）

**实现方式**：复用 `octopus-asr-local::StreamingEngine`，离线本地转写。

```
MP4 文件
  └─ ffmpeg_sidecar 抽音轨 → 16kHz PCM f32
       └─ StreamingEngine::accept_samples(&[f32], ...) → 累积全文
            └─ 按时间切片生成 SRT（每 N 秒一段）
                 └─ update recordings set transcript = ?, srt_path = ?
```

**Tauri 命令**：
```rust
#[tauri::command]
async fn transcribe_recording(
    state: tauri::State<'_, AppState>,
    id: i64,
    model: Option<String>,  // 默认 sensevoice / paraformer / whisper
) -> Result<TranscribeResult>
```

**关键复用**：
- `StreamingEngine` trait（`crates/asr-local/src/streaming_runner.rs:35`）的 `accept_samples / flush / finish` 三个方法正好对应"送音频→拿文本"的循环
- VAD 静音切句（`step_silence` 函数）已有，直接用——它会在静音处插逗号分段，SRT 的句子边界天然形成
- 模型选择：复用 `octopus-cli` 已有的模型下载/管理（`builtin_models.rs`）

**风险**：
- 长录屏（>1 小时）转写耗时——后台异步 + 进度回推（`transcribe://progress` 事件）
- 抽音轨需要 ffmpeg——引入 `ffmpeg_sidecar` 依赖（约 80MB）或要求用户机器有 ffmpeg（dlp crate 已经这么做了，可复用 `crates/dlp/` 的 `get_binary_path("ffmpeg")` 逻辑）

### §2.16 F16 — 字幕翻译

**实现方式**：复用 `octopus-translation` crate（本地 OPUS-MT + 云端 API）。

```rust
#[tauri::command]
async fn translate_recording_transcript(
    state: tauri::State<'_, AppState>,
    id: i64,
    target_lang: String,  // "en" / "ja" / ...
) -> Result<TranslatedTranscript>
```

输出：双语 SRT（原文 + 译文交替），或独立译文 SRT 文件。

### §2.17 F17 — 录屏历史全文搜索

**实现方式**：`recordings_fts` 表（F6 已建）+ 前端搜索框。

```rust
#[tauri::command]
async fn search_recordings(state, query: String) -> Result<Vec<Recording>> {
    // SELECT r.* FROM recordings r
    // JOIN recordings_fts f ON f.rowid = r.id
    // WHERE recordings_fts MATCH ? AND r.deleted_at IS NULL
    // ORDER BY rank
}
```

复用 `clipboard_history_fts` 的 trigram tokenizer（CJK 友好）和触发器模式。

### §2.18 F18 — 可编辑光标（P3，中长期）

**实现方式**：vendor openscreen 的 cursor helper（352 行 Swift），录屏时同步启动。

- 录屏配置：`video.hideSystemCursor = true`（helper 配置层）
- 同步启动 cursor helper：spawn `octopus-cursor-helper`，与 sck helper 并行运行
- cursor helper 输出 JSON 流（CGEvent tap 监听点击 + NSCursor 采样 + SHA256 去重）
- 主进程把 cursor 轨迹存到 `~/.octopus/recordings/{id}.cursor.json`
- **后期重放**：前端编辑器按时间轴渲染光标（PixiJS 或 Canvas）

**分阶段**：
- P3.1：仅录制光标轨迹（不做编辑器），存 JSON 备用
- P3.2：前端时间线编辑器（缩放/隐藏/换样式）
- P3.3：光标特效（点击动画、平滑曲线）

### §2.19 F19 — 录后编辑器（P3）

**实现方式**：前端时间线 UI + ffmpeg_sidecar 修剪 + PixiJS 缩放动画（参考 openscreen）。

- 修剪：ffmpeg `-ss {start} -to {end} -c copy`
- 缩放关键帧：openscreen 的 gsap + PixiJS 模式
- 导出：重新编码（ffmpeg libx264）

### §2.20 F20 — GIF 导出（P3）

> **实现注记（2026-07-26 已实现）**：见 [`2026-07-26-record-gif-export.md`](2026-07-26-record-gif-export.md)。
> 录屏历史列表行加「导出 GIF」按钮（Clapperboard 图标），点击 → 后端 spawn ffmpeg 转 GIF → toast 反馈。
> GIF 自动保存到源 MP4 同目录、同名 `.gif`（`-y` 覆盖）。不打包 ffmpeg（缺失则报错引导 `brew install ffmpeg`）。
> 不用 ffmpeg_sidecar 依赖（裸调 `tokio::process::Command`，复用 dlp 的 `get_binary_path` 逻辑作私有 `find_ffmpeg`）。

**ffmpeg 参数**（spec 原文，已落地）：

```bash
ffmpeg -y -i input.mp4 -vf "fps=15,scale=800:-1:flags=lanczos" -loop 0 output.gif
```

复用 snow-shot 的 GIF 参数模式（`video_record_service.rs` 里的 GIF 分支）。

### §2.21 F21 — 摄像头画中画（P3）

**实现方式**：helper 扩展 webcam capture（fork openscreen 的 `webcam_capture.cpp` 是 Windows 的，macOS 需另写 AVFoundation 版本）。

复杂度较高，可作为独立子项目。

---

## 3. 与 octopus 既有架构的对接清单

| octopus 既有 | 录屏对接方式 | 复用程度 |
| --- | --- | --- |
| `crates/dlp/`（yt-dlp sidecar 模式） | helper 二进制获取与 spawn 模式 | **高**（spawn + JSON 协议参考）|
| `crates/infra/db.sql` + schema 升级 | `recordings` 表 + `recordings_fts` + schema v51 | **高**（直接追加）|
| `crates/asr-local/streaming_runner.rs` | F15 字幕：`StreamingEngine::accept_samples` | **极高**（核心复用）|
| `crates/translation/` | F16 字幕翻译 | **高** |
| `crates/clipboard/`（FTS5 + image_data + 软删） | F6/F7/F13/F17 元数据管理模式 | **高**（结构范本）|
| `crates/capx/`（xcap 截图） | F11 录制源选择的预览图 | 中（display 缩略图）|
| `crates/desktop/src/overlay_window.rs` | F14 录制控制浮窗 | **高**（窗口创建范本）|
| `crates/desktop/src/action_hotkey.rs` | F8 录屏快捷键 | **高**（global-shortcut 注册）|
| `crates/desktop/src/clipboard_commands.rs` | F7 Tauri 命令组织 | **高**（命令风格范本）|
| `crates/desktop/capabilities/default.json` | 新增 record 窗口 + 权限 | 直接修改 |
| `scripts/build-macos-dmg.sh` | helper 打包集成 | 直接扩展 |

---

## 4. 实现路线图（建议）

```
阶段 1：MVP（P0，预计 3-5 天）
  └─ F1 helper 打包 + F2 权限 + F3 全屏录制 + F4 控制 + F5 状态回推
     + F6 入库 + F7 历史列表 + F8 快捷键
  → 交付：可用的全屏录屏（无音频），历史可回看

阶段 2：核心增强（P1，预计 1-2 周）
  └─ F9 系统音频 + F10 麦克风 + F11 源选择 + F12 编码参数
     + F13 文件管理 + F14 控制浮窗
  → 交付：完整的录屏工具（对标 QuickRecorder 基础功能）

阶段 3：差异化（P2，预计 1 周）
  └─ F15 自动字幕（复用 ASR）+ F16 翻译 + F17 全文搜索
  → 交付：octopus 独有的"可搜索的录屏"——这是与 QuickRecorder/Cap 的核心差异

阶段 4：高级（P3，中长期，按需）
  └─ F18 可编辑光标 + F19 编辑器 + F20 GIF + F21 摄像头
  → 交付：对标 Cap/openscreen 的录屏 + 编辑产品形态
```

---

## 5. 待确认问题（已基于 octopus 既有先例给出建议，2026-07-25）

> 勘读 octopus 现状后的关键发现，直接影响以下 5 个问题的答案：
> - **octopus 仓库无 LICENSE 文件**，Cargo.toml 无 license 字段 → 法律上默认 "All rights reserved"（专有/闭源语义）
> - **`crates/dlp/` 已有完整的 GPLv3 隔离先例**（`crates/dlp/docs/architecture.md` §1），用「物理进程隔离 + stdout pipe + Mere Aggregation 边界」论证合规——**这与 D-Swift 的 helper 子进程架构完全同构**
> - **ASR 模型走 HF cache + builtin 兜底**（`builtin_models.rs`：zipformer-small 首次启动下载到 `~/.octopus/models/`，不随 app 打包）

### Q1. Helper 许可证合规

**建议：直接复用 dlp 的「物理进程隔离」论证，新增录屏 helper 的 LICENSE/attribution。**

- openscreen 是 MIT（`openscreen/LICENSE` 头部确认），是**最宽松的 copyleft-free 许可**，与 dlp 的 GPLv3 隔离论证完全兼容
- D-Swift 的 helper 是**独立编译的子进程**，主进程通过 `tokio::process::Command` + stdout pipe 通信——这正是 `crates/dlp/docs/architecture.md:7-29` 已论证过的「Mere Aggregation」边界
- **attribution 具体做法**（MIT 要求保留版权声明）：
  1. helper 源码目录 `crates/desktop/native/screencapturekit-helper/LICENSE`（保留 openscreen 原 LICENSE，附加 octopus 修改声明）
  2. helper 二进制内嵌版本信息（`--version` 输出 `octopus-sck-helper 0.1.0 (based on openscreen MIT, Copyright (c) 2025 Siddharth Vaddem)`）
  3. 全局 `THIRD_PARTY_LICENSES.md`（根目录新增，集中所有第三方组件——dlp 的 yt-dlp/ffmpeg、本次的 openscreen、未来的依赖都列在这里）
- **比 dlp 更宽松**：dlp 是 GPLv3（强 copyleft，必须物理隔离）；openscreen 是 MIT（copyleft-free，理论上可以直接链接进主进程，但我们仍采用子进程架构是**为了解耦与未来跨平台**，不是为了许可证）

### Q2. octopus 自身许可证

**建议：当前维持「无 LICENSE = All rights reserved」的现状，不阻塞录屏功能；未来若决定开源，MIT/Apache-2.0 与 openscreen vendor 完全兼容。**

- octopus 既无 LICENSE 也无 Cargo.toml license 字段，**法律上等于专有软件**（"All rights reserved" by default）
- 这是 dlp 文档里明确表述的立场（`architecture.md:9`：「其他闭源/专有模块」）
- **对录屏功能的影响 = 零**：
  - 若 octopus 维持闭源 → MIT 的 openscreen 仍可 vendor（MIT 允许闭源使用，只需 attribution）
  - 若 octopus 未来开源为 MIT/Apache → 同样兼容（MIT 与 MIT 双向兼容）
  - 若 octopus 未来开源为 GPL → 也兼容（MIT 可纳入 GPL 项目）
- **不需要为了录屏功能先决定 octopus 的许可证**——这是独立的产品决策

### Q3. Helper 维护策略

**建议：初期完全独立维护（vendor 后视为 octopus 自己的代码），不主动跟进 openscreen 上游；只在 openscreen 有重大特性（如新版 macOS API 支持）时手动 cherry-pick。**

理由：
- openscreen 的 helper 与 octopus 的需求**不完全重合**——openscreen 服务于 Electron 生态（JSON schema 含 webcam/cursor 等 octopus MVP 不需要的字段），octopus 需要精简定制版
- D-Swift 选型的核心价值是「**复用 673 行已验证的 SCK+AVAssetWriter 实现**」，不是「持续同步 openscreen 更新」——vendor 后这 673 行就是稳定的基线
- helper 代码量小（673 + 352 = 1025 行 Swift），单人可维护
- **维护策略文档化**：在 `crates/desktop/native/screencapturekit-helper/README.md` 写明「基于 openscreen commit `<sha>` fork，本地修改列表见 git log」——未来 cherry-pick 有据可查
- 参考 snow-shot 的做法：它 fork 了 xcap/scap/device_query，也是「custom/master 分支独立维护」，没跟进上游

### Q4. F15 字幕的模型依赖

**建议：F15 字幕是「可选增强」而非 MVP 强制项——有 ASR 模型就显示「转字幕」按钮，没有就隐藏。MVP（P0）阶段完全不实现 F15。**

理由（基于 `builtin_models.rs` 勘读）：
- octopus 的 builtin 模型策略**已经是「首次启动下载，不随 app 打包」**（zipformer-small 兜底引擎走 `~/.octopus/models/`）
- F15 复用 `StreamingEngine` trait，需要模型已下载——**与 octopus 既有 ASR 流程完全一致**（用户必须先下载模型才能用语音识别，这是已有约定）
- **不在 MVP 强制下载模型**的原因：
  1. 录屏本身不依赖 ASR 模型——用户可能只想要无字幕的录屏
  2. 强制下载会破坏「录屏功能开箱即用」的体验（首启下载几十 MB 模型）
  3. octopus 已有完整的模型管理 UI（下载页 + 进度 + 切换），F15 直接复用即可
- **实现层面**：F15 触发时检查 `check_builtin_models_missing()` / 用户已下载的模型列表，无可用模型则引导去下载页（参考既有 ASR 流程）

### Q5. ffmpeg 依赖

**建议：复用 `crates/dlp/` 的 ffmpeg 获取逻辑（系统 PATH + `~/.octopus/bin/ffmpeg` 回退），不打包进 app。DMG 体积保持 ~40MB 不变。**

理由（基于 `crates/dlp/src/main.rs:143-150` 勘读）：
- **octopus 已有 ffmpeg 检测模式**：先查系统 PATH，再查 `~/.octopus/bin/ffmpeg`（dlp 下载时缓存到这里），都没有则在 stderr 打印平台安装指导
- **录屏 F15 抽音轨复用这条路径**：
  ```rust
  // crates/record/src/ffmpeg.rs（新建）
  pub async fn find_ffmpeg() -> Result<PathBuf> {
      // 复用 crates/dlp 的 get_binary_path 逻辑
      // 或直接调用 octopus-infra 的 binary 查找
  }
  ```
- **不打包 ffmpeg 的理由**：
  1. DMG 体积从 ~40MB → ~120MB（+200%），违背「~2MB helper 增量」的选型承诺
  2. ffmpeg 是 LGPL/GPL 混合许可，打包进 app 会引入新的许可证合规负担（dlp 已论证过的 GPLv3 隔离成本）
  3. octopus 用户群（开发者/效率工具用户）大概率已装 ffmpeg（Homebrew 默认装）
  4. dlp 模块已经为用户提供了 ffmpeg 自动下载到 `~/.octopus/bin/` 的路径——首次用 dlp 下载过视频的用户，ffmpeg 已就位
- **降级体验**：F15 字幕按钮在无 ffmpeg 时禁用 + tooltip 提示「需安装 ffmpeg，点击查看指引」

---

## 5'. 待确认问题（基于以上建议收敛后的最终待决项）

经 Q1-Q5 的分析，**真正需要用户拍板的只剩 2 项**，用户已于 2026-07-25 拍板：

> **架构外溢决策（2026-07-25 brainstorming 补充）**：麦克风权限**应在 app 启动时统一主动申请**，而非录屏模块按需申请。
>
> **背景**：麦克风是跨模块共享权限（ASR / 录屏 / 未来视频会议等子模块都用）。当前 octopus 没有主动申请——依赖 cpal 在第一次打开输入设备时让 macOS 自动弹 TCC 模态框。这是隐式流程，应升级为**显式启动时申请**。
>
> **影响范围**：这是 octopus 跨模块的架构改进，**不属于录屏功能 MVP 范围**。录屏 spec 记录这个决策，但具体实现（启动 hook 申请麦克风权限的 Tauri 命令 + 首启 UI 引导）应作为独立的「权限基础设施」spec 处理。
>
> **录屏 spec 的责任**：依赖这个权限基础设施——录屏模块不再独立申请麦克风权限，假设 app 启动时已申请（或已授权）。录屏 helper 内部仍做权限检查（防御性，避免 helper 启动时权限已过期）。

1. ✅ **`THIRD_PARTY_LICENSES.md` 现在就建**——把 dlp 的 yt-dlp/ffmpeg 也一并补录（dlp 当前只有 docs/architecture.md 提到，没有正式的第三方许可文件）。文件位置：仓库根 `THIRD_PARTY_LICENSES.md`。本次先建文件 + 补录现有依赖，录屏 helper vendor 时再追加 openscreen 条目。

2. ✅ **F15 字幕做灰按钮占位**——MVP（P0）阶段 UI 上显示「转字幕」按钮但禁用，tooltip 提示「需下载 ASR 模型」，点击跳转模型下载页。比完全隐藏更早建立用户认知，也为 F15 实现时省去 UI 改动。F15 实现放在 P2 阶段。

其余 Q1-Q5 的建议均作为既定决策进入 spec：
- Q1 → 复用 dlp 物理进程隔离论证 + 新增 attribution
- Q2 → 维持 octopus 当前「无 LICENSE = ARR」现状，不阻塞
- Q3 → helper vendor 后独立维护，不跟进 openscreen 上游
- Q4 → F15 是可选增强，复用既有 ASR 模型管理流程，不强制下载
- Q5 → 复用 dlp 的 ffmpeg 获取逻辑（系统 PATH + `~/.octopus/bin/`），不打包进 app

---

## 6. 下一步

本文档是功能点**清单**，不是正式 spec。建议流程：

1. 用户确认 §5 的 5 个待确认问题
2. 进入 `superpowers:brainstorming` skill，针对 MVP 阶段（P0）深入设计：
   - helper fork 的具体改动（哪些字段保留/删除/改名）
   - JSON schema 的 octopus 定制版
   - recordings 表的最终 schema
   - 浮窗与历史列表的 UI 设计（触发 `frontend-design` skill）
3. 写 spec：`docs/superpowers/specs/2026-07-25-screen-record-design.md`
4. 写 plan：`docs/superpowers/plans/2026-07-25-screen-record.md`（按 P0 任务分解）
