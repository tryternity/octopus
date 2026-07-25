# 录屏功能调研分析（基于源码勘读的更新版）

> **目的**：在 octopus（Tauri 2 + Rust 桌面应用）里加入屏幕录制能力。本文在第 1 版（仅文档勘读）的基础上，结合本地源码勘读得出更具体的工程判断。
>
> **勘读的源码**：
> - `/Users/wudarui/workspace/agent/screencapturekit-rs/`（sck-rs 完整源 + 24 个示例）
> - `/Users/wudarui/workspace/agent/snow-shot/`（Tauri 2 + 录屏成熟产品，FFmpeg 路线）
> - `/Users/wudarui/workspace/agent/QuickRecorder/`（Swift + SCK + AVAssetWriter 标杆）
> - `/Users/wudarui/workspace/agent/openscreen/`（**Electron + 平台原生 helper 进程**，跨平台 SCK/WGC + 可编辑光标）
>
> **范围**：调研 + 方案对比 + 落地建议。最终设计仍需走 brainstorming → spec → plan。

---

## 0. 第 1 版结论的修正

第 1 版基于 `.tolaria/` 笔记推断「snow-shot 用 scap 录屏」。**勘读源码后这条结论错了**——snow-shot 实际上用 **`ffmpeg_sidecar`（FFmpeg CLI 进程）+ avfoundation/gdigrab** 做录屏，scap 只在截图路径里用（`capture_current_monitor_with_scap`）。这一发现显著改变了路线对比：

| 项目（修正后） | 录屏栈 | 关键发现 |
| --- | --- | --- |
| **snow-shot** | `ffmpeg_sidecar` + 平台原生 input format（macOS `avfoundation` / Win `gdigrab`） | **不用 scap**；FFmpeg CLI 命令构造 1100 行；参数支持 hwaccel / nvenc / amf / crf / scale / crop；分段录制（pause/resume 用 segment）+ 后期 concat |
| **QuickRecorder** | Swift + ScreenCaptureKit + AVAssetWriter + VideoToolbox 硬编 | **CMSampleBuffer 直送 AVAssetWriterInput**（不手动调 VT）；双音轨分离录制 + 后期 `mixAudioTracks` 合成；麦克风可选 `AECAudioStream`（带 AEC 回声消除） |
| **Cap** | Rust 自研 `scap-*`（macOS SCK + Win D3D）+ kameo actor + wgpu | 工程量是商业产品级别（45 crate + 7 fork + vendor wgpu-hal/tao）|
| **openscreen** | Electron 主进程 + **平台原生 helper 子进程**（macOS Swift SCK 673 行 / Windows C++ WGC+WASAPI+MF 2035 行）+ MediaRecorder/WebCodets 备份路径 | **架构同构机会**：Tauri 2 也支持 sidecar，可借鉴 helper 进程模式；**最干净的 SCK + AVAssetWriter 单文件范本**（673 行覆盖 display/window/audio/mic/pause/resume/PTS 重写）|
| **screencapturekit-rs** | 纯 Rust 绑定 SCK | 提供 `SCStream + SCRecordingOutput`（macOS 15+ 直录文件）/ `add_output_handler`（自定义处理 CMSampleBuffer）/ `AsyncSCStream`（async 迭代）三种 API；**不带编码器集成** |

---

## 1. 三条技术路线的源码级对比

### 路线 A：screencapturekit-rs（macOS 原生 SCK）

**真实可用 API（已勘读源码确认）**：

```rust
// 路径 A1：macOS 15+ 直录文件（最少代码、Apple 原生硬编）
let rec_config = SCRecordingOutputConfiguration::new()
    .with_output_url(&output_path)
    .with_video_codec(SCRecordingOutputCodec::H264)  // 或 HEVC
    .with_output_file_type(SCRecordingOutputFileType::MP4);  // 或 MOV
let recording = SCRecordingOutput::new(&rec_config)
    .ok_or("failed to create recording")?;

let mut stream = SCStream::new(&filter, &config);
stream.add_recording_output(&recording)?;   // stream/sc_stream.rs:747
stream.start_capture()?;
// ...
stream.stop_capture()?;
stream.remove_recording_output(&recording)?;
```

```rust
// 路径 A2：自定义处理帧（编码自由度高）
// 用 sync handler
struct Handler;
impl SCStreamOutputTrait for Handler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, ty: SCStreamOutputType) {
        match ty {
            SCStreamOutputType::Screen  => { /* 帧来自 SCK，含 PTS、pixel buffer */ }
            SCStreamOutputType::Audio   => { /* 系统音频 PCM */ }
            SCStreamOutputType::Microphone => { /* 麦克风 PCM（macOS 15+）*/ }
        }
    }
}
// 或用 async API（NextSample / frames / frames_typed）
let stream = AsyncSCStream::new(&filter, &config, 30, SCStreamOutputType::Screen);
while let Some(frame) = stream.next().await { /* CMSampleBuffer */ }
```

**从 QuickRecorder 源码勘读的真实工程要点**（`RecordEngine.swift` + `SCContext.swift`）：

1. **CMSampleBuffer → AVAssetWriterInput.append()** 是 QuickRecorder 的核心数据流（`RecordEngine.swift:598`）。AVAssetWriter 内部自动调用 VideoToolbox 硬编，**不需要手动管理 VTCompressionSession**（只在初始化时探测一次 H264 硬编能力，`RecordEngine.swift:259-283`，给用户「硬编不支持就切 H265」的提示）。
2. **反压机制**：`input.expectsMediaDataInRealTime = true` + `input.isReadyForMoreMediaData` 判定（`RecordEngine.swift:578`）。SCK 单帧掉了就掉了，不能阻塞。
3. **首帧时间戳对齐**：`vW.startSession(atSourceTime: first_pts)`（`RecordEngine.swift:570`）。**第一帧到来才能 startSession**，否则写文件失败。
4. **帧过滤**：必须检查 `SCFrameStatus.complete`（`RecordEngine.swift:564-566`），SCK 会送非完整帧（如 `.idle`/`.suspended`），不滤掉会写坏文件。
5. **暂停/恢复**：用 `timeOffset` 累计偏移 + `adjustTime`（`SCContext.swift:685`）重写后续帧的 PTS。**不是停 stream**，而是给帧改时间戳。
6. **配置坑**：
   - `conf.width = 2; conf.height = 2` 作为初始值（`RecordEngine.swift:162`），随后根据 `filter.contentRect * pointPixelScale` 重算（HiDPI Retina 处理）
   - `minimumFrameInterval = CMTime(value: 1, timescale: frameRate)`（`RecordEngine.swift:224`）；`frameRate >= 60` 时设为 0（不限速，让 SCK 自己决定）
   - `queueDepth = 8`（HDR 录制时，`RecordEngine.swift:212`，参考 Apple WWDC22 10155）
   - `pixelFormat = kCVPixelFormatType_32BGRA` + `colorSpaceName = sRGB`（SDR）；HDR 走 `itur_2100_PQ`
7. **双音轨**：系统音频走 `SCStreamOutputType.audio` → `awInput`；麦克风走 `AVAudioEngine.inputNode.installTap` → `micInput`（`RecordEngine.swift:454-462`）。两条独立 AVAssetWriterInput，**互不干扰**，后期用 `AVMutableComposition` 合成（`SCContext.swift:714`）。
8. **麦克风回声消除（AEC）可选**：`AECAudioStream` 库（`SCContext.swift:36`），`AUVoiceIOOtherAudioDuckingLevel` 控制系统声 ducking 级别。

**音频内录路径的"无驱动"机制**：SCK 的 `capturesAudio = true`（`RecordEngine.swift:217`）+ `sampleRate = 48000` + `channelCount = 2`，直接拿到系统回放的 PCM，**完全不需要 BlackHole 等虚拟音频驱动**。这是笔记里"无驱动音频内录"的真实含义。

**sck-rs 在 octopus 中的对接代价**：
- octopus 是 Rust，**不能直接用 AVAssetWriter**（无成熟 Rust crate 封装 AVFoundation 的 AssetWriter）
- 两条子路径：
  - **A1（推荐 MVP）**：`SCRecordingOutput`（macOS 15+）— Apple 原生，零编码逻辑，但限制是 macOS 15+ 且无中间帧访问（不能做实时预览/水印）
  - **A2**：sck-rs 拿到 `CMSampleBuffer` → 喂给 `ffmpeg_sidecar` 子进程（参考 `examples/19_ffmpeg_encoding.rs`，已验证可行）— 跨 macOS 12.3+，灵活，但要管 ffmpeg 进程
- **许可证**：Apache-2.0/MIT，**无传染风险**

### 路线 B：xcap::VideoRecorder（已依赖）

源码勘读确认了笔记里的"WIP"判断，没有新增信息。结论不变：**短期不适合做主路径**。

### 路线 C：ffmpeg_sidecar（snow-shot 实战路线，第 1 版未列入）

**第 1 版遗漏的重要路线**——snow-shot 1102 行 `video_record_service.rs` 是这条路线最完整的 Tauri 2 参考实现：

```rust
// snow-shot 的真实命令构造（macOS）
FfmpegCommand::new_with_path(&ffmpeg_path)
    .arg("-hwaccel").arg("auto")            // 可选硬件加速
    .arg("-f").arg("avfoundation")          // macOS 平台原生
    .arg("-framerate").arg(frame_rate)
    .arg("-i").arg(format!("{monitor}:{audio_idx}"))  // 视频:音频设备索引
    .arg("-c:v").arg(&encoder)              // libx264 / h264_nvenc / h264_amf / videotoolbox
    .arg("-preset").arg(&preset)            // ultrafast/.../veryslow 或 nvenc p1-p7
    .arg("-vf").arg("crop=W:H:X:Y,scale=...")  // 区域裁剪 + 缩放
    .arg("-crf").arg("23")
    .arg("-pix_fmt").arg("uyvy422")         // macOS 用 uyvy422，Win 用 yuv420p
    .arg("-c:a").arg("aac").arg("-b:a").arg("128k")
    .arg("-filter_complex").arg("[1:a]anlmdn=s=10:p=0.001:r=0.005[aout]")  // 音频降噪
    .arg("-map").arg("0:v").arg("-map").arg("[aout]")
    .arg("-movflags").arg("+faststart")
    .spawn()
```

**snow-shot 的工程实战要点**：
1. **跨平台抽象在 ffmpeg input layer**：macOS `avfoundation` / Win `gdigrab` / Linux `x11grab`，输出格式/编码器统一
2. **硬件加速编码器自动适配**：检测 `nvenc` / `amf` / `videotoolbox`，preset 名称做平台映射（`video_record_service.rs:425-457`）
3. **暂停/恢复用分段录制**：每次 pause 切一个 segment 文件（`segment_001.mp4 / segment_002.mp4 ...`），最后 concat；不是 PTS 重写
4. **macOS 音频设备枚举**：通过 ffmpeg `-list_devices true -f avfoundation -i ""` 输出解析正则（`video_record_service.rs:577-599`）
5. **ffmpeg 二进制分发**：随 app 打包，首次运行 `set_mode(0o755)` 加可执行权限（`video_record_service.rs:108-128`）
6. **GIF 后处理**：录完 MP4 后单独 ffmpeg 命令转 GIF
7. **关键限制**：**macOS avfoundation 无法内录系统音频**——只能录麦克风（除非装 BlackHole 等虚拟驱动）；snow-shot 的 `enable_system_audio` 参数在 macOS 上**实际是注释掉的**（`video_record_service.rs:300-308` 是 Windows dshow 的，macOS 分支没有 system audio 路径）

**路线 C 的代价**：
- ffmpeg 二进制约 80-100MB（macOS universal），随 app 分发显著增大体积
- 子进程 IPC（stdin 喂帧 / stdout 读事件），错误处理复杂
- **macOS 无法内录系统音频**（avfoundation 限制），是硬伤

### 路线 D：原生 helper 子进程（openscreen 实战路线，第 1 版未列入）

**第 1 版因 openscreen 仓库为空遗漏，本次勘读后补全。** openscreen 的设计非常精妙——**把"原生捕获 + 编码"完全剥离成独立子进程**，主进程（Electron）纯 JS 调度，重计算旁路给原生二进制：

```
┌─────────────────────────────────────────────────────────────┐
│ Electron 主进程（JS）                                         │
│  ├─ ipcMain.handle("start-native-mac-recording", ...)        │
│  │     spawn("openscreen-screencapturekit-helper", [JSON])   │
│  │     proc.stdin.write("pause\n" / "resume\n" / "stop\n")   │
│  │     解析 proc.stdout 的 JSON line 事件流                   │
│  └─ MediaRecorder fallback（getUserMedia + WebCodecs）       │
└──────────────┬──────────────────────────────────────────────┘
               │ spawn + JSON-over-stdio 协议
               ▼
┌──────────────────────────────────────────────────────────────┐
│ 平台原生 helper 子进程（独立二进制）                            │
│  ├─ macOS: openscreen-screencapturekit-helper (Swift 673 行)  │
│  │     SCStream + AVAssetWriter → 直接写 MP4                  │
│  │     argv[1] = RecordingRequest JSON                       │
│  │     stdin  = pause/resume/stop 命令流                      │
│  │     stdout = ready/recording-started/paused/stopped/error │
│  ├─ macOS: openscreen-macos-cursor-helper (Swift 352 行)     │
│  │     CGEvent tap + NSCursor 采样 → JSON 协议（去重发送）     │
│  └─ Windows: wgc-capture.exe (C++ 2035 行)                    │
│        WGC + WASAPI loopback + MediaFoundation encoder        │
└──────────────────────────────────────────────────────────────┘
```

**协议设计（极简且全平台统一）**：

```swift
// 启动：argv[1] 是 RecordingRequest JSON
let requestData = Data(CommandLine.arguments[1].utf8)
let request = try decoder.decode(RecordingRequest.self, from: requestData)

// 控制命令流（stdin 读行）
while let line = readLine() {
    switch line.trimmingCharacters(in: .whitespaces) {
    case "pause":  recorder.pause()
    case "resume": recorder.resume()
    case "stop":   await recorder.stop(); exit(0)
    default: break
    }
}

// 事件流（stdout 单行 JSON，主进程按行解析）
emit(["event": "ready"])
emit(["event": "recording-started", "timestampMs": ..., "width": ..., "height": ...])
emit(["event": "recording-paused", "timestampMs": ...])
emit(["event": "recording-stopped", "screenPath": ...])
emitError(code: "writer-failed", message: ...)
```

**openscreen 的工程实战要点**（来自源码勘读）：

1. **最干净的 SCK + AVAssetWriter 单文件范本**（`electron/native/screencapturekit/Sources/OpenScreenScreenCaptureKitHelper/main.swift`，673 行）。覆盖 display/window capture、system audio、native microphone、pause/resume、PTS 重写、错误恢复。**比 QuickRecorder 的 SCContext+RecordEngine 双文件 1600 行更易读**，可作为 Rust 实现的伪代码对照。
2. **隐藏系统光标 + 独立 cursor helper** 实现录后编辑：
   - 录屏时 `configuration.showsCursor = false`（在 SCK config 里），原生光标不入视频
   - 独立 helper 进程高频采样（33ms 间隔）：`CGEvent tap` 监听 leftMouseDown/Up（不拦截），`NSCursor.current` 拿当前光标
   - **SHA256 去重发送**：首次见到的光标形状发完整 base64 PNG，后续只发 `assetId`（避免 stdout 爆炸）
   - 后期编辑器（PixiJS）按时间轴重放光标轨迹，用户可换光标样式/隐藏/放大
3. **MediaRecorder fallback 路径**（`src/hooks/useScreenRecorder.ts`，1686 行）：当 helper 不可用（如旧系统版本/未签名）时降级到 `getUserMedia + MediaRecorder + fix-webm-duration`，分片 chunk 通过 IPC 流式写盘（`recordingStream.ts`，长期录制不爆内存）。**双路径保证可降级**。
4. **跨平台 helper 用同一套协议**：argv[1]=JSON 配置，stdin=命令，stdout=事件流。macOS Swift 和 Windows C++ 的 IPC 协议**完全一致**，主进程无需感知平台差异。
5. ** entitlements 完整模板**（`macos.entitlements`）：除了 V8 必需的 allow-jit/allow-unsigned-executable-memory/disable-library-validation，还有 `device.audio-input`、`device.camera`、`device.screen-capture`——**比 sck-rs 的 22_tauri_app 示例更完整**，直接抄可用。
6. **native bridge 第二层 IPC**（`nativeBridge.ts` + `contracts.ts`）：除了录屏控制，还有 system/project/cursor 三个 domain 的 typed RPC，带 `requestId / version / retryable`，是 Tauri 命令组织的良好参考。
7. **microphone 通过 SCK 原生捕获**（macOS 15+）：`configuration.setValue(true, forKey: "captureMicrophone")` 是私有 KVC，用 `responds(to:)` 探测可用性（`main.swift:603-607`），降级到 `AVCaptureDevice`。比 QuickRecorder 的 `AVAudioEngine` installTap 更优雅。
8. **macOS 13+ 即可用**（不像 sck-rs 的 `SCRecordingOutput` 需要 15+）：openscreen 的 helper 直接用 `SCStream + AVAssetWriter`，绕开了 macOS 15 的限制。
9. **音频双输入独立编码**：system audio (192kbps AAC) 和 microphone (128kbps AAC) 各自一条 `AVAssetWriterInput`，便于后期单独处理。

**路线 D 在 octopus 中的对接代价**：
- octopus 是 Rust，**需要把 openscreen 的 Swift helper 翻译成 Rust**——但 Rust 没有成熟的 AVFoundation AVAssetWriter 封装
- 两条子路径：
  - **D-Rust**：sck-rs `add_output_handler` 拿 CMSampleBuffer → 写自定义 sidecar 进程（Rust + ffmpeg_sidecar）——本质等于路线 A2，但用 helper 进程封装
  - **D-Swift**：直接复用 openscreen 的 Swift helper（673 行已验证）作为 octopus 的 sidecar，Tauri 2 spawn 它——**最快落地但引入 Swift 编译链依赖**
- **关键差异化价值**：openscreen 的**可编辑光标**是其他路线都没有的，若 octopus 想做"录屏后可编辑"产品（不只是录屏工具），这是杀手特性
- **许可证**：MIT，**完全友好**

---

## 2. 修订后的路线推荐

**结合源码勘读的新结论（含 openscreen 路线 D）**：

| 维度 | A1（sck-rs + SCRecordingOutput） | A2（sck-rs + ffmpeg_sidecar） | C（纯 ffmpeg_sidecar） | D-Swift（openscreen helper）|
| --- | --- | --- | --- | --- |
| macOS 版本 | **15.0+** | 12.3+ | 任意 | **13+** |
| 跨平台 | ❌ macOS only | ❌ macOS only | ✅ 三端 | ❌ macOS only（Win 需另写 WGC helper）|
| 系统音频内录 | ✅ macOS 13+ | ✅ macOS 13+ | ❌ macOS avfoundation 不支持 | ✅ macOS 13+ |
| 编码自由度 | ❌ 固定 H264/HEVC | ✅ ffmpeg 全部 | ✅ ffmpeg 全部 | ❌ 固定 H264（AVAssetWriter）|
| 实时帧访问 | ❌（Apple 黑盒） | ✅（拿到 CMSampleBuffer） | ❌（ffmpeg 进程内部） | ❌（helper 内部 AVAssetWriter）|
| 体积开销 | 0（动态链接系统框架） | ~80MB ffmpeg | ~80MB ffmpeg | ~2MB（Swift helper 二进制）|
| 工程复杂度 | **低（10-30 行）** | 中（200-400 行） | 中（参考 snow-shot 1100 行） | 低-中（复用 673 行 Swift）|
| 可编辑光标 | ❌ | ❌ | ❌ | **✅（openscreen 招牌特性）** |
| 与 octopus 既有栈契合 | ✅ 纯 Rust crate | ✅ + 子进程管理 | ⚠️ 与 capx 的 SCK 截图双轨 | ⚠️ 引入 Swift 编译链 |
| 许可证 | Apache-2.0/MIT | + ffmpeg LGPL/MIT | + ffmpeg LGPL/MIT | **MIT（最友好）** |

**修订推荐**：

> **MVP 仍推荐 A1（sck-rs + SCRecordingOutput），但路线 D 是值得关注的中长期选项。**
>
> 理由（基于源码勘读）：
> 1. A1 的代码量极小（sck-rs 的 `SCRecordingOutput` 已封装 Apple 原生硬编 + 文件管理），10-30 行可跑通「点按钮 → 录 MP4」的核心路径。
> 2. A1 自动获得 macOS 13+ 的**无驱动系统音频内录**（snow-shot 走 ffmpeg 在 macOS 上做不到这条）。
> 3. octopus 用户群推断为 macOS（`~/.octopus/` 配置路径），macOS 15+ 占比正在提升（参考 QuickRecorder v1.6.0 后放弃 macOS 12 支持），15.0+ 要求不是阻塞性问题。
> 4. A1 失败时降级到 A2 是平滑升级——同一套 `SCStream` 配置 + filter，只是从 `add_recording_output` 换成 `add_output_handler` + ffmpeg 编码。
> 5. **避免路线 C**：snow-shot 选 ffmpeg 是因为它要跨平台（Win + macOS），octopus 如果只做 macOS，sck-rs 是更优解（音频内录 + 零体积 + GPU 路径）。
>
> **何时转向路线 D**：
> - 若用户想要「录屏后可编辑」（换光标、加缩放动画、修剪、字幕）这种 Cap/ScreenStudio/openscreen 级产品形态，路线 A 不够（A1 拿不到帧、A2 只能编码不能编辑）
> - 若 octopus 想突破「录屏工具」定位，做「屏幕录制 + 后期编辑」一体化，**D-Swift 复用 openscreen 673 行 helper** 是最快路径（自研需 1-2 个月，复用只需 1-2 周适配）
> - 路线 D 的 Swift helper 是**独立的可执行文件**，与 Tauri 2 sidecar 模型契合（参考 `crates/dlp/` 已有的 yt-dlp sidecar 模式）
> - 路线 D 不与 A1 冲突，可作为 A1 之后的「录后编辑」增强阶段

---

## 3. 落地路径（修订版，分 3 阶段）

### 阶段 1：MVP（sck-rs + SCRecordingOutput，2-3 天）

```
crates/record/             # 新 crate（仿 capx 模式，纯逻辑）
├── Cargo.toml             # deps: screencapturekit = { version = "8", features = ["macos_15_0"] }
├── src/
│   ├── lib.rs             # pub struct Recorder { stream: Option<SCStream>, recording: Option<SCRecordingOutput> }
│   ├── config.rs          # RecorderConfig { display_id, width, height, fps, codec, output_path }
│   └── error.rs           # thiserror
└── tests/                 # 单元测试用 mock filter（无真实显示器也能跑）

crates/desktop/
├── src/record_commands.rs # 5 个 Tauri 命令：list_displays / start / stop / pause / status
├── capabilities/default.json  # 加入 record_window
└── Info.plist             # 加 NSScreenCaptureUsageDescription / NSMicrophoneUsageDescription

~/.octopus/recordings/     # 输出目录（不进 git sync）
```

**Tauri 命令签名**（参考 snow-shot `video_record.rs` + sck-rs 的 `22_tauri_app` 示例）：
```rust
#[tauri::command]
async fn record_start(
    state: tauri::State<'_, Mutex<Recorder>>,
    display_id: u32,
    output_path: String,
    fps: Option<u32>,
    codec: Option<String>,  // "h264" | "hevc"
) -> Result<(), String>

#[tauri::command]
async fn record_stop(state: tauri::State<'_, Mutex<Recorder>>) -> Result<RecordResult, String>
```

**Info.plist 模板**（直接抄 snow-shot 的，已验证可用）：
```xml
<key>NSScreenCaptureUsageDescription</key>
<string>用于屏幕录制功能</string>
<key>NSMicrophoneUsageDescription</key>
<string>用于录制麦克风音频</string>
```

**Info.plist 集成**：octopus 已有 `crates/desktop/`，需要找到现有 Info.plist 模板（可能是 `tauri.macos.conf.json` 或 build script 生成）追加这两个键。

### 阶段 2：音频 + 选区 + 自定义编码（sck-rs + ffmpeg_sidecar，1-2 周）

- 升级到 `add_output_handler` 拿到 CMSampleBuffer
- 视频帧 → ffmpeg_sidecar `libx264/videotoolbox` 编码（参考 sck-rs `examples/19_ffmpeg_encoding.rs`）
- 音频帧（`SCStreamOutputType::Audio` + `::Microphone`）→ 独立 ffmpeg audio input
- 支持选区录制（`SCContentFilter::with_display + excludingApplications + exceptingWindows`）
- 支持窗口/应用录制（参考 QuickRecorder `RecordEngine.swift:87-131` 的 filter 构造）
- 摄像头画中画（参考 QuickRecorder `AVContext.swift`，但 octopus 用 Rust 对接 AVFoundation 较难，可后置）

### 阶段 3：编辑与导出（复用 octopus 既有能力，1 周）

- 录屏历史库：复用 `octopus-clipboard` 模式（FTS5 + 缩略图 + 元数据），新增 `recordings` 表
- 字幕：从录屏音轨抽音频 → 复用 `octopus-asr-local` 流式 ASR（Paraformer/SenseVoice）→ 生成 SRT
- 字幕翻译：复用 `octopus-translation`
- 录后修剪：调用 ffmpeg_sidecar `-ss/-to` 命令，无需自研编辑器

---

## 4. 关键工程坑（来自源码勘读，进入 spec 前必须知）

| # | 坑 | 出处 | 规避 |
| --- | --- | --- | --- |
| 1 | **首帧 startSession 对齐** | QuickRecorder `RecordEngine.swift:568-571` | 用 `SCRecordingOutput` 时 Apple 自动处理；A2 路径需手动 `vW.startSession(atSourceTime: first_pts)` 等价逻辑 |
| 2 | **SCFrameStatus.complete 过滤** | QuickRecorder `RecordEngine.swift:564` | A2 路径必须过滤非完整帧；A1 路径 Apple 内部处理 |
| 3 | **HiDPI 像素缩放** | QuickRecorder `RecordEngine.swift:167` | `conf.width = filter.contentRect.width * filter.pointPixelScale`，否则 Retina 屏只录一半像素 |
| 4 | **音频采样率/通道配置** | QuickRecorder `RecordEngine.swift:218` | `sampleRate = 48000; channelCount = 2`，与 ASR 引擎期望匹配 |
| 5 | **CMSampleBuffer PTS 单调** | QuickRecorder `RecordEngine.swift:576` `frameQueue` 去重 | 暂停/恢复后要重写 PTS，否则 writer 报错 |
| 6 | **macOS 屏幕录制权限** | sck-rs README + snow-shot Info.plist | `NSScreenCaptureUsageDescription` 必填；首次拒绝后 SCK 会持续重试（QuickRecorder `SCContext.swift:72-75`）|
| 7 | **`queueDepth` 不要 < 4** | QuickRecorder `RecordEngine.swift:211`（注释） | HDR/4K 录制卡顿；Apple 文档建议 ≤ 8 |
| 8 | **Tauri 2 capabilities** | octopus AGENTS.md 已踩坑多次 | 新窗口加 `windows` 数组；record 控制浮窗别忘了 |
| 9 | **window exclude 自身** | QuickRecorder `SCContext.swift:53` `excludedApps` | 录屏时排除 octopus 自己的浮窗/Dock 图标，否则录到录制按钮 |
| 10 | **ffmpeg 二进制权限** | snow-shot `video_record_service.rs:108` | A2/C 阶段：随 app 打包后需 `set_mode(0o755)`；macOS 还需 notarization 信任 |
| 11 | **麦克风原生捕获用私有 KVC** | openscreen `main.swift:404` | macOS 15+ 才能原生捕获麦克风；用 `responds(to: Selector(...))` 探测后降级到 AVCaptureDevice |
| 12 | **cursor helper SHA256 去重** | openscreen `OpenScreenMacOSCursorHelper/main.swift:266-269` | 录光标时首次见到的形状发完整 PNG，后续只发 assetId，否则 stdout 爆炸 |
| 13 | **Helper 进程的 entitlements** | openscreen `macos.entitlements` | Hardened Runtime 下需要 `device.audio-input` / `device.camera` / `device.screen-capture` 三个 entitlement（不是只 Info.plist 的 UsageDescription）|
| 14 | **MediaRecorder fallback** | openscreen `useScreenRecorder.ts` + `recordingStream.ts` | helper 不可用时必须有降级路径（getUserMedia + WebCodecs），否则旧系统/未签名场景全坏 |
| 15 | **`excludesCurrentProcessAudio`** | openscreen `main.swift:389` | 系统音频捕获时排除自身进程的音频输出，否则录到自己的提示音 |
| 16 | **argv[1] 启动配置 vs stdin 命令流** | openscreen `main.swift:642-660` | 启动配置一次性传（argv），运行时控制流式传（stdin），避免协议状态机复杂化 |

---

## 5. 与 octopus 既有架构的对接（修订）

| octopus 既有能力 | 录屏对接方式 | 复用价值 |
| --- | --- | --- |
| `octopus-capx`（xcap 截图） | 录屏前的"缩略图预览"用 capx 截一张图；**不与 sck-rs 冲突** | 高 |
| `octopus-asr-local`（流式 ASR） | 阶段 3 字幕：从录屏音轨抽 PCM → Paraformer/SenseVoice 流式转写 | **极高**（无需再造 STT）|
| `octopus-asr-cloud`（云端 ASR） | 长录屏可选云端转写（Aliyun/ByteDance/Tencent） | 中 |
| `octopus-clipboard`（FTS5 + image_data） | 录屏历史库同构（缩略图 BLOB + 元数据 + FTS）| 高 |
| `octopus-translation` | 字幕翻译 | 中 |
| `octopus-infra`（DB schema v50） | 新增 `recordings` 表（id/path/duration/width/height/created_at/thumbnail） | 高 |
| `crates/desktop` 浮窗 + 全局快捷键 | 录制控制浮窗（暂停/停止按钮）、`⌘+Shift+R` 快捷键 | 高 |
| `~/.octopus/recordings/` | 录屏文件存储（**不进 git sync**，体积太大） | 新增 |

---

## 6. 待决策问题（修订版，进入 spec 前必须确认）

> 第 1 版的 7 个问题经源码勘读后收敛为 5 个核心问题（含产品形态决策）：

1. **🔴 产品形态（最重要）**：octopus 的录屏是「录屏工具」还是「录屏 + 编辑产品」？
   - 选项 a：**轻量录屏工具**（点按钮 → 录 → 存 MP4 → 选区/窗口/系统音频即可）→ 走 **路线 A1**（最快），录后编辑交给系统播放器或其他工具
   - 选项 b：**录屏 + 后期编辑产品**（录完可修剪、加字幕、加缩放动画、换光标样式）→ 走 **路线 D-Swift**（复用 openscreen helper），否则自研编辑器需 1-2 月
   - 选项 c：**带 ASR 字幕的录屏**（录完用 `octopus-asr-local` 自动转写）→ 走 A1 + 阶段 3 字幕路径，介于 a 和 b 之间
   - **推荐 a**（轻量）作为 MVP，c（字幕）作为第一阶段增强，b（编辑）作为中长期方向——这与 octopus 既有 ASR 能力契合度最高
2. **macOS 版本基线**：是否接受 macOS 15.0+ 作为录屏功能最低版本？
   - 接受 → 走 A1（最简单）
   - 不接受（要支持 13/14）→ 走 A2 或 D（A2 在 macOS 13 也能跑，但工程量翻倍）
3. **音频范围**：v1 是否需要系统音频内录？
   - 需要 → 路线 C 出局（avfoundation 在 macOS 做不到）
   - 不需要（只录画面或只录麦克风）→ 路线选择面更宽
4. **crate 组织**：录屏代码放哪？
   - 选项 a：新建 `crates/record/`（与 capx 解耦，职责清晰）
   - 选项 b：扩展 `crates/capx/`（截图+录屏同 crate，符合 xcap 模式但 capx 当前只依赖 xcap）
   - **推荐 a**：capx 当前依赖 xcap 而非 sck-rs，混用会让 capx 同时背两套平台抽象
5. **存储与同步**：录屏文件是否进 `octopus-sync`（git sync）？
   - **推荐不进**：视频体积大，git sync 不适合；只同步元数据（路径/时长/缩略图），文件留本地
   - 这与 vault（加密文字数据）模式不同，需要 spec 时明确

---

## 7. 参考源码索引（本次勘读）

### screencapturekit-rs（核心参考）

| 文件 | 行数 | 价值 |
| --- | --- | --- |
| `src/recording_output.rs` | 691 | `SCRecordingOutput` 完整 API（A1 路径核心）|
| `src/stream/sc_stream.rs:730-785` | — | `add_recording_output` / `remove_recording_output` 实现 |
| `src/async_api.rs` | 1815 | `AsyncSCStream` + `NextSample` + `frames_typed` |
| `examples/10_recording_output.rs` | 107 | 配置对象演示（**未含实际录制流程**，需结合上面 API）|
| `examples/03_audio_capture.rs` | 98 | 系统音频 + 麦克风捕获模式 |
| `examples/04_pixel_access.rs` | 104 | `CMSampleBuffer` 像素访问（A2 路径基础）|
| `examples/19_ffmpeg_encoding.rs` | 219 | **A2 路径范本**：帧 → ffmpeg stdin |
| `examples/22_tauri_app/src-tauri/src/lib.rs` | 203 | **Tauri 2 集成范本**：list_displays / take_screenshot 命令 |
| `examples/22_tauri_app/src-tauri/Entitlements.plist` | — | Hardened Runtime 最小 entitlements |
| `examples/22_tauri_app/src-tauri/Info.plist` | — | Tauri + SCK Info.plist 模板 |

### snow-shot（路线 C 实战参考）

| 文件 | 行数 | 价值 |
| --- | --- | --- |
| `src-tauri/src/video_record.rs` | 150 | Tauri 命令薄封装（参数列表 + state 管理）|
| `src-tauri/src-crates/app-services/src/video_record_service.rs` | 1102 | **路线 C 完整实现**：ffmpeg 命令构造 / 分段录制 / 设备枚举 / 暂停恢复 |
| `src-tauri/Info.plist` | — | Tauri 2 + 录屏 Info.plist 模板（3 个 UsageDescription 键）|
| `src-tauri/Cargo.toml` | — | workspace 依赖组织（scap / xcap fork 来源）|
| `src-crates/app-utils/src/lib.rs:350-430` | — | scap 截图（**不是录屏**）的真实用法 |

### QuickRecorder（SCK + AVAssetWriter 工程范本）

| 文件 | 行数 | 价值 |
| --- | --- | --- |
| `QuickRecorder/RecordEngine.swift:140-303` | — | **SCK 配置 + 启动**（width/height/frameInterval/HDR/queueDepth）|
| `QuickRecorder/RecordEngine.swift:362-436` | — | **AVAssetWriter 初始化**（H264/H265/bitrate/colorProperties）|
| `QuickRecorder/RecordEngine.swift:489-622` | — | **SCK 帧回调核心**（complete 过滤 / startSession / PTS 重写 / 三种 outputType 分发）|
| `QuickRecorder/RecordEngine.swift:624-631` | — | stream 错误处理 |
| `QuickRecorder/SCContext.swift:17-53` | — | SCK 全局状态字段定义 |
| `QuickRecorder/SCContext.swift:685-700` | — | `adjustTime`（PTS 偏移）|
| `QuickRecorder/SCContext.swift:714-770` | — | `mixAudioTracks`（双音轨合成）|
| `QuickRecorder/SCContext.swift:329-477` | — | `stopRecording` 完整流程 |

### openscreen（路线 D 核心参考）

| 文件 | 行数 | 价值 |
| --- | --- | --- |
| `electron/native/screencapturekit/Sources/OpenScreenScreenCaptureKitHelper/main.swift` | 673 | **最干净的 SCK + AVAssetWriter 单文件范本**：display/window/system audio/microphone/pause/resume/PTS 重写全覆盖；JSON-over-stdio 协议；argv 启动配置 + stdin 命令流 + stdout 事件流 |
| `electron/native/screencapturekit/Sources/OpenScreenMacOSCursorHelper/main.swift` | 352 | **可编辑光标核心**：CGEvent tap 监听点击 + NSCursor 采样 + SHA256 去重发送；这是 openscreen 区别于其他录屏工具的招牌特性 |
| `electron/native/screencapturekit/Package.swift` | — | Swift Package 最小配置（macOS 13+）|
| `electron/native/wgc-capture/src/main.cpp` | 859 | Windows 端等价物：WGC + WASAPI loopback + MF encoder，同 JSON-stdio 协议 |
| `electron/native/wgc-capture/src/wgc_session.cpp` | 315 | Windows Graphics Capture 实现 |
| `electron/native/wgc-capture/src/wasapi_loopback_capture.cpp` | 411 | **WASAPI loopback = Windows 的"系统音频内录"**（macOS 对应 SCK capturesAudio） |
| `electron/native/wgc-capture/src/mf_encoder.cpp` | 450 | Media Foundation 硬编 |
| `electron/ipc/handlers.ts` | 2916 | 主进程 IPC handler 全集；`start-native-mac-recording` (1723) / `pause/resume/stop` / `is-native-*-capture-available`（双路径分发 + cursor 录制协调）|
| `electron/ipc/recordingStream.ts` | 139 | MediaRecorder fallback 路径的流式写盘（分片 chunk 不爆内存）|
| `electron/ipc/nativeBridge.ts` | 239 | system/project/cursor 三 domain 的 typed RPC（带 requestId/version/retryable）|
| `src/native/contracts.ts` | 239 | IPC 契约类型定义（domain/action/payload，可作为 Tauri 命令组织参考）|
| `src/hooks/useScreenRecorder.ts` | 1686 | 前端录屏 hook：native helper / MediaRecorder 双路径选择与降级 |
| `macos.entitlements` | — | **完整 Hardened Runtime entitlements 模板**：allow-jit + allow-unsigned-executable-memory + disable-library-validation + device.audio-input + device.camera + device.screen-capture |
| `package.json` | — | Electron 22 + Vite + Pixi.js 8 + mediabunny（WebCodecs muxer）+ mp4box + gsap（编辑动画）|

**openscreen 在 octopus 借鉴清单**：
- ✅ 直接抄：`macos.entitlements` 模板、JSON-stdio 协议设计、cursor helper 的 SHA256 去重
- ✅ 作为参考代码：673 行 Swift helper 是 Rust 实现的伪代码对照（A2 路径的最佳蓝本）
- ⚠️ 需要翻译：MediaRecorder fallback 在 Tauri 没有 getUserMedia 等价物，需用其他方式降级
- ❌ 不直接用：Electron 主进程的 IPC 模式（Tauri 有自己的命令系统）

---

## 8. 下一步

1. **等用户对第 6 节 5 个待决策问题给方向**（特别是问题 1 产品形态——这决定走 A1 还是 D-Swift）
2. 决策后进入 `superpowers:brainstorming` skill 探讨：
   - 产品形态（轻量工具 vs 录屏+编辑 vs 录屏+ASR 字幕）
   - macOS 版本基线的影响面
   - crate 命名（record vs screen-record vs video-record）
   - 与 capx 的边界（截图永远 xcap？录屏永远 sck-rs？）
   - 是否预留 helper 进程架构（即便 MVP 用 A1，未来升级 D 时不推翻重来）
3. 写 spec：`docs/superpowers/specs/2026-07-25-screen-record-design.md`
4. 写 plan：`docs/superpowers/plans/2026-07-25-screen-record.md`

本文档**仅是调研，不是 spec**，不触发"代码-文档同步"约束。但本文档基于真实源码勘读得出（4 个仓库的源码索引见第 7 节），可直接作为 spec 输入。
