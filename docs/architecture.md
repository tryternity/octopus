# 架构概览

octopus 是一个基于 ONNX Runtime 的语音识别（ASR）工具集，支持多种 ASR 引擎和多种使用方式。

## 项目结构

```
octopus/
├── crates/
│   ├── infra/       # 基础设施 (octopus-infra) — 常量 + octopus_config_home，无项目内依赖
│   ├── asr/         # 核心推理库 (octopus-asr) — 含 db.rs（SQLite：模型配置+识别历史）
│   ├── llm/         # LLM 润色 (octopus-llm)
│   ├── cli/         # 命令行工具 (octopus-cli)
│   ├── server/      # HTTP/WebSocket 服务 (octopus-server)
│   ├── desktop/     # Tauri 桌面应用 (octopus-desktop)
│   └── dlp/         # 模型下载工具 (octopus-dlp)
├── docs/            # 文档
└── usage.md         # 快速使用指南
```

## 模块说明

### octopus-infra（基础设施）

无项目内依赖的最底层 crate，承载跨 crate 共享的基础设施：`consts`（固定路径常量：VAD 模型 / 默认 ASR 模型目录 / 润色 prompt 文件名）+ `paths`（`octopus_config_home()` 返回 `~/.octopus`，三端统一不再各自定义）+ `config`（`AppConfig`——config.yaml 的**统一 schema** 与 `load_config()` 读取，asr/desktop/cli 共享，多余字段对各端无害）。未来加时间工具等。任何项目 crate 都可依赖它。

### octopus-asr（核心推理库）

ASR 推理的核心库，所有上层组件都依赖它。

| 模块 | 说明 |
|------|------|
| `config` | DB 模型配置加载（`AsrConfig`）、模型发现、引擎路由（`resolve_engine_in_config` 按 `{provider}:{category}:{model_name}` 3-part spec 解析）、全局默认引擎兜底（`resolve_active_engine`）、云引擎分类（`EngineCategory::Aliyun`，由 `resolve_category` 按 provider 分支识别） |
| `audio` | WAV 读取、重采样（`resample_to` 一次性 / `AudioResampler` 流式，支持任意 from→to 速率，含 denoise 48k 桥接）、VAD 语音过滤 |
| `denoise` | 可插拔流式环境降噪后端（`FrameDenoise` trait，由 `denoise_mode` 选择）：`1`=RNNoise（`nnnoiseless`，纯 Rust 移植 Xiph RNNoise，内置默认模型，48kHz/FRAME_SIZE=480→频带特征+VAD/噪声/降噪 GRU→频带增益+OLA，GRU 状态跨帧保持）/ `2`=DeepFilterNet3（`Df3Backend` 包装 libDF v0.5.6 的 `DfTract` + tract 0.19，48kHz 全频带）。`DenoiseProcessor` 为 mode 分发器，采集层前置 |
| `vad` | Silero VAD 语音活动检测 |
| `whisper` | Whisper 离线识别 |
| `sensevoice` | SenseVoice 离线识别 |
| `paraformer` | Paraformer 离线识别 |
| `qwen3_asr` | Qwen3-ASR 离线识别 |
| `zipformer` | Zipformer 离线识别 |
| `streaming_paraformer` | Paraformer 流式识别 |
| `streaming_zipformer` | Zipformer 流式识别 |
| `corrector` | 基于拼音映射和 Bigram 转移概率的轻量级中文拼音纠错与热词校正 |
| `hans` | 简繁体字形转换（单字级，开放词典网 CC-BY 3.0 对照表编译期嵌入）；按 `output_simplified` 归一化 ASR 输出 |


**数据流（离线）：**
```
音频文件/WAV → read_wav_16k → [VAD 过滤] → 引擎.transcribe → 文本
```

**数据流（流式）：**
```
麦克风 → PCM chunk → resample_to_16k → 引擎.accept_samples → [partial]
                                    └─ 静音≥0.5s → 引擎.flush（补零吐尾音，无逗号）→ [partial]
                                                              → engine.finish → [final]
```

### octopus-cli（命令行工具）

通过 clap 提供 5 个子命令：

| 命令 | 说明 |
|------|------|
| `devices` | 列出可用麦克风 |
| `config` | 显示模型发现信息 |
| `transcribe` | WAV 文件离线识别 |
| `e2e` | 麦克风实时识别（离线/流式） |
| `stream-test` | WAV 文件流式识别测试 |

### octopus-server（HTTP 服务）

基于 Axum 的 Web 服务，提供 REST 和 WebSocket 接口。

```
Client ──HTTP POST──→ /transcribe ──→ octopus-asr ──→ JSON 响应
Client ──WebSocket──→ /ws/stream  ──→ VAD + ASR   ──→ 流式 JSON
```

### octopus-desktop（桌面应用）

基于 Tauri 2 的桌面应用，支持系统托盘、全局快捷键、悬浮窗、流式识别。

**识别模式：**

| 模式 | 引擎 | 说明 |
|------|------|------|
| 流式 | Paraformer, Zipformer | 边说边识别，600ms tick 驱动 |
| 离线 | SenseVoice, Whisper, Qwen3-ASR | VAD 分段伪流式，300ms tick 驱动，阈值可配置 |

**窗口管理：**

| 窗口 | 用途 |
|------|------|
| `recording_overlay` | 录音/识别状态提示（离线模式） |
| `result_window` | 识别结果展示（可拖拽、多行滚动、透明无边框、置顶）。顶部悬停工具栏：鼠标移入展开（窗口高度 100→132px），移出收起；6 个工具——系统设置 / 语音模型 / 降噪模式 / 润色模型 / 润色模式 / 立即润色。由 `config.yaml.hide_toolbar`（默认 `true`）控制：`true`=hover 显隐，`false`=始终显示。**运行时切换立即生效**：设置窗口改 `hide_toolbar` → emit `config-changed` 事件 → result window 的 `refreshActive()` 双向切换（`false`→移除 hover + 常驻展开；`true`→恢复 hover + 立即收起） |
| `settings_window` | 独立设置窗口（原生标题栏、800×600 可调大小、最小 640×480）。三页面侧边栏布局：识别记录（倒序分页 + 批量删除 + 润色优先显示 + 拷贝）/ 系统设置（17 字段表单，卡片分组 + toggle/select/number input + 生效时间标签内联）/ 模型管理（占位）。单例管理：已打开则 `set_focus`。入口：工具栏设置按钮 + 托盘菜单「设置...」。6 个命令：`open_settings` / `get_config` / `set_config(key,value)` / `get_history` / `delete_history(ids)` / `check_shortcut(shortcut)` |

**macOS 动态激活策略（Dock 图标显隐）：** 应用启动即 `Accessory` 模式（无 Dock 图标，纯托盘应用）。用户打开设置窗口时 `open_settings` 切 `Regular`，并经 `set_dock_icon()` 用 `objc2` 手动 `setApplicationIconImage`（release 裸二进制无 .app bundle，Tauri 仅 debug 自动设图标，故需手动设 Dock + 应用图标）；设置窗口 `Destroyed` 事件触发 `on_settings_closed` 切回 `Accessory`。`#[cfg(target_os = "macos")]` 条件编译，Windows / Linux 无此逻辑。

**窗口加载就绪（ready）机制：** 结果窗 webview 首次加载有延迟，若后端在页面就绪前 `emit('show-result')`，事件丢失导致「文本不显示 / 不弹窗」。`result_window.rs` 以 `WINDOW_READY`（AtomicBool）+ `PENDING_TEXT`（Mutex<Option<String>>）兜底——未 ready 时暂存文本，前端 `index.html` 加载完成后发起 `result_window_ready` Tauri command → 后端置 ready 并冲刷积压文本。`show_result` / `update_result` 把「判 ready + 写 pending」收进同一把 `PENDING_TEXT` 锁，与 `result_window_ready` 的 store(true)+take 互斥，消除启动首帧 TOCTOU 文本滞留。

**核心状态机（Coordinator）：**
- 单线程 mpsc channel 串行化所有事件
- 流式模式：Streaming → (Polishing) → Pasting
- 离线模式（VadSegmented 伪流式）：VadSegmented → WaitingCompletion → (Polishing) → Pasting
- 云端流式模式（dashscope feature，VAD-gated per-utterance streaming）：CloudStreaming → (Polishing) → Pasting
- **音频处理流水线（drain_samples → VAD → ASR，三种 stage 共用同一前处理）**：从 cpal 回调到引擎输入只走一条路径，所有降噪 / 重采样都在 `SharedAudioState::drain_samples` 内部完成，coordinator 层从不直接调 DenoiseProcessor。详见 `crates/desktop/src/audio.rs::process_pipeline`。

  ```
  cpal Stream 回调（设备原生 SR）
    │  → SharedAudioState.samples（Mutex<Vec<f32>>）
    ▼
  drain_samples()                    ← coordinator 每 tick 调用
    │  1. take(samples)
    │  2. process_pipeline(raw, SR, flush=false)
    │     │
    │     ├─ 直通路径（denoise_mode=0 / 后端加载失败 / 单帧推理失败 降级）：
    │     │     原生 SR ───────────resampler──────────▶ 16k
    │     │
    │     └─ 降噪路径（denoise_mode=1 RNNoise / 2 DeepFilterNet3，详见「环境降噪」）：
    │           原生 SR ──down_sampler──▶ 48k
    │             ──DenoiseProcessor.process_samples──▶ 48k 已降噪
    │             ──resampler────────────────────────▶ 16k 已降噪
    │
    │  GRU 隐状态跨 tick / 跨段连续保持（flush=false 保滤波器+GRU 续接，
    │  噪声估计是连续物理过程）；仅会话级 start() 调 reset()（DF3 = 重载模型）
    ▼
  samples: Vec<f32>（16k 单声道，已降噪 或 直通）—— 三种 stage 看到的是同一份
    │
    ├─ Streaming（StreamingSession 本地流式，[`crates/asr/src/streaming`]）：
    │     StreamingSession.accept_samples(&samples, was_silent) → partial
    │     （累积静音 ≥0.5s 时引擎独立补零 Active Flush，不走 drain_samples）
    │
    ├─ VadSegmented（本地离线引擎，[`coordinator.rs::handle_vad_segmented_tick`]）：
    │     audio_buffer.extend(&samples)
    │     compute_speech_chunks(vad, &samples)         // 检测 VAD（跨 tick 有状态累积）
    │     → 静音 ≥ segment_silence / 持续 ≥ SEGMENT_DURATION_S（20s 常量）：
    │         filter_speech_from_buffer(filter_vad, send_buffer)  // 过滤 VAD（每段 reset）
    │         → spawn_blocking(engine.transcribe(&speech_samples))
    │         → Command::TranscriptionDone{seq} → 按 seq 有序拼接
    │
    └─ CloudStreaming（DashScope WSS 长连接，[`coordinator.rs::handle_cloud_streaming_tick`]）：
          pre_roll_buffer 滚动追加 samples（保留后 200ms = CLOUD_PREROLL_BUFFER_SAMPLES）
          compute_speech_chunks(vad, &samples) → onset 检测（≥2 speech chunks）
          ├─ 无活跃 WSS + onset：
          │     resolve_dashscope_config(spec) → (endpoint, key, model)
          │     DashScopeStreamSession::open(pre_roll = pre_roll_buffer 末 100ms = 1600 样本)
          │     push_pcm(&samples)
          ├─ 有活跃 WSS + 持续语音：
          │     push_pcm(&samples)（→ s16le → WS binary）
          │     drain_stream_events() → try_recv_text → 更新 transcript + emit("show-result")
          └─ 有活跃 WSS + 静音 ≥ pause_polish_threshold_ms（700ms）：
                session.close(rt)（发 finish-task + 阻塞收最终文本）
                → transcript.append_segment(final_text)（逗号拼接跨句）
                → check_and_trigger_polish（停顿润色）
                → session = None（等下一段 onset）
  ```

  **关键不变量**：
  - **降噪在 drain_samples 内部完成**——三种 stage 拿到的 `samples` 都是 16k 已降噪（或降级直通）样本；VAD 与 ASR 用同一份降噪后信号，避免参数 / 状态不一致致 VAD 误判而 ASR 准的解耦 bug。云端引擎（CloudStreaming）的 pre-roll 同样从 drain_samples 取，DashScope 收到的是干净音频。
  - **降噪 GRU 与 VAD LSTM 状态语义相反**：降噪 GRU **跨 tick / 跨段连续保持**（`flush=false`，噪声估计是连续物理过程，仅会话 `start()` 才 reset）；检测 VAD **跨 tick 有状态累积**（看完整流，稳语音/静音边界）；过滤 VAD **每段 reset**（独立冷启动，等价每段新 VAD 但复用 ONNX Session）。详见「VAD 分段切分策略」。
  - **降级不 panic**：`denoise_mode=0` / 后端模型缺失 / 单帧推理失败 → `process_pipeline` 走直通分支（原生→16k），仅 warn 日志，识别继续不阻断录音。
  - **CloudStreaming 的 VAD 用法与 VadSegmented 一致**：同一个 `compute_speech_chunks` + `SileroVad` 检测 onset，但**不切分过滤**（不调 `filter_speech_from_buffer`）——DashScope 自己有 server-side `max_sentence_silence` 切句，客户端 VAD 只负责「何时开 / 何时关 WSS」的生命周期门控。
- **CloudStreaming（DashScope WSS 长连接，[`dashscope_stream.rs`](../crates/desktop/src/dashscope_stream.rs)）**：当 `is_cloud_engine(cfg)`（`asr_engine` 解析 category=Aliyun）时启用。与本地 Streaming / VadSegmented 不同——**不调用 `TranscriptionEngine::transcribe`**，而是直接管理一条 DashScope WebSocket 长连接（`DashScopeStreamSession`），由 VAD 决定连接生命周期：① 语音 onset（VAD 检测 ≥2 speech chunks）→ `DashScopeStreamSession::open`（建连 + run-task 含 `max_sentence_silence:600` + 推 100ms pre-roll 给 ASR 声学上下文）；② 持续语音 → `push_pcm` 推帧 + `try_recv_text` 轮询 partial 实时更新 transcript + UI；③ 静音 ≥ `pause_polish_threshold_ms`（700ms）→ `close`（发 finish-task + 阻塞收最终文本）→ append 到 transcript（逗号分隔跨句）→ `check_and_trigger_polish` 停顿润色 → `session=None`（等下一段 onset 再开新 WSS）。**tick 间隔 100ms**（`CLOUD_STREAMING_TICK_INTERVAL_MS`），**pre-roll 滚动缓冲 200ms**（`CLOUD_PREROLL_BUFFER_SAMPLES=3200`，每次 onset 提取后 100ms `CLOUD_PREROLL_SAMPLES=1600` 推给 WSS）。后台 WS task 在 tauri tokio runtime 上 spawn，双向循环（PCM channel→WS binary / WS text→result channel）用 `tokio::select!`。Toggle 停止时若 WSS 仍活跃 → `close` 收尾文本 → 走 Pasting。详见 [spec](superpowers/specs/2026-06-18-dashscope-streaming-design.md)。
- **最终润色异步化**：停止后若启用润色（mode=1/2），`start_final_polish_or_paste` 进入 `Stage::Polishing`（spawn 独立线程跑 LLM 网络请求，托盘显「处理中」、结果窗显「最终润色中」），LLM 完成回调 `Command::FinalPolishDone` 后 `do_paste` 落地；未启用润色则直接 `do_paste`。**润色期间协调器线程不阻塞**，`Cancel`（Esc）可即时回滚 Idle、丢弃在途结果，`Toggle` 被互斥忽略（防并发缓存污染）。Polishing 仅持 `id` + `raw_text`（不需 Transcript 其余字段）
- **粘贴异步化（`do_paste`）**：`do_paste` 先同步 `show_result` + 置 `Stage::Pasting`（状态机线程），再把真正的落库粘贴（`paste::paste`——含 enigo 键盘模拟 + 焦点切换 `sleep`）投递到 `tauri::async_runtime::spawn` + `tokio::task::spawn_blocking`，完成后回 `Command::PasteDone`——粘贴期间不占用 Tauri UI 主线程、不阻塞协调器线程。**macOS 键盘模拟线程安全**：`paste_via_clipboard` 的 V 键用固定虚拟键码 `Key::Other(9)`（`kVK_ANSI_V`）而非 `Key::Unicode('v')`——enigo 0.6.1 的 `Key::Unicode` 在 macOS 走 `get_layoutdependent_keycode`（循环调用非线程安全的 Carbon `TIS*`/`UCKeyTranslate` API），在 `spawn_blocking` 非主线程执行会触发 SIGTRAP（`Trace/BPT trap: 5`）；`Key::Other` 直接当 keycode 用绕过 layout 查找。详见 [spec](superpowers/specs/2026-06-17-archived-design.md)
- **取消录音（Cancel）**：结果窗按 Esc → 前端 `invoke('cancel_recording')` → `coordinator::cancel_recording` Tauri command → `Coordinator::cancel` 发 `Command::Cancel`。`handle_cancel` 跨阶段生效——Streaming 停采集 + reset 引擎，VadSegmented 停 tick + 停采集，WaitingCompletion / Polishing 丢弃在途结果，统一回 `Idle` + 隐藏 overlay / result 窗 + 托盘置 Idle（Idle 下为 no-op）。Esc 同时 `currentWindow.hide()` 提供即时反馈（区别于 RuntimeConfig 子系统的 4 个命令，`cancel_recording` 定义在 `coordinator` 模块）
- **音频采集按需启停（替代常驻，修复菜单栏麦克风指示灯常亮）**：`cpal::Stream` 所有权收归 `SharedAudioState`（`Mutex<Option<cpal::Stream>>`），不再 `std::mem::forget` 泄漏保活——**每次录音 `start()` 现场建流（`build_stream`）+ play，`stop()` pause + drop（take 出 Option 在本线程析构）**：空闲期无流、菜单栏麦克风指示灯灭、不触发麦克风权限；录音期间流持续播放、回调内 `is_recording` 作冗余守卫。**Send-safety（已根治）**：`cpal::Stream` 为 `!Send + !Sync`，但 SharedAudioState 的 Arc 被 `move` 进 Coordinator 的 `std::thread::spawn` 循环闭包、仅该线程独占持有（`audio` 不在 Coordinator 结构体字段），故 Stream 的建（start）/ 播（play）/ 停（stop）/ 析构（stop take-drop 或循环线程退出）全程同线程、无跨线程访问；cpal 回调线程只持有独立 clone 的 `Arc<Mutex<Vec>>`/`Arc<AtomicBool>`。`unsafe impl Send/Sync` 在此前提下 sound（注释记录该不变量）。建流失败由 `start()` 返回 `Err`、上层降级
- **音频初始化防闪退**：`AudioRecorder::open()` 仅校验麦克风存在（失败 `log::error` + 仍持有静音占位 `SharedAudioState`，应用进托盘不 `expect` panic）；真正的 `build_stream` 推迟到首次 `start()`，建流失败（无设备 / 权限拒绝 / 占用）由 `start()` 返回 `Err`、上层降级（采样恒空 → 识别静默 → 空文本回 `Idle`），改配置后重启恢复
- **流式重采样器缓存**：非 16kHz 麦克风源的流式重采样经 `crates/asr/src/audio.rs` 的 `AudioResampler`（有状态 `rubato::FftFixedIn` + 跨帧 leftover 缓冲）——`desktop::SharedAudioState` 持 `Mutex<Option<AudioResampler>>`，源速率不变时**复用同一规划器**（避免每 ~300ms tick 的 FFT planner 重规划开销，并保留滤波器跨帧状态保边界 glitch-free），仅 `stop` 时 `flush` 补零吐尾 + 置 `None`；`drain_samples` 不 flush。`AudioResampler` 经编译期断言 `Send+Sync`（固化 `SharedAudioState` 的 `unsafe impl` 前提，防 rubato 升级引入非 Send 字段静默退化为 UB）
- **环境降噪（可插拔后端，采集层前置）**：麦克风音频送入 VAD/ASR 前，经 `crates/asr/src/denoise.rs` 的 `DenoiseProcessor`（mode 分发器，对外接口与旧 RNNoise-only 一致）降低背景噪声。降噪为**可插拔后端**（`FrameDenoise` trait，`process_frame(&[f32;480], &mut [f32;480])` 用 `[-1,1]` 单声道契约），由 `config.yaml.denoise_mode` 选择：
  - `0` = 关闭（直通，零开销）。
  - `1` = RNNoise（`RnnoiseBackend`，`nnnoiseless` 纯 Rust 移植 Xiph RNNoise，内置默认模型，48kHz FRAME_SIZE=480(10ms)→频带特征 + VAD/噪声/降噪 GRU → 频带增益 → iSTFT+OLA）。**默认**。
  - `2` = DeepFilterNet3（`Df3Backend`，libDF v0.5.6 的 `DfTract` + tract 0.19，48kHz 全频带，编译期内嵌 ~7.9MB `DeepFilterNet3_onnx.tar.gz` 模型）。质量最佳（干净语音 gain≈0.96、带噪 gain≈0.60、RTF≈0.015–0.036）。DF3 **懒加载**：`new(mode=Df3)` 仅占位，首次 `process_samples` 才加载模型（避免构造热路径阻塞）。
  - 缺省 `1`（`default_denoise_mode()`）。`denoise_mode: u8` 亦可由工具栏运行时切换（`set_denoise_mode` 命令）并持久化回 config.yaml。

  **帧边界隔离 ndarray 版本**：libDF（deep_filter）依赖 ndarray 0.15，asr 现有 ndarray 0.17（ort/whisper 等）。Cargo 允许同 workspace 共存（不同 major）。`FrameDenoise` trait 只用原生 `&[f32]`/`&mut [f32]`，绝不暴露 ndarray 类型；`Df3Backend` 内部用与 libDF 同实例的 `ndarray_015`（package rename）构造 `ArrayView2 [1,480]` 喂 `DfTract::process`，asr 的 0.17 类型完全不触及。

  **DF3 依赖（git，非 crates.io）**：`df = { git = "https://github.com/Rikorose/DeepFilterNet.git", tag = "v0.5.6", package = "deep_filter", features = ["tract", "default-model", "transforms"] }`（libDF 不在 crates.io，只能 git）。tag v0.5.6 对应 commit `978576aa`，tract `^0.19.4`（解析到 0.19.16，**不可用 0.21.x**——0.21.4 在 native 有 codegen bug 致权重 NaN，连官方 `deep-filter` bin 也崩）。

  **Send/Sync**：`DfTract` 含 `Arc<dyn RealToComplex<f32>>`（无 `+ Send`）→ `!Send`，故 `Df3Backend` 经 `unsafe impl Send/Sync`（照 VST3 plugin/src/lib.rs:9-11）。安全性：`DenoiseProcessor` 在 `Mutex` 内、coordinator 单线程串行 lock+process（audio.rs:94 注释），实际无跨线程并发，unsafe 仅满足类型约束不引入数据竞争。`RnnoiseBackend`（`Box<DenoiseState<'static>>`）天然 Send，无需 unsafe。

  **状态保持与降级**：GRU 隐状态 + 特征缓冲 **跨 `drain_samples` 周期、跨 VAD 分段连续保持**（噪声估计是连续物理过程，与 `filter_vad` 每段 reset 故意相反）；新会话 `start()` 调 `reset()`（DF3 reset = 重载 7.9MB 模型，仅会话边界可接受）。链路 `process_pipeline`：原生SR→(`down_sampler`)→48k→DenoiseProcessor→(`resampler`)→16k（`flush` 语义同重采样器：`drain_samples` 不 flush 保连续、`stop` flush 取尾）。**三级降级**：`mode=0`→直通；后端加载/单帧推理失败→warn + backend 置 `None`→直通；**不 panic**、不阻断录音。无外部模型文件依赖（RNNoise 内置模型 / DF3 编译期内嵌），不进 DB、不参与引擎选择。

  **DF3 加载日志**：tract 加载 DF3 模型时刷大量 DEBUG（`tract_core::optim` 的 `applying patch`、`tract_hir::infer` 的 shape 推断），`crates/desktop/src/main.rs` 的 `tauri_plugin_log::Builder` 对 `tract_core`/`tract_hir`/`tract_onnx`/`tract_linalg` 四子模块 `level_for(Warn)` 压制；**保留** `df::tract` 自身 `Info`（`Loading model ...` / `Init encoder` / `Running with model type ...`）作加载进度信号。RNNoise 无 tract 依赖，不受影响。

  **历史**：第一版曾用第三方 `dfn3.onnx` + ort（模型缺陷压语音至 ~10%，已弃用换 RNNoise，见 [`2026-06-16-denoise-deepfilternet-design.md`](superpowers/specs/2026-06-16-archived-design.md)）；本版改用官方原生 libDF + tract（spike 验证 gain=0.958 不压语音），DF3 与 RNNoise 并存。详见 [spec](superpowers/specs/2026-06-17-archived-design.md)
- **VAD 分段切分策略**（`handle_vad_segmented_tick`）：静音边界切分（主）+ 连续超时强制切断（兜底）
  - 静音切分：检测到语音后静音 ≥ `segment_silence`（默认 400ms）→ 切分，**无 overlap**（静音是自然语句边界，下一段从干净开始）
  - 强制切断：连续语音缓冲达 `SEGMENT_DURATION_S`（20s 常量）仍未静音 → 强制切断，**保留末尾 200ms（常量 `SEGMENT_OVERLAP_MS`）作下一段 overlap**（语句被硬切，需重叠保连贯）。`segment_duration` / `segment_overlap` 原为 config 字段，因属实现细节（用户不可感知）已改为常量
  - **双 VAD 实例（检测流 vs 过滤，修 LSTM 状态污染）**：SileroVad 是有状态 LSTM（`compute()` 更新 `h`/`c`，`reset()` 归零）。`VadSegmented` stage 持**两个独立实例**：① 检测用 `vad`——逐 tick 喂入顺序音频、跨 tick 有状态累积（续接上下文使语音/静音边界判定更稳），喂 `compute_speech_chunks`；② 过滤用 `filter_vad`——仅 `filter_speech_from_buffer` 用，**每次过滤前 `reset()` 归零**，恢复「每段独立冷启动」语义（等价旧代码每 buffer 新建 VAD，但 ONNX Session 在录音开始时一次性创建，过滤只 reset 不重建，兼顾正确性与性能）。分离原因：检测流已按顺序见过 `samples`，而 `send_buffer`（`overlap_tail` + `audio_buffer`）与之重叠，若共用一个有状态 VAD 会双重喂入 + 跨段污染 LSTM → 段首 gating 失真（裁掉语音起音或混入前导噪声）
  - 每段经 `filter_speech_from_buffer` 过滤静音后，由 `spawn_offline_transcription_with_seq` 派发到 **Tauri 全局异步运行时**（`tauri::async_runtime::spawn`）执行 `engine.transcribe`（底层 CPU 密集推理已 `spawn_blocking` 包裹、不阻塞 runtime worker，不再为每段新建 / 销毁 current-thread Runtime），完成回 `Command::TranscriptionDone{seq}`；协调器按 `seq` 有序拼接（`completed_results: HashMap<seq,String>` + `completed_seq` 游标连续消费）。**识别失败 / 空结果仍占位该 `seq`（写空串）以保证游标连续推进**——否则缺失序号会让消费卡死、该次录音此后所有有效段积压丢失；`Command::TranscriptionDone` 另带 `session_id`（= 录音开始毫秒时间戳 = `transcript.id`），消费它的 `VadSegmented` / `WaitingCompletion` 分支先比对 `transcript.id != session_id` → 丢弃旧会话在途结果（快速双击 Toggle / 录音中重启的竞态：新会话分配新 id，旧会话残留的异步转写回调不再污染当前结果）
- **Transcript 文本状态机**：识别文本状态由 `Transcript` 结构（`crates/desktop/src/transcript.rs`）统一管理——内部用 `full`（当前完整 ASR）+ `raw_len`（上次停顿快照的 char 长度）派生 `raw`（停顿快照，润色基准）/ `increase`（停顿后增量），避免维护三份字符串。`Stage::Streaming` / `VadSegmented` / `WaitingCompletion` 各持 `transcript: Transcript` 字段，文本流经 Transcript 方法（`set_full` / `append_segment` / `display_text` / `db_text`）。停止后 `Stage::Polishing`（最终润色中，持 `id` + `raw_text`）→ `Stage::Pasting`（持 `id` + `raw_text` + `polished_text` + `polish_status`）。入库的 `engine` / `engine_mode` 在过程入库的 raw 阶段已写（`update_transcription_raw(&config.asr_engine, ..)`），`Pasting` 不再持有。详见 [spec](superpowers/specs/2026-06-14-archived-design.md)
- **停顿驱动润色**：流式 / 伪流式统一——静音 ≥ `pause_polish_threshold_ms`（默认 600ms，可配置）/ 伪流式段边界完成时，经 `take_polish_input()` 取润色输入（无编辑 = 全量 ASR `raw + increase`；已编辑 = `(edited, 新增)` 边界，见「结果窗可编辑」）送 LLM 润色（mode=2 only），**不重置流式引擎**（只读送 LLM，引擎状态原样保留）。修复了流式中间润色 P0（partial 全量覆盖 polished）。默认 600ms > Active Flush 500ms（GUI 约束 `>= 600`，须大于句间停顿最大值，否则润色先于尾音冲刷、快照缺尾音），润色在 tick 流程最末执行，快照可靠
- **立即润色（PolishNow）**：工具栏「立即润色」按钮（`tool-polish-now`）点击 → `invoke('polish_now')` → `Command::PolishNow` → `handle_polish_now`：**忽略 `polish_mode`**（不受 mode=0/1/2 限制，区别于停顿润色的 mode=2 限制），经 `llm_config_ignore_mode()` 取 LLM 配置，复用 `take_polish_input` → `spawn_polish_thread(ignore_mode=true)` → `Command::PolishDone` 路径。`spawn_polish_thread` 新增 `ignore_mode` 参数控制是否绕过 mode 检查。`handle_polish_done` 接受 `Streaming`/`VadSegmented`/`WaitingCompletion` 三阶段（防用户点按钮后停录音致 stage 切换、润色结果被丢弃），写回后 `emit("polish-done")` 通知前端恢复按钮（成功/失败/stage 不匹配均通知）。**`handle_polish_now` 所有早退路径（stage 不匹配 / transcript 空 / 已 pending / LLM 配置缺失）都 emit `polish-done`**——否则前端 `btnPolishNow.disabled=true` 永久卡死。`Transcript::display_text()` 同步变更：**polished 非空即展示**（`polished + increase`），不再仅限 `mode==Intermediate`，使 PolishNow 在任意 mode 下都能让润色文本覆盖 raw 回显到展示区
- **结果窗可编辑（Transcript 三文本分层）**：
  - `Transcript` 三文本分层：`edited ≻ polished ≻ raw`。`display_text()` = committed + increase；`full`（原始 ASR）独立保留为 DB `raw_text`。
  - 编辑态：coordinator 主循环 `editing` 标志置位时，Streaming/VadSegmented tick 跳过喂引擎、只排空丢弃音频（硬暂停）。`commit_edit` 写回 transcript 并 `UPDATE edited_text`。
  - 编辑×润色（spec §12）：`take_polish_input()` 返回 `(preserved=edited, to_polish=increase)`，LLM 仅润色新增；`on_polish_done` 在 `has_edit()` 时折回 `edited`（避免遮蔽丢字）。
  - `transcriptions` 表加 `edited_text` 列（commit + 中间润色折回时写）。
  - 停止路径：润色输入 = `take_polish_input`；无润色/兜底粘贴 = `edited_display()`；最终润色失败兜底 = `Stage::Polishing.fallback_text`；DB raw 仍 = `db_text()`。
- VAD 标点：基于 SileroVad 静音检测，>0.5s 静音插入逗号。**段间拼接标点去重**：`consume_completed_results` 在段间补逗号前同时检查「新段不以标点开头」和「已有文本不以标点结尾」，避免 ASR 引擎返回的自带句尾标点与补的逗号连续出现（`。，` `？，`）
- 流式尾音冲刷（Active Flush）：流式模式累积静音 ≥0.5s 时向引擎补零，强制对齐右上下文 / 触发 CIF，把憋住的尾音即时吐出；走独立路径不插逗号，每个静音段仅触发一次（`flushed` 标志，恢复说话时重置）。详见 [spec](superpowers/specs/2026-06-14-archived-design.md)
- **设置窗口子系统（settings_commands + settings_window）**：独立 Tauri 窗口 `settings_window`（`settings_window.rs`），原生标题栏、800×600 可调。6 个 Tauri 命令：`open_settings`（单例窗口管理——`get_webview_window` → `set_focus`，否则 `WebviewWindowBuilder` 新建；macOS 打开时切 `Regular` 激活策略显示 Dock 图标 + `setApplicationIconImage`）、`get_config`（返回 `ConfigResponse`：AppConfig JSON + ASR 引擎列表 + LLM 模型列表 + 麦克风设备列表）、`set_config(key, value)`（通用字段写入器，`apply_config_value` 做 17 字段类型/范围校验 → `sync_runtime_config` 同步 RuntimeConfig 镜像 → `write_config_yaml` 持久化；`shortcut` 字段热重载——注销旧快捷键 + `register_shortcut` 新的）、`get_history(limit, offset)`（分页查询 `transcriptions` 表，返回 `Vec<TranscriptionRecord>`）、`delete_history(ids)`（批量删除，`IN` 子句，返回删除行数）、`check_shortcut(shortcut)`（快捷键冲突检测——`on_shortcut` 注册 → 立即 `unregister`，仅检测不持久化）。前端 `dist/settings/index.html` 纯 vanilla HTML，无构建步骤。`polish_mode` 序列化为 `u8`（0/1/2），前端 select 用数字 value。**识别记录页**：倒序排列，润色 text 优先显示（黑色主文本）、原始 text 折叠（灰色次要），工具栏含全选 checkbox + 批量删除（两次点击确认，Tauri webview 不支持原生 `confirm()`，任何勾选变化/超时自动取消确认态），每条记录右侧拷贝按钮（内联 `copy.svg` 图标）。**系统设置页**：6 张卡片（交互置顶，全部无标题）；生效时间标签内联到 label 文字后面（灰色小字括号如「(立即)」）；快捷键改为键盘捕获按钮（捕获 → `check_shortcut` 冲突检测 → `set_config` 热重载）；润色间隔/润色停顿阈值改为下拉选择（`pause_polish_threshold_ms` 约束 `>= 600`）；语言仅 auto/zh/en。
- **运行时配置子系统（RuntimeConfig）**：工具栏可运行时切换 `asr_engine` / `polish_mode` / `polish_llm` / `denoise_mode`，无需重启。`runtime_config.rs` 提供 `SharedRuntimeConfig`（`Arc<RwLock<RuntimeConfig>>`，挂 `tauri::State`）作为这四个字段的**可变运行时镜像**，与启动只读的 `AppConfig` 快照互补。8 个 Tauri 命令（`toolbar_state` / `list_asr_engines` / `switch_asr_engine` / `set_polish_mode` / `list_llm_models` / `switch_polish_llm` / `set_denoise_mode` / `polish_now`）读写镜像 + best-effort 持久化回 `~/.octopus/config.yaml`（写盘失败仅 `warn`，本次仍生效、重启回退；`polish_now` 不写盘，只触发润色流程）。**`switch_asr_engine` / `switch_polish_llm` 前端传裸 `model_name`，后端查 DB 取 `provider` / `category` 构造 3-part spec（`"{provider}:{category}:{model_name}"`）写入 RuntimeConfig + config.yaml**——保证持久化值与 `parse_model_spec` 解析一致。`list_*` 的 current 判定经 `parse_model_spec(current).model_name()` 提取裸名比较，兼容 3-part 和裸名两种历史格式。`switch_asr_engine` 同时经 `tray::update_tray_engine_label` 实时刷新系统托盘菜单的「引擎: <model_name> (<mode>)」项（`TRAY_ITEMS` 缓存 `engine_info` MenuItem handle，`set_text` 更新而非重建，规避 `MenuItem::with_id` 重复 ID panic）。Coordinator 闭包持镜像句柄，**在 Toggle 进入 `Idle` 时重读 `asr_engine` / `polish_mode` / `polish_llm` 并经 `resolve_active_engine` 校验有效性——保留完整 3-part spec（`rc.asr_engine.clone()`）写回 `config.asr_engine`，失效则兜底 `local:zipformer:zipformer-small-ctc`**，保证 `is_streaming_engine` 判定 / `use_streaming` 重算 / `StreamingSession::new` / 离线 `transcribe` / transcriptions.engine 记录全用完整有效 spec；`main.rs` 启动 preheat 同样解析（preheat 与实际工作模型一致）。**外部修改 RuntimeConfig 后立即同步到 coordinator（2026-06-18 改进）**：`set_config`（设置窗口）和 `switch_polish_llm`（工具栏浮层）写完 RuntimeConfig 后调 `coordinator.update_runtime()` → `Command::UpdateRuntime` → `sync_runtime_fields` 把 `polish_llm` / `polish_mode` / `asr_correct` / `output_simplified` / `hide_toolbar` 同步到 config 快照，**无需 Toggle 即可生效**（用户在录音中改 polish_llm 下次润色就用新模型）。`asr_engine` 不走此路径（需重建引擎实例）。`polish_mode` 仍保留每 tick 读 `set_mode`（双保险立即生效）。详见 [spec](superpowers/specs/2026-06-16-archived-design.md)

**文本持久化（嵌入式 SQLite）：**
- 存储：`~/.octopus/octopus.db`（`crates/infra/src/db.rs`，全局 `OnceLock<Mutex<Connection>>`；asr crate 经 `pub use octopus_infra::db` 以 `crate::db` 暴露；cli/server/desktop 共用）
- `transcriptions` 表：识别历史，每条存原生识别全文（`raw_text`）+ 润色版（`polished_text`）+ 润色状态（`polish_status`：`off`/`done`/`failed`）+ 元数据（engine / engine_mode / created_at / char_count / duration_ms）
- **过程增量入库（schema v3）**：`transcriptions.id` = 识别开始毫秒时间戳（`INTEGER PRIMARY KEY`，应用写入，去 `AUTOINCREMENT`），兼任主键 / 业务 key / 开始时间戳；`duration_ms = finalize_now_ms - id`。入库时机分散到识别过程各事件：首次有 ASR 文本 → `INSERT`（`insert_transcription_at_id`）；分段 / 流式 partial → `UPDATE raw_text`（`update_raw_text`）；停顿润色完成 → `UPDATE polished_text`（`update_polished`）；停止 → `finalize`（含 `duration_ms`，`finalize_transcription`）。DB 失败仅 `warn` log 不阻塞识别（best-effort）。v2→v3 migration DROP 重建（旧数据无所谓）。详见 [spec](superpowers/specs/2026-06-14-archived-design.md)
- **非阻塞 DB 写入（actor 模式）**：上述 `INSERT`/`UPDATE`/`finalize` 不在协调器线程同步执行——`update_transcription_raw` / `PasteDone` 等调用方仅 `get_db_sender().send(DbCommand)` 入队后立即返回，真实落库由**后台 DB 写线程**（`static DB_SENDER: OnceLock<Sender<DbCommand>>` 懒加载 spawn）单线程消费。mpsc 的 FIFO 保证同 id 的 `Insert` 必在 `UpdateRaw` 之前被消费（故 `mark_db_inserted()` 在 send 后即置位仍安全——真实顺序由 channel 保，不由标志位保）。识别主循环不再被 SQLite I/O 阻塞。
- **关机优雅 drain**：后台写线程 `&'static Sender` 永不 drop，进程 kill 时队列里未处理命令会丢失（典型路径：录音结束 → `Finalize` 入队 → 用户立即退出 → 该条记录停留未 finalize 态）。`coordinator::shutdown_db()` 置 `DB_SHUTDOWN`（AtomicBool）→ 后台线程排空 `try_iter()` 剩余命令后退出，主线程 `JoinHandle::join` 等待落库完成；`main.rs` 挂到 `tauri::RunEvent::ExitRequested`（macOS Cmd+Q / 关闭最后一个窗口触发），保证退出前队列清空。
- `models` 表：模型目录（**唯一来源**，schema 见 `crates/infra/src/db.sql`，首次建库 `user_version=0` 时整体执行一次 seed 默认引擎集；列 `domain` / `provider` / `category` / `model_name` / `source` / `secret_key` / `language` / `is_local` / `is_thinking` / `is_streaming` / `is_enabled` / `description`，唯一键 `UNIQUE(domain, provider, category, model_name)`；`load_models_at` 仅读 `domain='asr' AND is_enabled=1`，`domain='llm'` 经 `load_llm_model(spec)` 按 `{provider}:{category}:{model_name}` 3-part spec 读；引擎激活由 `config.yaml.asr_engine` 决定，无 `is_active` 列，见「模型管理」）
- `model.json` / `history.txt` / `record.txt` 已从代码彻底删除——DB 是唯一配置/存储源
- `polish_status` 基于润色调用结果：未启用→`off`；启用且返回非空→`done`；启用但返回空或失败→`failed`
- 润色三档（`polish_mode`：0 关闭 / 1 仅最终 / 2 中间+最终）：中间润色由 `check_and_trigger_polish` 在停顿点触发（流式静音 ≥ `pause_polish_threshold_ms`（默认 600ms）/ 伪流式段边界），把 `Transcript.take_polish_input()`（完整 ASR；已编辑时分块 `edited + 新增`）送 LLM 润色，节流 `polish_interval`（下限 `MIN_POLISH_INTERVAL_SEC=1.0s`）；最终润色在 `start_final_polish_or_paste`（停止后）：启用润色→`Stage::Polishing` 异步线程跑 LLM，回调 `Command::FinalPolishDone` 后 `do_paste`；未启用→直接 `do_paste`。详见 [设计](superpowers/specs/2026-06-14-archived-design.md)。
- 停止空文本边界：Toggle 停止录音时若 `transcript.full()` 为空（麦克风静音 / VAD 未检出语音），`start_final_polish_or_paste` 空文本分支直接回 `Idle`，必须对称清理 `result_window::hide_result` + `overlay::hide_overlay` + `tray → Idle` 三类 UI 反馈（缺一则"正在聆听…"框残留）。详见 [设计 §4.5](superpowers/specs/2026-06-14-archived-design.md)。

支持三种引擎接入模式：
- **embedded**（默认）：内嵌 octopus-asr，本地推理
- **remote-ws**：通过 WebSocket 连接远程 octopus-server
- **remote-grpc**：通过 gRPC 连接远程推理服务
- **云引擎（aliyun）**：`config.yaml.asr_engine` 解析为 `provider='aliyun'`（`EngineCategory::Aliyun`）时，路由 `DashscopeEngine`（desktop crate，`dashscope` feature 后），不走 `engine_mode` 分支。详见下方「云端 ASR 引擎」
- **远程超时保护**：`WsRemoteEngine` / `GrpcRemoteEngine` / `DashscopeEngine` 的 `transcribe` 均以 `tokio::time::timeout(8s)` 包裹（连接 + 收发全程），`health_check` 同样 `timeout(3s)`——规避网络断开 / 后端无响应致 ASR 队列无限期卡死。超时返回 `Err`，经序列空洞修复的空串占位分支保证 `completed_seq` 连续推进、不拖死后续分段

## 模型管理

模型配置**唯一来源**是 `~/.octopus/octopus.db` 的 `models` 表。小模型（VAD + 默认 ASR）随应用打包到固定路径，开箱即用；大模型按需从 HuggingFace 下载到缓存。

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite（models 表 + transcriptions 表，唯一存储）
├── config.yaml         # 应用配置（麦克风/引擎选择/分段参数等）
└── models/             # 随应用打包的小模型（固定路径）
    ├── silero_vad_v4.onnx   # VAD（1.8M，find_silero_vad 固定加载）
    └── zipformer/           # 默认 ASR（27M，model.int8.onnx + tokens.txt）

~/.cache/huggingface/hub/   # 大模型 HF 缓存（whisper/sensevoice/qwen3/paraformer 等，按需下载）
```

**模型目录解析（`config::resolve_model_dir`）** —— source 字段双模式：
- 本地相对路径（如 `models/zipformer`）→ `~/.octopus/<source>`（随应用打包的小模型）
- HF repo 名（如 `onnx-community/whisper-small`）→ `~/.cache/huggingface/hub/`（大模型缓存）

**两份配置，各司其职：**
- **应用行为配置** `config.yaml` → `infra::config::AppConfig`（`octopus_infra::config::load_config()`，25 字段：麦克风/引擎选择/分段/润色/LLM/粘贴/硬件加速/ASR 纠错/降噪/简繁输出/工具栏显隐/降噪模式等）。schema 统一定义在 infra，asr/desktop/cli 共享。
- **DB 模型目录** `~/.octopus/octopus.db` `models` 表 → `asr::config::AsrConfig`（`octopus_asr::config::load_config()`，首次 `db::ensure_db()` 自动建表 + seed，读后缓存到 `OnceLock`）。
- **`write_to_clipboard`**（默认 `true`）：粘贴后是否把识别结果留在剪贴板，方便他处再粘贴；与 `paste_method`（`clipboard` / `direct` / `none`）构成三模式矩阵——`clipboard` 模式 true 时不恢复原剪贴板内容、false 时恢复；`direct` 模式 true 时 enigo 输入后末尾写剪贴板、false 时不碰剪贴板；`none` 模式忽略此配置（其唯一目的就是写剪贴板）。`false` 时三种粘贴行为等同重构前现状（不破坏现有用户习惯）。详见 [spec §6](superpowers/specs/2026-06-14-archived-design.md)。

**引擎选择（单一真相 = `config.yaml.asr_engine`）：**
- `models` 表无 `is_active` 列（开发期 schema 变更采用删库重初始化——见 `crates/infra/src/db.sql` 注释；`init_schema` 仅 `user_version=0→1` 一次性建表 + seed，不做 migration）。
- **provider × category taxonomy**（`provider`=vendor/运行位置，与 `category`=引擎族/模型系列 正交）：

  | `provider` | ASR（`category`） | LLM（`category`） |
  |---|---|---|
  | `local` | `zipformer`/`whisper`/`sensevoice`/`paraformer`/`qwen3-asr` | —（暂无本地 LLM） |
  | `aliyun` | `Fun-ASR`（DashScope FunASR Realtime WS） | `qwen` / `deepseek`（经 DashScope 代管） |
  | `deepseek` | — | `deepseek`（直连） |
  | `bigmodel` | — | `glm`（智谱） |

- **模型选择 spec（`asr_engine` / `polish_llm` 统一 3-part 格式）**：配置字符串支持 `"{provider}:{category}:{model_name}"` 三段格式从 DB `models` 表唯一定位模型（见 [spec](superpowers/specs/2026-06-17-archived-design.md)）：
  - `"local:zipformer:zipformer-small-ctc"` → 4 字段精确匹配本地 zipformer
  - `"aliyun:Fun-ASR:fun-asr-2025-11-07"` → 云端 DashScope FunASR
  - `"aliyun:qwen:qwen-plus"` / `"deepseek:deepseek:deepseek-v4-flash"` / `"bigmodel:glm:glm-4-flashx"` → LLM
  - 裸名 `"{model_name}"`（无冒号）→ 仅全局 fallback 路径用（跨 provider/category 搜，优先 local）
  - 旧 2-part（1 冒号）→ warn + 裸名兜底（迁移期）
  - 统一解析在 `infra::db::parse_model_spec`（返回 `ModelSpec::Full` / `NameOnly`），ASR 经 `asr::config::resolve_engine_in_config` 查找，LLM 经 `infra::db::load_llm_model` 查找。区分三段是因为 DB 唯一键是 `UNIQUE(domain, provider, category, model_name)`，不同 provider 或 category 下可有同名模型（如 `deepseek-v4-flash` 在 deepseek 直连与 aliyun 代管下各一行）。
- 全局默认引擎由 `resolve_active_engine(asr_engine)` 解析：**兜底引擎短路**（裸名为 `zipformer-small-ctc` 时跳过 DB 查找，直接返回硬构造兜底 entry，不触发 warning）→ 其余 spec 匹配命中则用；空/不匹配回退兜底 `zipformer-small-ctc`（`DEFAULT_ASR_MODEL_DIR` 本地打包路径，开箱可用）。返回 `ResolvedEngine.model_name` 始终是**裸名**（去掉前缀），下游缓存和加载按裸名工作。
- **云引擎路由（`provider='aliyun'` → `DashscopeEngine`）**：`resolve_active_engine` 解析时若 `provider='aliyun'` → 由 `resolve_category(provider, category)` 按 provider 分支返回 `EngineCategory::Aliyun`（**注意**：`engine_category_from_str("aliyun")` 仍返回 `None`——aliyun 不进 5 个本地族字符串映射，靠 provider 分支识别）。`desktop/src/main.rs` 启动时 `resolve_active_engine(&config.asr_engine)` → 若 `resolved.category == Some(EngineCategory::Aliyun)` → `Arc::new(DashscopeEngine::new())`（需开 `dashscope` feature）；否则按 `engine_mode`（embedded/websocket/grpc）走本地 `build_local_engine`。云 ↔ 本地切换改 `config.yaml.asr_engine` 后**重启**生效（engine 实例启动时固定）。
- **流式判定数据驱动**：是否走流式识别由 `models.is_streaming` 列决定——`is_streaming_engine(cfg)` = `resolve_active_engine(cfg.asr_engine).entry.is_streaming`（seed：zipformer×3 + paraformer = 流式；whisper / sensevoice / qwen3-asr×2 / aliyun Fun-ASR = 非流式），不再按 category 硬编码匹配。Coordinator 的 `use_streaming` 据此在 Toggle 进入 `Idle`（切引擎 / 切模式）时重算——流式引擎走本地流式 partial，非流式引擎自动回退 VAD 分段伪流式。`StreamingSession::new` 同样走 `resolve_active_engine`（带兜底），与 `is_streaming_engine` 对称——避免 DB 未命中时 `is_streaming_engine` 兜底成功（→ 进 streaming 路径）但 `StreamingSession::new` 创建失败（→ session 错误）。
- 显式参数（cli `--model`、server 请求 `engine`、`AsrEngineManager.switch_model`）优先级更高，支持 spec 格式、**不走兜底**（匹配不到直接报错）。
- VAD 模型固定路径（`find_silero_vad` 直接返回 `~/.octopus/models/silero_vad_v4.onnx`），不进 DB、不读配置。
- **手编 `models` 表 / `config.yaml` 需重启进程生效**（`OnceLock` 缓存，运行中不可热更新）。

### 云端 ASR 引擎（DashscopeEngine）

`crates/desktop/src/engine_dashscope.rs`（`dashscope` cargo feature 后，默认不开）impl `TranscriptionEngine`，接入阿里云百炼 DashScope FunASR Realtime WebSocket。与本地引擎不同：**不在 ASR crate 内**，而在 desktop crate——因为它是分块式 `TranscriptionEngine`（每段 VAD 开一条 WS 跑 duplex 协议），与本地离线引擎共享 coordinator 的 chunk 路径接口（`is_streaming=0` → 不进本地 `StreamingSession`）。

- **协议流程**：每段 `transcribe(samples, language, "aliyun:Fun-ASR:fun-asr-2025-11-07")` 内部：① parse_model_spec → 取 model_name → 查 `cfg.asr.aliyun[model_name]` 拿 endpoint + secret_key（空则 bail 明确报错含 sqlite3 命令）；② WS 握手 + `Authorization: bearer <key>`；③ 发 `run-task`（text frame，streaming=duplex，format=pcm，sample_rate=16000，language_hints，**`input:{}` 必须在 `payload` 内部**）；④ 流式发二进制 PCM 帧（f32[-1,1]→s16le，200ms 分块）；⑤ 发 `finish-task`（只需 header + `payload.input`，不带 model/parameters）；⑥ 收 `result-generated` 累积 `payload.output.sentence.text`（取最终句）；⑦ `task-finished` 收尾。段级超时 8s（与 `engine_ws.rs` 一致）。
- **鉴权**：WS 握手请求经 `IntoClientRequest` + 追加 `Authorization: bearer <secret_key>` header（DashScope api-ws 端点强制要求）。
- **无运行时状态**：每次 `transcribe` 从 DB 重新解析 → 取最新 endpoint/key（运行时切引擎可即时生效）。
- **健康检查**：`health_check()` 保守返回 `true`，避免每次启动探活消耗 API 额度；真实健康度在首次 transcribe 时由错误路径暴露。

## 支持的 ASR 引擎

| 引擎 | 类型 | 特点 |
|------|------|------|
| Whisper | 离线 | 多语言；传 `auto` 且 DB `models.language` 配了具体语种时优先用后者（`entry_language` 覆盖），否则自动检测 |
| SenseVoice | 离线 | 快速，自动语言检测 |
| Paraformer | 离线/流式 | 中文优化 |
| Qwen3-ASR | 离线 | 大模型能力 |
| Zipformer | 离线/流式 | 轻量级 CTC |

## 拼音纠错与热词校正 (ASR Corrector)

为了在不引入重型深度学习模型（如 MacBERT 等动辄几百 MB 的模型）的前提下，实现极致轻量的纠错与专有名词（热词）校正，项目实现了一套基于 **“拼音映射 + 长度归一化 Bigram 转移概率”** 的轻量级后处理纠错引擎。

### 核心特性
- **纯静态与轻量化**：纠错所需的 unigram 词表与 bigram 共现表（各精简至高频的前 40,000 条，压缩后约 450KB）直接通过 `include_bytes!` 静态嵌入二进制中，无需额外网络下载，运行时解压，额外内存占用约 30MB。
- **配置开关控制**：由 `config.yaml` 中的 `asr_correct` 字段控制（默认 `false`）。
- **智能排除**：由于 Qwen3-ASR (0.6B/1.7B) 模型本身输出带有标点且语义纠错能力强，纠错引擎会自动跳过对 Qwen3-ASR 结果的处理，仅应用于 Whisper、SenseVoice、Paraformer 和 Zipformer。

### 纠错算法逻辑
1. **滑窗候选召回 (Sliding Window)**：使用 2 字和 3 字的字符滑窗扫描识别出的文本，通过拼音库计算滑窗文本的拼音，并在此拼音的 $O(1)$ 模糊拼音倒排索引（支持南方口音混淆，如 `zh/ch/sh` <-> `z/c/s`、`in/en` <-> `ing/eng`、`n` <-> `l` 等）中召回**相同字符长度**的同音/近音候选词。
2. **长度归一化打分 (Length Normalization)**：利用未登录词（typo）容易被 `jieba` 拆碎分词的特性，评估替换后的句子，并使用 **“句子总 log 概率 / 分词后 Token 数量”** 对句子的语言模型得分进行归一化，彻底消除倾向于更短分词结果的长度偏置。
3. **基于 Jieba 字典的自适应惩罚**：
   - 如果原滑窗词是 Jieba 字典中的已登录词（即 `jieba.cut().len() == 1`，说明它是合法的词，如 `"坐上"`），系统施加极高的修改惩罚（`-1.5`）以保护正确表述不被误改；
   - 如果原滑窗词是未登录词（typo，如 `"以经"` 被 Jieba 拆分为 `"以"` 和 `"经"`），则修改惩罚降低（`-0.2`）以积极纠错。

## ASR 输出简繁归一化 (Hans Variant Normalization)

ASR（尤其 Qwen3-ASR 在 `language=auto` 下）输出会混入繁体字；sherpa-onnx [#3509](https://github.com/k2-fsa/sherpa-onnx/issues/3509) 显示 `language` 参数不可靠。故在 ASR 输出边界做**单字级字形归一化**（保持 auto 多语言优势，不依赖 language 参数）：

- **实现**：`crates/asr/src/hans.rs`，基于「开放词典网」(kaifangcidian.com) CC-BY 3.0 单字对照表（`data/t2s.txt` 繁→简、`data/s2t.txt` 简→繁，`include_str!` 编译期嵌入，零运行时文件依赖）。仅转字形、不转地域用词（"愚能"转换）；简→繁一对多取数据首选（已消歧，如「发→發」）。
- **开关**：`config.yaml.output_simplified`（默认 `true`=简体）；`true`→繁转简，`false`→简转繁。
- **注入点**：`engine.rs::transcribe_with_vad` 返回前（offline 统一出口）+ `streaming_engine.rs::finish` 返回前（streaming 统一出口），在 corrector 之后、paste/入库之前。增量中间显示段不转换（短暂过程，最终输出归一化）。

## ASR 硬件加速与自动降级机制 (ASR Hardware Acceleration & Fallback)

为了最大化利用用户本机的 GPU 资源加速语音识别，同时避免因显卡驱动或算子不支持导致应用程序崩溃，系统在 `octopus-asr` 核心引擎中实现了一套手自动一体的硬件加速及平滑降级机制。

### 核心特性
- **手动控制开关**：在 `config.yaml` 中提供 `asr_hardware_accelerated` 字段（`bool` 类型，默认 `false`）。用户如果不需要加速，或者大模型加速不稳定时，可随时降级回退到纯 CPU 推理。
- **多平台加速后端支持**：通过 `ort` (ONNX Runtime) crate 的硬件加速接口，自动支持多平台主流 EP (Execution Provider) 注册：
  - **macOS**: 自动尝试使用 `CoreML` 执行提供商进行加速。
  - **Windows/Linux**: 自动尝试使用 `CUDA` 和 `DirectML` 进行 GPU 加速。
- **平滑降级机制 (CPU Fallback)**：
  在初始化推理 Session 时，若检测到 `asr_hardware_accelerated: true`，SessionBuilder 会动态尝试注册对应的 GPU EP。如果系统驱动不兼容、加速库文件缺失，或模型自身包含硬件加速器不支持的特殊算子（例如 `Qwen3-ASR` 由于含有复杂动态 Shape 算子，在部分平台的 CoreML 加速启动时会被拦截限制），构建器会捕获该错误、打印 Warning 日志，并**自动且无缝地重构出一个纯 CPU 的 Session**，保证语音识别服务不发生闪退或中断。
- **VAD 免加速策略**：
  由于 VAD (Silero VAD) 模型的体积极其微小 (1.8MB)，且对实时性要求极高。将其调度至 GPU 进行加速所产生的显存交互与上下文切换开销（Latency Overhead）远超加速本身带来的收益。因此，**VAD 推理固定运行在 CPU 端**，完全不受 `asr_hardware_accelerated` 字段的影响。

## 技术栈

- **推理引擎**: ONNX Runtime（通过 ort crate）；可选硬件加速——CUDA/DirectML/CoreML execution provider（由 `config.yaml.asr_hardware_accelerated` 控制，默认 `false`，注册失败自动回退 CPU），VAD 不受影响（固定 CPU）。config 经 `APP_CONFIG` OnceLock 缓存避免每次 session 构建重复读 yaml。详见 [spec](superpowers/specs/2026-06-16-archived-design.md)
- **音频处理**: cpal（录音）、rubato（重采样，含 denoise 48k 桥接）、nnnoiseless（RNNoise 降噪）、rustfft（各引擎 fbank STFT）、hound（WAV 读取）
- **Web 框架**: Axum + Tokio
- **桌面框架**: Tauri 2
- **模型加载**: HuggingFace Hub 本地缓存
- **嵌入式存储**: rusqlite（`bundled` feature，自带 SQLite C 库）— desktop 用，存识别历史 + 模型配置
