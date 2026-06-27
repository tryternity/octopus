# 架构概览

octopus 是一个基于 ONNX Runtime 的语音识别（ASR）工具集，支持多种 ASR 引擎和多种使用方式。

## 项目结构

```
octopus/
├── crates/
│   ├── infra/       # 基础设施 (octopus-infra)
│   ├── asr-local/   # 核心推理库 (octopus-asr-local)
│   ├── asr-cloud/   # 云端 ASR 协议层 (octopus-asr-cloud)
│   ├── clipboard/   # 剪贴板历史管理 (octopus-clipboard)
│   ├── llm/         # LLM 润色 (octopus-llm)
│   ├── cli/         # 命令行工具 (octopus-cli)
│   ├── server/      # HTTP/WebSocket 服务 (octopus-server)
│   ├── desktop/     # Tauri 桌面应用 (octopus-desktop)
│   ├── download/    # 模型下载 (octopus-download)
│   └── dlp/         # 视频音频下载 (octopus-dlp)
├── docs/            # 文档
└── usage.md         # 快速使用指南
```

## 模块说明

### octopus-infra（基础设施）

无项目内依赖的最底层 crate，承载跨 crate 共享的基础设施：`consts`（固定路径常量：VAD 模型 / 默认 ASR 模型目录）+ `paths`（`octopus_config_home()` 返回 `~/.octopus`，三端统一）+ `config`（`AppConfig`——应用配置统一 schema）+ `db`（SQLite 嵌入式存储，含 `app_config` 表（category 分组：`setting`用户配置 / `system`窗口位置等系统状态）/ `models` 表 / `transcriptions` 表 / `prompts` 表 / `clipboard_history` 表 + FTS5 虚表）。DB 迁移至 v6。`with_db` 为公开 API 供其他 crate 调用。`image_util`（PNG 原样保存 + WebP 有损压缩（`webp` crate）+ JPEG 有损压缩（`image` crate JpegEncoder），均接受 quality 参数）。

### octopus-asr-local（核心推理库）

ASR 推理的核心库，所有上层组件都依赖它。

| 模块 | 说明 |
|------|------|
| `config` | DB 模型配置加载（`AsrConfig`）、模型发现、引擎路由（`resolve_engine_in_config` 按 `{provider}:{category}:{model_name}` 3-part spec 解析）、全局默认引擎兜底（`resolve_active_engine`）、云引擎分类（`EngineCategory::Aliyun` / `EngineCategory::ByteDance`，由 `resolve_category` 按 provider 分支识别） |
| `audio` | WAV 读取、重采样（`resample_to` 一次性 / `AudioResampler` 流式，支持任意 from→to 速率，含 denoise 48k 桥接）、VAD 语音过滤 |
| `denoise` | 可插拔流式环境降噪后端（`FrameDenoise` trait，由 `denoise_mode` 选择）：`1`=RNNoise（`nnnoiseless`，纯 Rust 移植 Xiph RNNoise，内置默认模型，48kHz/FRAME_SIZE=480→频带特征+VAD/噪声/降噪 GRU→频带增益+OLA，GRU 状态跨帧保持）/ `2`=DeepFilterNet3（`Df3Backend` 包装 libDF v0.5.6 的 `DfTract` + tract 0.19，48kHz 全频带）。`DenoiseProcessor` 为 mode 分发器，采集层前置 |
| `vad` | Silero VAD 语音活动检测 |
| `whisper` | Whisper 离线识别（int8 三件套优先：encoder + dec_init + dec_past；decoder 层数 / D_MODEL 从 session 输出动态获取，支持 tiny/base/small 等不同规模模型（但仅 whisper-small.en 识别质量可用，tiny/base 经实测不可用故不入 seed）；**不支持 Large v3 / Turbo**——这些变体使用 128 mel bins，引擎硬编码 `N_MELS=80` + 静态 80×201 filterbank，`WhisperEngine::new` 加载时校验 encoder 输入维度，遇到 128 mel 会提前 fail 给出明确错误而非踩 ONNX shape mismatch 崩溃；auto-language 两步式检测：先喂 `[sot]` 预测语言 token，再拼完整 `[sot, lang, transcribe, no_ts]` prompt；**特殊 token 强制查询**——各变体 token ID 不同（.en 整体偏移 -1），`token_to_id` 用 `ok_or_else(bail!)` 强制查询，失败立即报错而非静默 fallback 到错误的 multilingual ID；**短音频提早结束**——compute_mel 会把音频 0 填充到 30s，若 VAD 只传入 2s 片段剩余 28s 为静音，原硬编码 `max_tokens=448` 会让模型在静音段幻听（重复最后一句话 / “谢谢观看”），现按实际音频时长 `max_tokens = (seconds × 6 + 10).min(448)` 动态限制解码步数，.en 模型平均 ~6 text tokens/秒；**Mel 频谱 center=True reflect 填充**——与 OpenAI `torch.stft` 默认行为一致，frame t 覆盖 `[t×hop - n_fft/2, t×hop + n_fft/2)`，左右越界样本按 PyTorch `pad_mode="reflect"` 反射填充 200 采样，使 frame 0 中心对齐 sample 0，避免整个时间轴偏移 12.5ms 影响首音节识别） |
| `sensevoice` | SenseVoice 离线识别 |
| `paraformer` | Paraformer 离线识别（fbank: hamming 窗 + DC offset + pre-emphasis） |
| `qwen3_asr` | Qwen3-ASR 离线识别 |
| `zipformer` | Zipformer 离线识别 |
| `moonshine` | Moonshine 离线识别（纯 ONNX 4-session 流水线：preprocess → encode → uncached_decode → cached_decode 循环 + KV cache；英语） |
| `streaming_paraformer` | Paraformer 流式识别（增量式 fbank: povey 窗 + DC offset + pre-emphasis + 跨帧状态） |
| `streaming_zipformer` | Zipformer 流式识别 |
| `corrector` | 基于拼音映射和 Bigram 转移概率的轻量级中文拼音纠错与热词校正 |
| `hans` | 简繁体字形转换（单字级，开放词典网 CC-BY 3.0 对照表编译期嵌入）；按 `output_simplified` 归一化 ASR 输出 |
| `pipeline` | 批处理 pipeline 编排（阶段1 新增）：`PipelineConfig`（language/correct/simplify/ngram）+ `transcribe_batch`（VAD 分段 → 逐段转写 → 纠错 → 简繁归一化，收编自 `transcribe_with_vad`，纠错/简繁参数化为 cfg 字段）；`transcribe_with_vad` 退化为从 app_config 构造 cfg 的薄包装（desktop 向后兼容）；cli 经 `AsrEngineManager::transcribe_batch` 复用同一编排 |
| `streaming_engine` | 流式 ASR 引擎抽象：`StreamingSession`（Paraformer / ZipformerCtc / ZipformerTransducer 增量流式，`&self` + 内部 Mutex）；阶段2a impl `StreamingEngine` trait |
| `streaming_runner` | 流式编排 helper（阶段2a 新增）：`StreamingRunner`（持 `Box<dyn StreamingEngine>` + VAD + 静音/标点状态）收编 VAD 静音检测 + 标点触发 + accept/flush/finish；`TranscriptEvent`（Partial/Committed/Final/Error）+ `StreamingEngine` trait（local `StreamingSession` impl，cloud 2c-2）。denoise/resample 不入（留 `audio.rs`，输入为已降噪 16k 样本） |


**数据流（离线）：**
```
音频文件/WAV → read_wav_16k → [VAD 过滤] → 引擎.transcribe → 文本
```

**数据流（流式）：**
```
麦克风 → PCM chunk → resample_to_16k → 引擎.accept_samples → [partial]
                                    └─ 静音≥0.5s → 引擎.flush(insert_comma=true)（padding 冲刷尾音 + 即时逗号）→ [partial]
                                                              → engine.finish → [final]
```

### octopus-asr-cloud（云端 ASR 协议层 + 批引擎）

云端 ASR（Aliyun/ByteDance/Tencent/Baidu 4 provider）WSS 协议层 + 批引擎，cli/server 批处理转译音频文件可选云端 API（不必只靠本地 onnx）。依赖 `octopus-asr-local`（**单向**，asr 不依赖 cloud，避免循环——本地/云端分流在 cli 层）。

| 模块 | 说明 |
|------|------|
| `cloud_types` | `PcmFrame`/`StreamEvent`/`CloudStreamHandle`（`new`/`push_pcm`/`finish`/`close_async`，迁自 desktop；`CLOUD_CLOSE_TIMEOUT_SECS=8`）|
| `aliyun_stream`/`bytedance_stream`/`tencent_stream`/`baidu_stream` | 4 provider WSS 协议层（建连/鉴权/帧编解码/WS 循环），1:1 复刻 desktop `*_stream.rs`，仅改造 `open()`：去 `tauri::async_runtime::RuntimeHandle` 参、`rt.spawn`→`tokio::spawn`；协议字节级零差异，单测随源文件搬入 |
| `config` | `resolve_*_config`（从 `AppConfig.asr.{provider}` 取 `ModelEntry` 校验 `secret_key`）+ `open_cloud_session(spec, lang, pre_roll) -> anyhow::Result<CloudStreamHandle>`（按 `resolve_engine_category` 分发到 4 provider `open`）|
| `batch` | `CloudBatchEngine impl OfflineAsrEngine`：`from_spec`（3-part 云端 spec 校验 + 建 tokio runtime，不查 DB/不连网）；`transcribe` = 单段→单 WSS session→完整文本（`block_on`：open + 分块 push_pcm + close_async，分段由上层 `transcribe_segments` 自动完成）；`skip_corrector()=true`；`is_cloud_spec`（`parse_model_spec` 3-part provider 前缀判云端，不查 DB）|

> desktop 协议层副本已删（2026-06-25 cloud-dedupe，ff-merge main `a16c98f`）：删 `*_stream.rs`×4 + `cloud_types.rs`（1868 行），`CloudPipelineEngine`（`cloud_pipeline.rs`，desktop pipeline 壳保留）+ `pipeline.rs` trait + `engine_aliyun.rs` 改指 `octopus-asr-cloud` 协议层，协议层单源。

### octopus-cli（命令行工具）

通过 clap 提供 6 个子命令：

| 命令 | 说明 |
|------|------|
| `devices` | 列出可用麦克风 |
| `config` | 显示模型发现信息 |
| `transcribe` | WAV 文件离线识别；`--model` 支持 3-part 云端 spec（`provider:category:model_name`，provider=aliyun/bytedance/tencent/baidu）→ `octopus-asr-cloud` 的 `CloudBatchEngine`（WSS，`skip_corrector=true`），否则本地 onnx；两端都经 `asr::pipeline::transcribe_batch`（VAD 分段 + 纠错 + 简繁）。分流在 cli 层（`pipeline::run` 用 `is_cloud_spec`） |
| `e2e` | 麦克风实时识别（离线/流式） |
| `stream-test` | WAV 文件流式识别测试 |
| `download` | 从 HuggingFace 下载模型到 `~/.octopus/models/<repo>/`（薄封装 `octopus-download`：`--include`/`--exclude` glob 过滤、`--mirror` 镜像；镜像优先级 `--mirror` > config `download_mirror` > 官方源） |

### octopus-server（HTTP 服务）

基于 Axum 的 Web 服务，提供 REST 和 WebSocket 接口。两条 ASR 路径均走 asr helper（阶段3，2026-06-26）：批处理 `/transcribe` → `AsrEngineManager::transcribe_batch` + `PipelineConfig`（VAD 分段 + 纠错 + 简繁）；流式 `/ws/stream` → `pipeline.rs::WsStreamSession`（薄包 `StreamingRunner`，VAD 静音/标点内部收编）→ `event_to_json` 回推 `TranscriptEvent` `{type,text}`。`pipeline.rs`（WS↔runner 桥接 + 序列化，纯逻辑可单测）+ `main.rs`（路由 + WS/HTTP 胶水）。

```
Client ──HTTP POST──→ /transcribe ──→ transcribe_batch（asr::pipeline）──→ JSON 响应
Client ──WebSocket──→ /ws/stream  ──→ WsStreamSession(StreamingRunner) ──→ {type,text} JSON
```

### octopus-clipboard（剪贴板历史管理）

独立的剪贴板历史核心库，仅依赖 `octopus-infra`。基于 `clipboard-rs`（跨平台剪贴板读写 + 监听），替代了原来的 `tauri-plugin-clipboard-manager`。

| 模块 | 说明 |
|------|------|
| `model` | 数据结构：`ItemType`（Text/Image/File）/ `Source`（Clipboard/Asr）/ `ClipboardItem`（含 `ImageMeta`/`FileMeta`/`AsrMeta`）/ `QueryFilter`（6 种过滤 + 分页 + 搜索）|
| `handle` | `ClipboardHandle`：`Mutex<ClipboardContext>` 全局单例（Windows 防锁竞争）+ `AtomicBool` suppress flag（区分 ASR 写入与外部复制，watcher 跳过自身写入） |
| `watcher` | `ClipboardWatcher`：后台线程跑 `ClipboardWatcherContext::start_watch()`（阻塞），`on_clipboard_change` 回调检查 suppress flag → 判断类型（files > image > text 优先级）→ 去重 → 存 DB → 通知前端 |
| `store` | DB CRUD：`insert_clipboard_item` / `insert_asr_item`（source=asr，关联 transcription_id）/ `query_history`（LIKE 搜索 + 6 种过滤 + 分页）/ `toggle_favorite` / `delete_item`（+ 删除计数器 `track_deletes`）/ `delete_by_transcription_ids`（级联删除：Settings 删转译记录时同步删剪贴板引用）/ `clear_history`（保留收藏）/ `rebuild_fts_index`（FTS5 索引重建，启动时 + 删除计数达 10 自动调用）+ 去重（hash / text） |
| `image` | PNG 编码（`image` crate）+ SHA-256 去重 + 缩略图 240×240（Lanczos3）+ 孤立 blob 回收。图片存 `~/.octopus/clipboard_images/<hash>.png` |
| `cleanup` | 自动清理：按天数（默认 30）+ 按数量（默认 1000）删除非收藏记录 + 孤立 blob 回收 + FTS5 索引重建。**注意：`run_cleanup` 已实现但尚未接入定时调用**；FTS5 索引维护已单独接入（启动时 rebuild + 删除计数器达 10 自动 rebuild，见 store.rs） |

**监听机制（clipboard-rs 内置）：** macOS 轮询 `NSPasteboard.changeCount`（500ms）；Windows 事件驱动 `AddClipboardFormatListener`；Linux X11 XFixes 事件驱动；Linux Wayland 两级轮询（MIME 类型 + text 内容，500ms）。

**ASR 集成：** `coordinator.rs::do_paste` 中先调 `store::insert_asr_item`（写 DB source=asr）再调 `paste::paste`（写剪贴板，suppress flag 阻止 watcher 重复记录）。**级联删除：** Settings 删除转译记录时，`delete_history` 同步调 `delete_by_transcription_ids` 清理剪贴板中引用该记录的条目；反向不级联（剪贴板删除语音条目只删 `clipboard_history` 行，外键 `ON DELETE SET NULL` 处理引用置空）。

**DB 表：** `clipboard_history`（全字段：item_type/source/content/search_text/is_favorite/created_at + image 元数据 blob_hash/width/height/has_thumbnail + file_count + is_rich + ASR 元数据 transcription_id/polish_status/engine/model）+ `clipboard_history_fts`（FTS5 虚表，trigram tokenizer）+ 3 触发器自动同步。**FTS5 索引维护**：FTS5 external content table 的 DELETE 触发器只移除逻辑索引，`_data` 表 b-tree 页不收缩，删除越多空洞越大。维护策略——启动时 rebuild 一次（`main.rs` setup）+ 运行中删除计数器（`AtomicU32`，阈值 10）达 10 自动 rebuild + 清零。

### octopus-desktop（桌面应用）

基于 Tauri 2 的桌面应用，支持系统托盘、全局快捷键、结果窗口、流式识别。

**识别模式：**

| 模式 | 引擎 | 说明 |
|------|------|------|
| 流式 | Paraformer, Zipformer | 边说边识别，600ms tick 驱动 |
| 离线 | SenseVoice, Whisper, Qwen3-ASR | VAD 分段伪流式，300ms tick 驱动，阈值可配置 |

**窗口管理：**

| 窗口 | 用途 |
|------|------|
| `result_window` | 识别结果展示（可拖拽、多行滚动、透明无边框、置顶）。顶部悬停工具栏：鼠标移入展开（窗口高度 116→148px），移出收起；工具精简为 5 个——**关闭**（首位，放弃内容保留 DB 记录）/ 系统设置 / 降噪模式 / 润色模式 / 立即润色 / 编辑（编辑态追加取消/保存）。语音模型和润色模型入口已移至 Settings 页面（模型太多，下拉空间有限）。由 `app_config.hide_toolbar`（默认 `true`）控制：`true`=hover 显隐，`false`=始终显示。**运行时切换立即生效**：设置窗口改 `hide_toolbar` → emit `config-changed` 事件 → result window 的 `refreshActive()` 双向切换。**历史**：曾同时存在 `recording_overlay` 窗口（独立 WebView 渲染进程），UI 统一到 result_window 后 overlay 已废弃。 |
| `settings_window` | 独立设置窗口（原生标题栏、圆角、可调大小）。四页面侧边栏布局：识别记录 / 系统设置 / 模型管理 / 提示词。React 组件化，表单用 react-hook-form。窗口位置记忆。 |
| `clipboard_window` | 剪贴板历史浮窗（300×600，无边框圆角透明置顶，Alt+V 唤起，窗口位置记忆）。顶部标题栏（X + 「剪贴板」 + Pin），搜索框 + 6 类过滤（全部/语音/文本/图片/文件/收藏，纯图标 tooltip），列表（hairline 分隔线，ASR 条目左侧 voice 色条，hover 显示收藏/删除/保存/打开按钮）。单击选中不关闭，双击复制到剪贴板（用户手动 Cmd+V，不模拟粘贴）。图片条目可保存到 `~/Downloads/octopus/`（自定义浮层选格式 JPEG/WebP/PNG + 质量，默认 JPEG 85%，可选保存后打开文件夹）。 |

**macOS 动态激活策略（Dock 图标显隐）：** 应用启动即 `Accessory` 模式（无 Dock 图标，纯托盘应用）。用户打开设置窗口时 `open_settings` 切 `Regular`，并经 `set_dock_icon()` 用 `objc2` 手动 `setApplicationIconImage`（release 裸二进制无 .app bundle，Tauri 仅 debug 自动设图标，故需手动设 Dock + 应用图标）；设置窗口 `Destroyed` 事件触发 `on_settings_closed` 切回 `Accessory`。`#[cfg(target_os = "macos")]` 条件编译，Windows / Linux 无此逻辑。

**窗口加载就绪（ready）机制：** 结果窗 webview 首次加载有延迟，若后端在页面就绪前 `emit('show-result')`，事件丢失导致「文本不显示 / 不弹窗」。`result_window.rs` 以 `WINDOW_READY`（AtomicBool）+ `PENDING_TEXT`（Mutex<Option<String>>）兜底——未 ready 时暂存文本，前端 `index.html` 加载完成后发起 `result_window_ready` Tauri command → 后端置 ready 并冲刷积压文本。`show_result` / `update_result` 把「判 ready + 写 pending」收进同一把 `PENDING_TEXT` 锁，与 `result_window_ready` 的 store(true)+take 互斥，消除启动首帧 TOCTOU 文本滞留。**`show_result` 的物理 `window.show()` 无条件执行**（不受 ready 门控，仅 `emit('show-result')` 受门控）——冷启动首启 webview 未 ready（走 pending 分支）时按快捷键也能立即弹窗，可见窗口的 webview 优先首绘亦加速 ready；`#container` 默认 `opacity:0`，提前 show 不产生空窗闪烁。

**核心状态机（Coordinator）：**
- 单线程 mpsc channel 串行化所有事件
- 流式模式：Streaming → (StoppingPolish) → (Polishing) → Pasting
- 离线模式（VadSegmented 伪流式）：VadSegmented → WaitingCompletion → (StoppingPolish) → (Polishing) → Pasting
- 云端流式模式（cloud feature，VAD-gated per-utterance streaming）：Streaming（cloud，`CloudPipelineEngine`，独立 100ms tick 线程）→ (StoppingPolish) → (Polishing) → Pasting；stop 时 close 在飞 → `CloudClosing` 中间态
- **StoppingPolish（Toggle 停止时立即润色仍在途）**：若用户点了「立即润色」后 LLM 未返回就 Toggle 结束录音，进入 `StoppingPolish { transcript }` 等待 `Command::PolishDone`，完成后按 `polish_mode` 走 final 路径。修复原 bug：原实现 `clear_polish_pending` 后走 final 路径，导致立即润色结果被 stage 切换丢弃 + `polish_mode=0` 时最终润色被跳过 → 只粘贴原文。**优化**：若 polished 非空且无新增 ASR（`has_increase=false`），跳过最终润色直接 paste（mode=1/2 也跳过），避免平白多一次 LLM 调用
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
    ├─ Streaming（本地流式 [`LocalPipelineEngine`]→[`asr::StreamingRunner`] / 云端流式 [`CloudPipelineEngine`]，
    │   coordinator 经 [`desktop::StreamingPipeline`]→`Box<dyn StreamingPipelineEngine>`，阶段2c-1/2c-2）：
    │     pipeline.tick(&samples, &mut transcript) → Vec<PipelineEvent>（2d，原 changed:bool）
    │       pipeline 内（承载层，local/cloud 共享）：engine.tick → TranscriptEvent（Partial/Committed/Final/Error）
    │         → Partial/Committed 幂等 set_full（changed=true）；Final 无条件覆盖；Error warn+stash last_error（下 tick 注入 PipelineEvent::Error，2d）
    │       · local engine：runner.push_samples → VAD 静音检测 → accept_samples(→Partial)
    │           → 累积静音 ≥0.5s → flush(true) 插逗号(→Committed)；stop 用 finish_with_tail(→Final)
    │     coordinator 端：dispatch_tick 调 pipeline.tick → apply_pipeline_events 路由（PersistRaw→DB / Emit→update_result /
    │       Polish→check_and_trigger_polish / Error→update_result），2d 统一事件循环（删原 handle_streaming_tick）
    │
    ├─ VadSegmented（本地离线引擎，[`desktop::VadSegmentedPipeline`]（pipeline.rs，2c-3 收编原 coordinator 逻辑））：
    │     pipeline.tick(&samples, &mut transcript) → Vec<PipelineEvent>（2d）
    │       run_tick：audio_buffer.extend + compute_speech_chunks(vad)（检测 VAD 跨 tick 有状态累积）
    │       → 静音 ≥ segment_silence / 持续 ≥ SEGMENT_DURATION_S（20s 常量）：
    │           filter_speech_from_buffer(filter_vad, send_buffer)  // 过滤 VAD（每段 reset）
    │           → spawn_blocking(engine.transcribe) → mpsc rx 按 seq 有序回填 completed_results（2c-3 删 TranscriptionDone）
    │       changed → [PersistRaw{vad_segmented}, Emit]；segment_cut → [Polish{INFINITY}]（段边界润色）
    │     coordinator 端：dispatch_tick → apply_pipeline_events 路由（2d，删原 handle_vad_segmented_tick）
    │
    └─ Streaming · cloud engine（[`CloudPipelineEngine`](../crates/desktop/src/cloud_pipeline.rs)，cfg cloud，阶段2c-2；
          coordinator 经统一 `dispatch_tick` 驱动（2d，原 handle_streaming_tick 已删），cloud 走独立 100ms `CloudStreamingTick` 线程）：
          engine 内 pre_roll_buffer 滚动追加 samples（保留后 200ms = CLOUD_PREROLL_BUFFER_SAMPLES）
          compute_speech_chunks(vad, &samples) → onset 检测（≥2 speech chunks）
          ├─ 无活跃 WSS + onset（连续 2 tick 确认，消除噪声脉冲）：
          │     cloud_pipeline::open_cloud_session(asr_engine, language, pre_roll) → CloudStreamHandle
          │       ├─ Aliyun：resolve_aliyun_config → aliyun_stream::open
          │       ├─ ByteDance：resolve_bytedance_config → bytedance_stream::open
          │       ├─ Tencent：resolve_tencent_config → tencent_stream::open
          │       └─ Baidu：resolve_baidu_config → baidu_stream::open
          │     session.push_pcm(&samples)
          ├─ 有活跃 WSS + 持续语音：
          │     push_pcm(&samples)（→ s16le / base64 → WS frame）
          │     drain_cloud_session → try_recv_text：
          │       ├─ StreamEvent::Text(partial) → current_partial = partial（**预览层，不进 transcript/DB**，仅 display）
          │       └─ StreamEvent::Finished → committed_text 逗号拼接 partial → 产 Committed 事件
          │         （承载层 set_full + coordinator DB + check_and_trigger_polish；!closing && !speaking → session.take drop）
          └─ 有活跃 WSS + 静音 ≥ pause_polish_threshold_ms：
                session.finish()（**非阻塞**，发 finish-task / 末帧负包）
                → is_closing = true（后续 tick drain 最终结果，不阻塞 coordinator）
          coordinator 端（cloud 分支，2d 事件流）：dispatch_tick → apply_pipeline_events 路由——changed(=Committed) →
            PersistRaw(DB) + Polish(停顿润色)；**每 tick Emit**（display + current_partial 预览，预览不进 DB）；
            WSS 开启失败 / StreamEvent::Failed → 承载层 last_error → 下 tick PipelineEvent::Error → update_result（删原 take_error）。
            stop → take_close_handle → spawn close_async → `Stage::CloudClosing`（close 留 coordinator，不可消除）
  ```

  **关键不变量**：
  - **降噪在 drain_samples 内部完成**——三种 stage 拿到的 `samples` 都是 16k 已降噪（或降级直通）样本；VAD 与 ASR 用同一份降噪后信号，避免参数 / 状态不一致致 VAD 误判而 ASR 准的解耦 bug。云端引擎（cloud）的 pre-roll 同样从 drain_samples 取，云端收到的是干净音频。
  - **降噪 GRU 与 VAD LSTM 状态语义相反**：降噪 GRU **跨 tick / 跨段连续保持**（`flush=false`，噪声估计是连续物理过程，仅会话 `start()` 才 reset）；检测 VAD **跨 tick 有状态累积**（看完整流，稳语音/静音边界）；过滤 VAD **每段 reset**（独立冷启动，等价每段新 VAD 但复用 ONNX Session）。详见「VAD 分段切分策略」。
  - **降级不 panic**：`denoise_mode=0` / 后端模型缺失 / 单帧推理失败 → `process_pipeline` 走直通分支（原生→16k），仅 warn 日志，识别继续不阻断录音。
  - **cloud engine 的 VAD 用法与 VadSegmented 一致**：同一个 `compute_speech_chunks`（迁自 coordinator，现 `pipeline::compute_speech_chunks` pub(crate)）+ `SileroVad` 检测 onset，但**不切分过滤**（不调 `filter_speech_from_buffer`）——云端服务端自己有切句逻辑（DashScope server-side `max_sentence_silence` / 豆包 `show_utterances`），客户端 VAD 只负责「何时开 / 何时关 WSS」的生命周期门控。**onset 抗噪**：连续 2 个 tick（~200ms）检测到语音才开 WSS（`speech_confirm_count`），消除单次噪声脉冲导致的空 session 误触发。
- **云端流式（cloud feature，`CloudPipelineEngine`，阶段2c-2）**：当 `is_cloud_engine(cfg)`（`asr_engine` 解析 category=Aliyun / ByteDance / Tencent / Baidu）时启用——coordinator 在 `handle_toggle` 建 `CloudPipelineEngine`→`Stage::Streaming`（与本地流式同 Stage，cloud 走独立 100ms `CloudStreamingTick` 线程）。与本地 Streaming / VadSegmented 不同——**不调用 `TranscriptionEngine::transcribe`**，而是 `CloudPipelineEngine`（[`cloud_pipeline.rs`](../crates/desktop/src/cloud_pipeline.rs)）直接管理一条云端 WebSocket 长连接，由 VAD 决定连接生命周期，`tick` 产 `Vec<TranscriptEvent>` 由 `StreamingPipeline` 承载层 set_full。**四个云端 provider** 统一返回 [`CloudStreamHandle`](../crates/desktop/src/cloud_types.rs)（含 `push_pcm`/`finish`/`try_recv_text`/`close_async` 共用方法）：
  - **Aliyun**（[`aliyun_stream.rs`](../crates/desktop/src/aliyun_stream.rs)）：阿里云百炼 DashScope。**三套协议自动分发**（[`is_qwen_realtime_endpoint`] 按 endpoint 路径分流）：
    - **Fun-ASR / Paraformer**（`/api-ws/v1/inference`）：任务型协议（`run-task` → 二进制 PCM → `finish-task` → `result-generated`（按 `sentence_id` + `sentence_end` 跨句累积）→ `task-finished`）
    - **Qwen-ASR Realtime**（`/api-ws/v1/realtime`）：OpenAI Realtime 风格会话协议（`session.update` → base64 PCM via `input_audio_buffer.append` → `session.finish` → `conversation.item.input_audio_transcription.text`/`completed`）
  - **ByteDance**（[`bytedance_stream.rs`](../crates/desktop/src/bytedance_stream.rs)）：字节跳动豆包大模型 ASR 双向流式（`bigmodel_async` 优化版）。二进制帧协议（4B header + payload），gzip 压缩，固定 endpoint `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`，鉴权经 `X-Api-Key` + `X-Api-Resource-Id` 握手 headers。`source`=Resource ID（如 `volc.bigasr.sauc.duration`），`secret_key`=API Key。详见 [spec](superpowers/specs/2026-06-21-bytedance-asr-design.md)。

  **生命周期**（`CloudPipelineEngine::tick` 内编排，产 `TranscriptEvent` 不直接写 transcript/emit）：① 语音 onset（连续 2 tick 确认）→ 根据 `EngineCategory` 调 `cloud_pipeline::open_cloud_session`（内部分派到对应 provider 的 `xxx_stream::open`，建连 + 初始化 + 推 100ms pre-roll）；② 持续语音 → `push_pcm` 推帧 + `drain_cloud_session` 把 partial 写到 engine 自持的 `current_partial`（**预览层不碰 transcript/DB**，coordinator display 拼 transcript + current_partial）；③ 静音 ≥ `pause_polish_threshold_ms` → `finish()`（**非阻塞**）→ `is_closing=true` → 后续 tick drain 最终结果；④ `StreamEvent::Finished` → `committed_text` 逗号拼接 `current_partial` → 产 `Committed` 事件（承载层 set_full + coordinator DB + `check_and_trigger_polish`）→ drop session。**四个 provider 共享 `PcmFrame` / `StreamEvent` / `CloudStreamHandle` 类型**（定义在 [`cloud_types.rs`](../crates/desktop/src/cloud_types.rs)），`CloudStreamHandle` 的 `push_pcm` / `finish` / `try_recv_text` / `close_async` 为共用实现。**partial 与 transcript 分离**（消除 partial 覆盖历史文本的 bug）、**非阻塞 finish**（消除 `close()` 的 `block_on` 冻结 coordinator 线程的 bug）。**DB INSERT 时机**：cloud 只在 commit（Finished→`Committed`，承载层 `changed=true`）时 `update_transcription_raw`（INSERT/UPDATE raw_text）——与本地 Streaming 路径每次 accept_samples 都 INSERT 不同。如果整个录音过程中从未触发 Finished（用户没停顿够就 Toggle stop / 点立即润色），记录从未创建 → 后续 `Finalize` / `UpdatePolished`（均为 UPDATE WHERE id=?）静默 0 行，数据丢失。**修复**：`finalize_cloud` 在 append partial 后、`start_final_polish_or_paste` 之前先调 `update_transcription_raw` 确保 INSERT；`handle_polish_now` 在 `take_polish_input` 之前也调 `update_transcription_raw`（本地路径已 INSERT 为 no-op，cloud 路径补 INSERT）。**tick 间隔 100ms**，**pre-roll 滚动缓冲 200ms**。Toggle 停止时若 WSS 仍活跃 → spawn `close_async`（**非阻塞**，审查三1）+ 进 `Stage::CloudClosing`（持 transcript/current_partial），close 完成回 `Command::CloudStreamingDone { text, session_id }` → `handle_cloud_streaming_done` 校验 `transcript.id == session_id`（跨会话护栏，见下）后 `set_full` + finalize → 走润色/粘贴。详见 [spec](superpowers/specs/2026-06-19-archived-design.md)（§ dashscope-streaming-design，已归档）。
- **最终润色异步化**：停止后若启用润色（mode=1/2），`start_final_polish_or_paste` 进入 `Stage::Polishing`（spawn 独立线程跑 LLM 网络请求，托盘显「处理中」、结果窗显「最终润色中」），LLM 完成回调 `Command::FinalPolishDone` 后 `do_paste` 落地；未启用润色则直接 `do_paste`。**润色期间协调器线程不阻塞**，`Cancel`（Esc）可即时回滚 Idle、丢弃在途结果，`Toggle` 被互斥忽略（防并发缓存污染）。**跨会话护栏**：`Command::FinalPolishDone` 携带 `session_id`（= 发起润色时的 transcript.id），`handle_final_polish_done` 校验当前 Polishing id 匹配才落地——Cancel+重开+再润色时旧结果匹配新 Polishing 的污染被拦（与 `PolishDone` 同理）。Polishing 仅持 `id` + `raw_text`（不需 Transcript 其余字段）
- **粘贴异步化（`do_paste`）**：`do_paste` 先同步 `show_result` + 置 `Stage::Pasting`（状态机线程），再把真正的落库粘贴（`paste::paste`——含 enigo 键盘模拟 + 焦点切换 `sleep`）投递到 `tauri::async_runtime::spawn` + `tokio::task::spawn_blocking`，完成后回 `Command::PasteDone`——粘贴期间不占用 Tauri UI 主线程、不阻塞协调器线程。**macOS 键盘模拟线程安全**：`paste_via_clipboard` 的 V 键用固定虚拟键码 `Key::Other(9)`（`kVK_ANSI_V`）而非 `Key::Unicode('v')`——enigo 0.6.1 的 `Key::Unicode` 在 macOS 走 `get_layoutdependent_keycode`（循环调用非线程安全的 Carbon `TIS*`/`UCKeyTranslate` API），在 `spawn_blocking` 非主线程执行会触发 SIGTRAP（`Trace/BPT trap: 5`）；`Key::Other` 直接当 keycode 用绕过 layout 查找。详见 [spec](superpowers/specs/2026-06-17-archived-design.md)
- **取消录音（Cancel）**：结果窗按 Esc → 前端 `invoke('cancel_recording')` → `coordinator::cancel_recording` Tauri command → `Coordinator::cancel` 发 `Command::Cancel`。`handle_cancel` 跨阶段生效——Streaming 停采集 + reset 引擎，VadSegmented 停 tick + 停采集，WaitingCompletion / Polishing 丢弃在途结果，统一回 `Idle` + 隐藏 result 窗 + 托盘置 Idle（Idle 下为 no-op）。Esc 同时 `currentWindow.hide()` 提供即时反馈（区别于运行时配置子系统的 4 个命令，`cancel_recording` 定义在 `coordinator` 模块）。**取消时清理 DB 脏数据（2026-06-21 审查修复）**：原实现仅回 `Idle` 不删除已 `INSERT` 的过程记录，导致垃圾数据遗留。修复后 `handle_cancel` 检查当前 `transcript.db_inserted()` ——`true` 则经 `DbCommand::Delete { id }` 后台删除该条未完成记录；`Polishing` / `Pasting` 阶段（仅有 `id` 无 transcript）直接删除（这两个阶段意味着已识别但被用户取消，不应保留）。**`StoppingPolish` 阶段**（Toggle 停止时立即润色仍在途）：Cancel 丢弃在途润色结果 + 删除 DB 脏数据（同其他阶段的 Cancel 语义）。与 Discard 的「保留识别历史」行为对称
- **放弃识别（Discard）**：工具栏「关闭」按钮（首位，close.svg 图标）→ 前端 `invoke('discard_recording')` → `Coordinator::discard` 发 `Command::Discard`。`handle_discard` 与 Cancel 共享停止逻辑（停采集 + reset 引擎 / 断 WSS），但**额外 finalize DB 记录**（`DbCommand::Finalize`：`raw_text` + `duration_ms` + `polish_status="off"` 入库，保留识别历史），**跳过 `do_paste`**（不粘贴、不入剪贴板）。与 Cancel 的本质区别：**Cancel 丢弃一切并删除 DB 过程记录（`DbCommand::Delete`），Discard 保留识别历史**。`Pasting` 阶段 no-op（粘贴进行中无法撤回），`Polishing` 阶段丢弃润色结果（`FinalPolishDone` 到达时若 stage 已回 Idle 或属另一会话的 Polishing，由 `session_id` 护栏丢弃）。`discard_recording` 同样定义在 `coordinator` 模块
- **音频采集按需启停（替代常驻，修复菜单栏麦克风指示灯常亮）**：`cpal::Stream` 所有权收归 `SharedAudioState`（`Mutex<Option<cpal::Stream>>`），不再 `std::mem::forget` 泄漏保活——**每次录音 `start()` 现场建流（`build_stream`）+ play，`stop()` pause + drop（take 出 Option 在本线程析构）**：空闲期无流、菜单栏麦克风指示灯灭、不触发麦克风权限；录音期间流持续播放、回调内 `is_recording` 作冗余守卫。**Send-safety（已根治）**：`cpal::Stream` 为 `!Send + !Sync`，但 SharedAudioState 的 Arc 被 `move` 进 Coordinator 的 `std::thread::spawn` 循环闭包、仅该线程独占持有（`audio` 不在 Coordinator 结构体字段），故 Stream 的建（start）/ 播（play）/ 停（stop）/ 析构（stop take-drop 或循环线程退出）全程同线程、无跨线程访问；cpal 回调线程只持有独立 clone 的 `Arc<Mutex<Vec>>`/`Arc<AtomicBool>`。`unsafe impl Send/Sync` 在此前提下 sound（注释记录该不变量）。建流失败由 `start()` 返回 `Err`、上层降级。**多采样格式支持**：`build_stream` 按 `config.sample_format()` 分派 F32 / I16 / U16 三类——F32 直接取均值；I16 → `s as f32 / i16::MAX`；U16（部分 Linux 驱动 / 老旧设备）→ `(s - 32768) / 32768`（center-zero 还原）。cpal 错误回调（如设备中途断开）从 `debug!` 提升至 `error!` 日志级别，便于故障排查
- **音频初始化防闪退**：`AudioRecorder::open()` 仅校验麦克风存在（失败 `log::error` + 仍持有静音占位 `SharedAudioState`，应用进托盘不 `expect` panic）；真正的 `build_stream` 推迟到首次 `start()`，建流失败（无设备 / 权限拒绝 / 占用）由 `start()` 返回 `Err`、上层降级（采样恒空 → 识别静默 → 空文本回 `Idle`），改配置后重启恢复
- **流式重采样器缓存**：非 16kHz 麦克风源的流式重采样经 `crates/asr/src/audio.rs` 的 `AudioResampler`（有状态 `rubato::FftFixedIn` + 跨帧 leftover 缓冲）——`desktop::SharedAudioState` 持 `Mutex<Option<AudioResampler>>`，源速率不变时**复用同一规划器**（避免每 ~300ms tick 的 FFT planner 重规划开销，并保留滤波器跨帧状态保边界 glitch-free），仅 `stop` 时 `flush` 补零吐尾 + 置 `None`；`drain_samples` 不 flush。`AudioResampler` 经编译期断言 `Send+Sync`（固化 `SharedAudioState` 的 `unsafe impl` 前提，防 rubato 升级引入非 Send 字段静默退化为 UB）
- **环境降噪（可插拔后端，采集层前置）**：麦克风音频送入 VAD/ASR 前，经 `crates/asr/src/denoise.rs` 的 `DenoiseProcessor`（mode 分发器，对外接口与旧 RNNoise-only 一致）降低背景噪声。降噪为**可插拔后端**（`FrameDenoise` trait，`process_frame(&[f32;480], &mut [f32;480])` 用 `[-1,1]` 单声道契约），由 `app_config.denoise_mode` 选择：
  - `0` = 关闭（直通，零开销）。
  - `1` = RNNoise（`RnnoiseBackend`，`nnnoiseless` 纯 Rust 移植 Xiph RNNoise，内置默认模型，48kHz FRAME_SIZE=480(10ms)→频带特征 + VAD/噪声/降噪 GRU → 频带增益 → iSTFT+OLA）。**默认**。
  - `2` = DeepFilterNet3（`Df3Backend`，libDF v0.5.6 的 `DfTract` + tract 0.19，48kHz 全频带，编译期内嵌 ~7.9MB `DeepFilterNet3_onnx.tar.gz` 模型）。质量最佳（干净语音 gain≈0.96、带噪 gain≈0.60、RTF≈0.015–0.036）。DF3 **懒加载**：`new(mode=Df3)` 仅占位，首次 `process_samples` 才加载模型（避免构造热路径阻塞）。
  - 缺省 `1`（`default_denoise_mode()`）。`denoise_mode: u8` 亦可由工具栏运行时切换（`set_denoise_mode` 命令）并持久化回 DB `app_config` 表。

  **帧边界隔离 ndarray 版本**：libDF（deep_filter）依赖 ndarray 0.15，asr 现有 ndarray 0.17（ort/whisper 等）。Cargo 允许同 workspace 共存（不同 major）。`FrameDenoise` trait 只用原生 `&[f32]`/`&mut [f32]`，绝不暴露 ndarray 类型；`Df3Backend` 内部用与 libDF 同实例的 `ndarray_015`（package rename）构造 `ArrayView2 [1,480]` 喂 `DfTract::process`，asr 的 0.17 类型完全不触及。

  **DF3 依赖（git，非 crates.io）**：`df = { git = "https://github.com/Rikorose/DeepFilterNet.git", tag = "v0.5.6", package = "deep_filter", features = ["tract", "default-model", "transforms"] }`（libDF 不在 crates.io，只能 git）。tag v0.5.6 对应 commit `978576aa`，tract `^0.19.4`（解析到 0.19.16，**不可用 0.21.x**——0.21.4 在 native 有 codegen bug 致权重 NaN，连官方 `deep-filter` bin 也崩）。

  **Send/Sync**：`DfTract` 含 `Arc<dyn RealToComplex<f32>>`（无 `+ Send`）→ `!Send`，故 `Df3Backend` 经 `unsafe impl Send/Sync`（照 VST3 plugin/src/lib.rs:9-11）。安全性：`DenoiseProcessor` 在 `Mutex` 内、coordinator 单线程串行 lock+process（audio.rs:94 注释），实际无跨线程并发，unsafe 仅满足类型约束不引入数据竞争。`RnnoiseBackend`（`Box<DenoiseState<'static>>`）天然 Send，无需 unsafe。

  **状态保持与降级**：GRU 隐状态 + 特征缓冲 **跨 `drain_samples` 周期、跨 VAD 分段连续保持**（噪声估计是连续物理过程，与 `filter_vad` 每段 reset 故意相反）；新会话 `start()` 调 `reset()`（DF3 reset = 重载 7.9MB 模型，仅会话边界可接受）。链路 `process_pipeline`：原生SR→(`down_sampler`)→48k→DenoiseProcessor→(`resampler`)→16k（`flush` 语义同重采样器：`drain_samples` 不 flush 保连续、`stop` flush 取尾）。**三级降级**：`mode=0`→直通；后端加载/单帧推理失败→warn + backend 置 `None`→直通；**不 panic**、不阻断录音。无外部模型文件依赖（RNNoise 内置模型 / DF3 编译期内嵌），不进 DB、不参与引擎选择。

  **DF3 加载日志**：tract 加载 DF3 模型时刷大量 DEBUG（`tract_core::optim` 的 `applying patch`、`tract_hir::infer` 的 shape 推断），`crates/desktop/src/main.rs` 的 `tauri_plugin_log::Builder` 对 `tract_core`/`tract_hir`/`tract_onnx`/`tract_linalg` 四子模块 `level_for(Warn)` 压制；**保留** `df::tract` 自身 `Info`（`Loading model ...` / `Init encoder` / `Running with model type ...`）作加载进度信号。RNNoise 无 tract 依赖，不受影响。

  **历史**：第一版曾用第三方 `dfn3.onnx` + ort（模型缺陷压语音至 ~10%，已弃用换 RNNoise，见 [`2026-06-16-denoise-deepfilternet-design.md`](superpowers/specs/2026-06-16-archived-design.md)）；本版改用官方原生 libDF + tract（spike 验证 gain=0.958 不压语音），DF3 与 RNNoise 并存。详见 [spec](superpowers/specs/2026-06-17-archived-design.md)
- **VAD 分段切分策略**（`VadSegmentedPipeline::run_tick`，pipeline.rs，2c-3 收编自原 coordinator）：静音边界切分（主）+ 连续超时强制切断（兜底）
  - 静音切分：检测到语音后静音 ≥ `segment_silence`（默认 400ms）→ 切分，**无 overlap**（静音是自然语句边界，下一段从干净开始）
  - 强制切断：连续语音缓冲达 `SEGMENT_DURATION_S`（20s 常量）仍未静音 → 强制切断，**保留末尾 200ms（常量 `SEGMENT_OVERLAP_MS`）作下一段 overlap**（语句被硬切，需重叠保连贯）。`segment_duration` / `segment_overlap` 原为 config 字段，因属实现细节（用户不可感知）已改为常量
  - **双 VAD 实例（检测流 vs 过滤，修 LSTM 状态污染）**：SileroVad 是有状态 LSTM（`compute()` 更新 `h`/`c`，`reset()` 归零）。`VadSegmented` stage 持**两个独立实例**：① 检测用 `vad`——逐 tick 喂入顺序音频、跨 tick 有状态累积（续接上下文使语音/静音边界判定更稳），喂 `compute_speech_chunks`；② 过滤用 `filter_vad`——仅 `filter_speech_from_buffer` 用，**每次过滤前 `reset()` 归零**，恢复「每段独立冷启动」语义（等价旧代码每 buffer 新建 VAD，但 ONNX Session 全局缓存（启动 preheat 加载、同 path 复用，`SileroVad::new` 仅 clone Arc + zeros h/c），过滤只 reset 不重建，兼顾正确性与性能）。分离原因：检测流已按顺序见过 `samples`，而 `send_buffer`（`overlap_tail` + `audio_buffer`）与之重叠，若共用一个有状态 VAD 会双重喂入 + 跨段污染 LSTM → 段首 gating 失真（裁掉语音起音或混入前导噪声）
  - **`filter_speech` 两端 trim（修首尾字丢失）**：检测流切出的单段经 `filter_speech_from_buffer` → `octopus_asr_local::audio::filter_speech` 过滤，**只 trim 首尾静音、保留中间全部音频**（含句内 ~50ms 停顿 / 轻声帧），**不逐帧删除**低于阈值帧——逐帧删会破坏句子连续时间结构 → 声学特征错乱 → 漏字 / 乱码 / 粘连。扫描首个 / 末个高于阈值的帧，各外扩 `SPEECH_PAD_MS`（120ms，@480 样本/30ms 帧 = 4 帧）作为起止点，补回 VAD 响应延迟切掉的首字音头、与衰减残尾被判静音的尾字尾音（参考 silero-vad `speech_pad_ms` 默认 30ms）；该 padding 远低于段间静音阈值（仅借回纯静音、不触及相邻段语音）。`transcribe_with_vad` 的 `segment_audio_vad`（>30s 长音频走此路径）共用同一 `SPEECH_PAD_MS`，段首预借 / 段尾后补同模式
  - 每段经 `filter_speech_from_buffer` 过滤静音后，由 `VadSegmentedPipeline` 内部 `spawn_offline_transcription_with_seq` 派发到 **Tauri 全局异步运行时**（`tauri::async_runtime::spawn`）执行 `engine.transcribe`（底层 CPU 密集推理已 `spawn_blocking` 包裹、不阻塞 runtime worker），完成经 **mpsc rx** 回填 `completed_results: HashMap<seq,String>` + `completed_seq` 游标连续消费（**2c-3 删 `Command::TranscriptionDone`**，改 pipeline 内部 mpsc，coordinator 不再参与段完成回调）；段间不做 overlap 去重——force_cut 段虽带 200ms overlap_tail，但仅 ≈1 字、与正常重字不可区分，曾因子串匹配误删真词（如「识别」），已移除去重逻辑改为逗号直接拼接。**识别失败 / 空结果仍占位该 `seq`（写空串）以保证游标连续推进**——否则缺失序号会让消费卡死、该次录音此后所有有效段积压丢失；**跨会话保护（2c-3）**：pipeline drop → mpsc rx disconnect，旧会话迟到段不污染新会话（原 `TranscriptionDone` 携带 `session_id` 比对 `transcript.id` 的机制随命令删除，改由 pipeline 生命周期兜底——快速双击 Toggle / 录音中重启时旧 pipeline drop 即切断其 rx，残留异步转写回调无处回填）
- **Transcript 文本状态机**：识别文本状态由 `Transcript` 结构（`crates/desktop/src/transcript.rs`）统一管理——内部用 `full`（当前完整 ASR）+ `raw_len`（上次停顿快照的 char 长度）派生 `raw`（停顿快照，润色基准）/ `increase`（停顿后增量），避免维护三份字符串。`Stage::Streaming` / `VadSegmented` / `WaitingCompletion` 各持 `transcript: Transcript` 字段，文本流经 Transcript 方法（`set_full` / `append_segment` / `display_text` / `db_text`）。停止后 `Stage::Polishing`（最终润色中，持 `id` + `raw_text`）→ `Stage::Pasting`（持 `id` + `raw_text` + `polished_text` + `polish_status`）。入库的 `engine` / `engine_mode` 在过程入库的 raw 阶段已写（`update_transcription_raw(&config.asr_engine, ..)`），`Pasting` 不再持有。详见 [spec](superpowers/specs/2026-06-14-archived-design.md)
- **停顿驱动润色**：流式 / 伪流式统一——静音 ≥ `pause_polish_threshold_ms`（默认 600ms，可配置）/ 伪流式段边界完成时，经 `take_polish_input()` 取润色输入（无编辑 = 全量 ASR `raw + increase`；已编辑 = `(edited, 新增)` 边界，见「结果窗可编辑」）送 LLM 润色（mode=2 only），**不重置流式引擎**（只读送 LLM，引擎状态原样保留）。修复了流式中间润色 P0（partial 全量覆盖 polished）。默认 600ms > Active Flush 500ms（GUI 约束 `>= 600`，须大于句间停顿最大值，否则润色先于尾音冲刷、快照缺尾音），润色在 tick 流程最末执行，快照可靠
- **立即润色（PolishNow）**：工具栏「立即润色」按钮（`tool-polish-now`）点击 → `invoke('polish_now')` → `Command::PolishNow` → `handle_polish_now`：**忽略 `polish_mode`**（不受 mode=0/1/2 限制，区别于停顿润色的 mode=2 限制），经 `llm_config_ignore_mode()` 取 LLM 配置，复用 `take_polish_input` → `spawn_polish_thread(ignore_mode=true)` → `Command::PolishDone` 路径。`spawn_polish_thread` 新增 `ignore_mode` 参数控制是否绕过 mode 检查。`handle_polish_done` 接受 `Streaming`/`VadSegmented`/`WaitingCompletion`/`CloudClosing` 四阶段（防用户点按钮后停录音致 stage 切换、润色结果被丢弃；**cloud 流式走 `Stage::Streaming`、`CloudClosing` 同样是活跃会话，必须支持**，否则用户用云端引擎时点「立即润色」会被忽略），写回后 `emit("polish-done")` 通知前端恢复按钮（成功/失败/stage 不匹配均通知）。**`handle_polish_now` / `handle_enter_edit_mode` / `commit_edit_apply` 三个 transcript 操作函数都支持全部活跃会话 stage**（`Streaming`/`VadSegmented`/`WaitingCompletion`/`CloudClosing`），否则云端引擎路径下编辑/立即润色功能全部失效。**`handle_polish_now` 所有早退路径（stage 不匹配 / transcript 空 / 已 pending / LLM 配置缺失）都 emit `polish-done`**——否则前端 `btnPolishNow.disabled=true` 永久卡死。`Transcript::display_text()` 同步变更：**polished 非空即展示**（`polished + increase`），不再仅限 `mode==Intermediate`，使 PolishNow 在任意 mode 下都能让润色文本覆盖 raw 回显到展示区
- **结果窗可编辑（Transcript 三文本分层）**：
  - `Transcript` 三文本分层：`edited ≻ polished ≻ raw`。`display_text()` = committed + increase；`full`（原始 ASR）独立保留为 DB `raw_text`。
  - 编辑态：coordinator 主循环 `editing` 标志置位时，Streaming/VadSegmented tick 跳过喂引擎、只排空丢弃音频（硬暂停）。`commit_edit` 写回 transcript 并 `UPDATE edited_text`。
  - 编辑×润色（spec §12）：`take_polish_input()` 返回 `(preserved=edited, to_polish=increase)`，LLM 仅润色新增；`on_polish_done` 在 `has_edit()` 时折回 `edited`（避免遮蔽丢字）。**raw_len 推进延迟到 `on_polish_done`**（flicker 修复）：`take_polish_input` 只记录 `polish_snapshot_len` 不推进 `raw_len`，保证润色 pending 期间 `display_text()` 不丢 increase（展示区文字不变），润色完成后 raw_len 才推进 + polished 覆盖 → display 只变一次。
  - `transcriptions` 表加 `edited_text` 列（commit + 中间润色折回时写）。
  - 停止路径：润色输入 = `take_polish_input`；无润色/兜底粘贴 = `edited_display()`；最终润色失败兜底 = `Stage::Polishing.fallback_text`；DB raw 仍 = `db_text()`。
- VAD 标点：基于 SileroVad 静音检测，>0.5s 静音插入逗号。**段间拼接标点去重**：`consume_completed_results` 在段间补逗号前同时检查「新段不以标点开头」和「已有文本不以标点结尾」，避免 ASR 引擎返回的自带句尾标点与补的逗号连续出现（`。，` `？，`）
- 流式尾音冲刷（Active Flush）：流式模式累积静音 ≥0.5s 时把憋住的尾音即时吐出——Zipformer 用 edge-replicate lookahead padding（3 chunks，与 `finish` 共享 `run_padding_flush`）对齐右上下文，Paraformer 用 CIF force-fire（`run_cif_final`）；**同时追加逗号**（`flush(insert_comma=true)`），提供即时分句反馈——此前逗号只在下一句话到来时插入，停顿期间无标点。每个静音段仅触发一次（`flushed` 标志，恢复说话时重置）。详见 [spec](superpowers/specs/2026-06-14-archived-design.md)
- **Paraformer 流式尾部 CIF force-fire**：CIF 机制 alpha 累积达阈值 1.0 才 fire 产出 token，`finish()` 时残留 alpha >0 但 <1.0 的声学特征卡在 `encoder_out_cache` 不触发 → 最后一个字被吞。`run_cif_final()` 在 CIF 循环结束后检查残留，alpha >0.5 则 force-fire 为最后一个 token 送 decoder（<0.5 视为噪声不 fire）；sherpa-onnx 官方也丢弃此残留（已知 trade-off），我们做了改善
- **Paraformer 流式 3 个严重 bug 修复**（sherpa-onnx 源码对照）：①离线 CMVN 重复 `* scale`——`extract_cmvn_from_metadata` 已在 inv_stddev 乘 sqrt(512)，`transcribe` 又乘一次 → 特征放大 22.6 倍，移除重复；②流式位置编码缺负号——`k_scale` 应为 `-ln(10000)/(half_dim-1)`，缺负号导致高维频率爆炸、随音频变长退化；③`process_chunk_final` 仍 mask 右侧 alpha——尾部 3 帧无下个 chunk 处理，mask 掉永久丢失 ~180ms 语音，新增 `mask_alphas_left_only`。另：`flush()` 最后一个 chunk 也走 force-fire（`run_cif_final`）；`accept_samples` 恢复逗号但加 `ends_with_punct` 防重复标点
- **Paraformer fbank 特征提取修复（5 个根因）**（详见 [spec](superpowers/specs/2026-06-21-paraformer-fbank-feature-extraction-fix.md)）：流式识别质量严重退化（token 重复 `thedayday`/`tomtomor`、英文粘连无空格）的根因全部在 fbank 层。①**缺 DC offset removal**——每帧 FFT 前未减帧均值（sherpa-onnx 默认 `remove_dc_offset=true`）；②**缺 pre-emphasis**——未做 `y[i]=x[i]-0.97*x[i-1]` 预加重（sherpa-onnx 默认 `preemph_coeff=0.97`）；③**窗口函数错误**——流式应用 povey 窗 `(0.5-0.5cos)^0.85` 而非 hamming 窗；④**mel 滤波器 high_freq 错误**——应用 7600 Hz（`high_freq=-400`）而非 8000 Hz；⑤**流式架构缺陷**——重叠 chunk 重复提取 fbank 致帧边界断裂，重写为**增量式 fbank**（`raw_samples` 线性追加 + `fbank_cache` 增量计算，对齐 sherpa-onnx `OnlineFbank`）。另：`decode_tokens` 重写为 sherpa-onnx `Convert()` 兼容的空格逻辑（英文词间加空格、`@@` BPE 合并）；新增 `smart_append()` 在 chunk 边界拼接时检测 ASCII↔非 ASCII 过渡插入空格
- **Fbank 特征提取参数化**（`paraformer.rs::compute_fbank`）：统一签名 `compute_fbank(samples, window, preemph_coeff)`——流式 Paraformer 用 povey 窗，离线 Paraformer 用 hamming 窗。**Pre-emphasis 无跨帧状态**：帧重叠（shift=160 < len=400）时上一帧末尾 ≠ 本帧 start-1，故直接从连续缓冲回溯 `samples[start-1]`（减本帧 mean 近似去直流），消除 `preemph_prev` 状态字段。离线 `ParaformerEngine` 与流式 `StreamingParaformer` 共享同一 `compute_fbank` 但窗口参数不同。Mel 滤波器（`MEL_FILTERBANK` static）high_freq=7600 Hz，mel 空间三角权重
- **Paraformer BPE 跨 chunk 整体解码**：模型输出的是 BPE 子词 token（如 `val@@` + `ue`），非完整单词。`StreamingParaformer` 累积 `all_token_ids: Vec<i64>` 跨所有 chunk，`accept_samples`/`flush` 整体 `decode_tokens(all_token_ids)` 返回完整 ASR 文本——避免 chunk 边界各自解码导致 BPE 续接断裂（`val`/`ue` 分开）。`StreamingSession` Paraformer 路径用 `punct_prefix`（已提交 ASR + 逗号）+ `committed_chars`（已提交字符数）管理逗号：静音点冻结快照 + 插逗号，新 delta = `full_asr.skip(committed_chars)` 拼在逗号后
- **Paraformer 流式热路径性能优化**（零拷贝，每 chunk 节省 ~420KB 堆分配 + 消除 FFT 重复规划）：① decoder_caches 更新用 `copy_from_slice` 复用预分配 Array3（省 16×320KB）；② encoder 输入用 `into_shape` 零拷贝 reshape（省 45KB clone）；③ `run_cif`/`run_cif_final` 用 `as_slice().unwrap()` 直接拿 `&[f32]`（省 20-40KB to_vec）；④ decoder input 键名 `in_cache_0..15` 预分配为 `cache_keys: Vec<String>`（省 16× format!）；⑤ **FFT 规划提升为全局静态** `FBANK_FFT: Lazy<Arc<dyn rustfft::Fft<f32>>>`（`paraformer.rs`，与 `POVEY_WINDOW` 同位置），`compute_fbank` 与 `StreamingParaformer::compute_new_fbank_frames` 共用——消除每 chunk `FftPlanner::new()` + `plan_fft_forward(512)` 的堆分配 + twiddle 规划计算；⑥ `apply_feat_overlap` 用 `ArrayView2::from_shape` 包装 `&self.feat_cache`（省 8×560×4B ≈ 17.5KB clone）；⑦ `run_decoder` 用 `ArrayView3::from_shape` 包装 `acoustic: &[f32]`（省 `to_vec()` 拷贝，与 `qwen3_asr.rs` 的 `ArrayView3+TensorRef` 模式一致）；⑧ `run_decoder` 的 `enc_len` / `acoustic_len` 单元素张量用栈数组 `[x]` + `ArrayView1::from(&[x])` 替代 `Array1::from_vec(vec![x])`（省 2 次微小堆分配）；⑨ `reset()` 的 decoder_caches 清零按形状分治：形状仍为初始 `(1, encoder_output_size, cache_time)` 时 `fill(0.0)` 复用内存，被 run_decoder 慢路径改过维度时才重分配恢复初始形状；⑩ 离线 `transcribe`（`paraformer.rs`）CIF 循环改用 `enc_tensor.slice(s![0, ..enc_len_scalar, ..]).as_slice().unwrap()` 直接借用，消除 `enc_tensor.clone().into_raw_vec_and_offset()` 的整段 encoder 输出拷贝，`enc_tensor` 保留供 decoder `view()` 使用；附带将离线 transcribe 的 `speech_lengths` / `acoustic_len` / `enc_len_for_dec` 单元素张量统一为栈数组 + `ArrayView1`
- **mask_alphas 越界防护**（`streaming_paraformer.rs`）：`mask_alphas` / `mask_alphas_left_only` 取 `n = alphas.len().min(enc_len)` 再循环，消除 ONNX 返回异常尺寸时 `alphas[i]` panic 风险
- **Paraformer 边界鲁棒性防御**：① `smart_append`（`paraformer.rs`）边界空格判定追加 `last_byte != 0x20 && first_byte != 0x20`，避免 `existing` 末尾或 `new` 首字符已是空格时再 push 空格导致双空格（空格 `0x20` 本身满足 `< 0x80` ascii 判定）；② `run_cif` / `run_cif_final`（`streaming_paraformer.rs`）的 encoder slice 改为 `..enc_len.min(enc_tensor.shape()[1])`，防御 ONNX `enc_len_data[0]` 与实际张量维度不一致（padding/截断异常）时的 slice panic，与 `mask_alphas` 同模式
- **Paraformer accept_samples 清 input_finished（会话内状态污染修复）**：`input_finished` 标记在 `flush()` 静音冲刷时置 `true`，让 `compute_new_fbank_frames` 末帧越界零 padding 多算帧、配合 CIF force-fire 吐尾音；仅 `reset()`（会话边界：录音停止 / 取消）清除。**Paraformer 流式会话内不 reset**（累积上下文跨 chunk），故用户停顿冲刷尾音后继续说话时 `accept_samples` 若不清，`input_finished` 持续 `true` → 每次多算越界零 padding 帧 → 特征错乱 → **首次停顿后会话级乱码 / 丢字 / 大量重复字**（本「首字 / 尾字」专项最严重的一个）。修复：`accept_samples` 入口置 `false`（语义 = 继续说话，回正常帧计算模式）。详见 [spec](superpowers/specs/2026-06-21-paraformer-fbank-feature-extraction-fix.md) §10
- **流式 partial 渲染单调性（防闪烁）**：`StreamingZipformer::process_chunks` 三个返回点（sample_buffer 空 / 样本不足凑 chunk / 末尾）统一经 `decoded_current()` 返回当前段文本——避免「样本不足凑 chunk 时早退返回 None、`StreamingSession` 丢 current_segment 只回 accumulated」导致长短态逐帧交替闪烁（coordinator 每 tick drain ~3200 样本，凑不够 chunk 时走早退）。`StreamingPipeline::tick` 承载层加幂等门（`text != transcript.full()` 才 set_full，changed 才产 PersistRaw+Emit，2d），消除静音期 flush 同文本反复重绘。前端 `update-result` listener 单调渲染：新文本是已显示内容的前缀（`startsWith`）则立即渲染并清待处理跳变；跳变 / 段切换延迟合并（`DIVERTED_DELAY_MS=300`）只渲染最新，连续跳变不闪烁。
- **设置窗口子系统（settings_commands + settings_window）**：独立 Tauri 窗口 `settings_window`（`settings_window.rs`），原生标题栏、800×600 可调。8 个 Tauri 命令：`open_settings`（单例窗口管理——`get_webview_window` → `set_focus`，否则 `WebviewWindowBuilder` 新建；macOS 打开时切 `Regular` 激活策略显示 Dock 图标 + `setApplicationIconImage`）、`get_config`（返回 `ConfigResponse`：AppConfig JSON + ASR 引擎列表 + LLM 模型列表 + 麦克风设备列表（字母排序，保证每次打开顺序恒定））、`set_config(key, value)`（通用字段写入器，`apply_config_value` 做 18 字段类型/范围校验；**快捷键先注册后持久化（2026-06-21 审查修复）**：`asr_shortcut` 字段先 `unregister` 旧的 + `register_shortcut` 新的，注册成功才写共享 `AppConfig` + `save_app_config`，失败则尝试恢复旧快捷键并返回 Err（避免无效快捷键持久化到 DB 致下次启动依然失败）；`edit_shortcut` / `hide_toolbar` 改动发 `config-changed` 事件让结果窗 `refreshActive` 刷新）、`get_history(limit, offset)`（分页查询 `transcriptions` 表，返回 `Vec<TranscriptionRecord>`）、`delete_history(ids)`（批量删除，`IN` 子句，返回删除行数）、`check_shortcut(shortcut)`（快捷键冲突检测——`on_shortcut` 注册 → 立即 `unregister`，仅检测不持久化）、`test_llm_connection(spec)`（润色模型连通性检测——从 DB 按 spec 加载 `CompatibleLlmConfig` → `async fn` + `spawn_blocking` 跑 `octopus_llm::test_connection`：发 `max_tokens=1` 的极简 chat 请求，10s 超时，成功返回「连接成功」/失败返回 HTTP 错误码 + body）、`test_asr_connection(bare_name)`（远程 ASR 引擎连通性检测——本地模型（`is_local=true`）直接返回 Err「本地模型无需连接测试」；远程模型（provider=aliyun）从 DB 取 endpoint + secret_key，`async fn` 直接 `await connect_async`（删 `Runtime::new`）→ WS 握手 + `Authorization: bearer <key>` → `tokio::time::timeout(3s, connect_async)`，仅验证握手成功不发协议帧）。前端 `dist/settings/index.html` 纯 vanilla HTML，无构建步骤。`polish_mode` 序列化为 `u8`（0/1/2），前端 select 用数字 value。**识别记录页**：倒序排列，润色 text 优先显示（黑色主文本）、原始 text 折叠（灰色次要），工具栏含全选 checkbox + 批量删除（两次点击确认，Tauri webview 不支持原生 `confirm()`，任何勾选变化/超时自动取消确认态），每条记录右侧拷贝按钮（内联 `copy.svg` 图标）。**系统设置页**：6 张卡片（交互置顶，全部无标题）；生效时间标签内联到 label 文字后面（灰色小字括号如「(立即)」）；快捷键改为键盘捕获按钮（全局 `asr_shortcut` → `check_shortcut` 冲突检测 → 热重载；窗口内 `edit_shortcut` 无需冲突检测 → 发 `config-changed` 刷新结果窗）；润色间隔/润色停顿阈值改为下拉选择（`pause_polish_threshold_ms` 约束 `>= 600`）；语言仅 auto/zh/en。**连接测试按钮（check.svg 图标）**：ASR 引擎 select 右侧 + 润色模型 select 右侧各一个 32×32 圆角按钮——三态视觉（默认灰 / 成功绿 #22c55e / 失败红 #ef4444），点击 `loading` 半透明 + 禁用，回调后切 ok/fail；ASR 按钮按当前选中引擎 `is_local` 切 `disabled`（本地灰掉 `pointer-events:none`），切换 select 时 `updateAsrTestBtn` 动态刷新；LLM 按钮始终可点，点击前先 `set_config('polish_llm', value)` 持久化再调 `test_llm_connection`（确保后端从 DB 取到最新 spec）。详见 [spec](superpowers/specs/2026-06-19-archived-design.md)（§ connection-test-design，已归档）。
- **运行时配置子系统（SharedRuntimeConfig）**：工具栏可运行时切换 `asr_engine` / `polish_mode` / `polish_llm` / `denoise_mode`，无需重启。`runtime_config.rs` 提供 `SharedRuntimeConfig`（`type = Arc<RwLock<AppConfig>>`，挂 `tauri::State`）——**完整 `AppConfig` 的唯一真相源**，取代旧 `RuntimeConfig` 部分镜像（消除字段同步遗漏，新增运行时生效字段零同步代码）。8 个 Tauri 命令（`toolbar_state` / `list_asr_engines` / `switch_asr_engine` / `set_polish_mode` / `list_llm_models` / `switch_polish_llm` / `set_denoise_mode` / `polish_now`）读写共享 `AppConfig`（即时生效）+ `persist_*` best-effort 持久化回 `~/.octopus/app_config 表`（写盘失败仅 `warn`，本次仍生效、重启回退；`polish_now` 不写盘，只触发润色流程）。**`switch_asr_engine` / `switch_polish_llm` 前端传裸 `model_name`，后端查 DB 取 `provider` / `category` 构造 3-part spec（`"{provider}:{category}:{model_name}"`）写入共享 `AppConfig` + app_config 表**——保证持久化值与 `parse_model_spec` 解析一致。`list_*` 的 current 判定经 `parse_model_spec(current).model_name()` 提取裸名比较，兼容 3-part 和裸名两种历史格式。`switch_asr_engine` 同时经 `tray::update_tray_engine_label` 实时刷新系统托盘菜单的「引擎: <model_name> (<mode>)」项（`TRAY_ITEMS` 缓存 `engine_info` MenuItem handle，`set_text` 更新而非重建，规避 `MenuItem::with_id` 重复 ID panic）。Coordinator 闭包持共享 `AppConfig` 句柄，**在 Toggle 进入 `Idle` 时重读 `asr_engine` / `polish_mode` / `polish_llm` 并经 `resolve_active_engine` 校验有效性——保留完整 3-part spec（`rc.asr_engine.clone()`）写回 `config.asr_engine`，失效则兜底 `local:zipformer:zipformer-small-ctc`**，保证 `is_streaming_engine` 判定 / `use_streaming` 重算 / `StreamingSession::new` / 离线 `transcribe` / transcriptions.engine 记录全用完整有效 spec；`main.rs` 启动 preheat 同样解析（preheat 与实际工作模型一致）。**外部修改共享 `AppConfig` 后立即同步到 coordinator（2026-06-18 改进）**：`set_config`（设置窗口）和 `switch_polish_llm`（工具栏浮层）写完共享 `AppConfig` 后调 `coordinator.update_runtime()` → `Command::UpdateRuntime` → `sync_runtime_fields` 把 `polish_llm` / `polish_mode` / `asr_correct` / `output_simplified` / `hide_toolbar` 同步到 config 快照，**无需 Toggle 即可生效**（用户在录音中改 polish_llm 下次润色就用新模型）。`asr_engine` 不走此路径（需重建引擎实例）。`polish_mode` 仍保留每 tick 读 `set_mode`（双保险立即生效）。详见 [spec](superpowers/specs/2026-06-16-archived-design.md)

**文本持久化（嵌入式 SQLite）：**
- 存储：`~/.octopus/octopus.db`（`crates/infra/src/db.rs`，全局 `OnceLock<Mutex<Connection>>`；asr crate 经 `pub use octopus_infra::db` 以 `crate::db` 暴露；cli/server/desktop 共用）
- `transcriptions` 表：识别历史，每条存原生识别全文（`raw_text`）+ 润色版（`polished_text`）+ 润色状态（`polish_status`：`off`/`done`/`failed`）+ 元数据（engine / engine_mode / created_at / char_count / duration_ms）
- **过程增量入库（schema v3）**：`transcriptions.id` = 识别开始毫秒时间戳（`INTEGER PRIMARY KEY`，应用写入，去 `AUTOINCREMENT`），兼任主键 / 业务 key / 开始时间戳；`duration_ms = finalize_now_ms - id`。入库时机分散到识别过程各事件：首次有 ASR 文本 → `INSERT`（`insert_transcription_at_id`）；分段 / 流式 partial → `UPDATE raw_text`（`update_raw_text`）；停顿润色完成 → `UPDATE polished_text`（`update_polished`）；停止 → `finalize`（含 `duration_ms`，`finalize_transcription`）。DB 失败仅 `warn` log 不阻塞识别（best-effort）。v2→v3 migration DROP 重建（旧数据无所谓）。详见 [spec](superpowers/specs/2026-06-14-archived-design.md)
- **非阻塞 DB 写入（actor 模式）**：上述 `INSERT`/`UPDATE`/`finalize` 不在协调器线程同步执行——`update_transcription_raw` / `PasteDone` 等调用方仅 `get_db_sender().send(DbCommand)` 入队后立即返回，真实落库由**后台 DB 写线程**（`static DB_SENDER: OnceLock<Sender<DbCommand>>` 懒加载 spawn）单线程消费。mpsc 的 FIFO 保证同 id 的 `Insert` 必在 `UpdateRaw` 之前被消费（故 `mark_db_inserted()` 在 send 后即置位仍安全——真实顺序由 channel 保，不由标志位保）。识别主循环不再被 SQLite I/O 阻塞。
- **关机优雅 drain**：后台写线程 `&'static Sender` 永不 drop，进程 kill 时队列里未处理命令会丢失（典型路径：录音结束 → `Finalize` 入队 → 用户立即退出 → 该条记录停留未 finalize 态）。`coordinator::shutdown_db()` 置 `DB_SHUTDOWN`（AtomicBool）→ 后台线程排空 `try_iter()` 剩余命令后退出，主线程 `JoinHandle::join` 等待落库完成；`main.rs` 挂到 `tauri::RunEvent::ExitRequested`（macOS Cmd+Q / 关闭最后一个窗口触发），保证退出前队列清空。
- `models` 表：模型目录（**唯一来源**，schema 见 `crates/infra/src/db.sql`，首次建库 `user_version=0` 时整体执行一次 seed；本地 ASR（is_local=1，12 行）初始**全 `is_enabled=0`**（待下载就绪），默认兜底引擎 `zipformer-small-ctc` 代码写死（`FALLBACK_ASR_ENGINE_NAME`）不占 seed 行，`app_config.asr_engine` 空/不匹配时 `fallback_engine` 硬构造（见下「全局默认引擎」）；列 `domain` / `provider` / `category` / `model_name` / `source` / `secret_key` / `language` / `is_local` / `is_thinking` / `is_streaming` / `is_enabled` / `description`，唯一键 `UNIQUE(domain, provider, category, model_name)`；`load_models_at` 仅读 `domain='asr' AND is_enabled=1`，`domain='llm'` 经 `load_llm_model(spec)` 按 `{provider}:{category}:{model_name}` 3-part spec 读；引擎激活由 `app_config.asr_engine` 决定，无 `is_active` 列，见「模型管理」）
- **`app_config` 表（v3+，替代旧 `config.yaml`）**：应用行为配置的统一存储（22 字段 key-value TEXT，含 `category` 分组列默认 `'default'` + `description` 描述列），由 `db.sql` seed 默认值 + `load_app_config()` 按字段类型解析。写入用 `ON CONFLICT DO UPDATE SET config_value`（仅改值，保留 description + category）。旧 `config.yaml` 首次启动时一次性导入 DB 后重命名为 `.bak`（迁移逻辑在 `init_schema` 中）。新增 `active_polish_prompt` key（存 prompts 表 id 字符串，默认 `'1'`）。
- **`prompts` 表（v4+，润色提示词管理）**：多 prompt 管理（替代旧单文件 `VOICE_POLISH.md`）。列：`id`（PK AUTOINCREMENT，用户不可编辑）/ `title`（用户可读别名，允许重复）/ `category`（固定 `voice_text_polish`）/ `content`（风格规则，不含增量逻辑）/ `description` / `is_system` / 时间戳。seed `id=1` 系统默认（不可编辑/删除）。`app_config.active_polish_prompt` 存激活 id。`llm::prompt::build_system_prompt(content) = content + INCREMENTAL_RULE`（第 7 条增量规则代码常量强制拼接，用户不可见）。启动时 `main.rs` 从 DB 读 active prompt → `set_system_prompt`；设置窗口 6 个 Tauri 命令（`list_prompts` / `get_active_prompt` / `set_active_prompt` / `create_prompt` / `update_prompt` / `delete_prompt`），切换即时生效（`set_system_prompt` 写 `RwLock<String>`，下次润色用新 prompt）。
- `model.json` / `history.txt` / `record.txt` 已从代码彻底删除——DB 是唯一配置/存储源
- `polish_status` 基于润色调用结果：未启用→`off`；启用且返回非空→`done`；启用但返回空或失败→`failed`
- 润色三档（`polish_mode`：0 关闭 / 1 仅最终 / 2 中间+最终）：中间润色由 `check_and_trigger_polish` 在停顿点触发（流式静音 ≥ `pause_polish_threshold_ms`（默认 600ms）/ 伪流式段边界），把 `Transcript.take_polish_input()`（完整 ASR；已编辑时分块 `edited + 新增`）送 LLM 润色，节流 `polish_interval`（下限 `MIN_POLISH_INTERVAL_SEC=1.0s`）；最终润色在 `start_final_polish_or_paste`（停止后）：启用润色→`Stage::Polishing` 异步线程跑 LLM，回调 `Command::FinalPolishDone` 后 `do_paste`；未启用→直接 `do_paste`。详见 [设计](superpowers/specs/2026-06-14-archived-design.md)。
- 停止空文本边界：Toggle 停止录音时若 `transcript.full()` 为空（麦克风静音 / VAD 未检出语音），`start_final_polish_or_paste` 空文本分支直接回 `Idle`，必须对称清理 `result_window::hide_result` + `tray → Idle` 两类 UI 反馈（缺一则"正在聆听…"框残留）；详见 [设计 §4.5](superpowers/specs/2026-06-14-archived-design.md)。**云端 WSS 连接失败（2026-06-21 审查修复）**：`CloudPipelineEngine::tick` 中 `cloud_pipeline::open_cloud_session` 返回 `Err` 时，除 `error!` 日志 + 复位 `is_speaking=false` 外，**产 `TranscriptEvent::Error("⚠️ 云端连接失败：<msg>")`**（承载层 last_error → 下 tick `PipelineEvent::Error` → `apply_pipeline_events` → `update_result`，2d 删原 `take_error`），让用户即时感知错误而非卡在"正在聆听…"假死状态。session 由 `!is_closing && !is_speaking` 分支自动 take，下次语音 onset 重开 WS（瞬时抖动自动重试；持续失败如 Key 无效每次 onset 报错，用户可见可排查）

支持三种引擎接入模式：
- **embedded**（默认）：内嵌 octopus-asr-local，本地推理
- **remote-ws**：通过 WebSocket 连接远程 octopus-server
- **remote-grpc**：通过 gRPC 连接远程推理服务
- **云引擎（cloud feature）**：`app_config.asr_engine` 解析为 `provider='aliyun'`（`EngineCategory::Aliyun`）时，路由 `AliyunEngine`（desktop crate，`cloud` feature 后）；`provider='bytedance'`（`EngineCategory::ByteDance`）时直接走 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支，无独立 engine）。均不走 `engine_mode` 分支。详见下方「云端 ASR 引擎」
- **远程超时保护**：`WsRemoteEngine` / `GrpcRemoteEngine` / `AliyunEngine` 的 `transcribe` 均以 `tokio::time::timeout(8s)` 包裹（连接 + 收发全程），`health_check` 同样 `timeout(3s)`——规避网络断开 / 后端无响应致 ASR 队列无限期卡死。超时返回 `Err`，经序列空洞修复的空串占位分支保证 `completed_seq` 连续推进、不拖死后续分段

### octopus-download（通用下载器）

通用文件下载 crate（分块并发 + 断点续传 sidecar + SHA256 校验 + 镜像 fallback），解终端用户下载大模型的三痛点：需装 Python + huggingface-cli、国内需切镜像、无参数 hf-cli 拉整个仓库（实际只需 int8 量化文件）。两模块：

- **`core`（通用，零 HF 知识）**：`Downloader` 走 probe（GET Range bytes=0-0 取 total/accept-ranges/etag）→ 规划分段 → 并发 Range+seek-write → 进度聚合（mpsc）→ 校验 → 原子 rename。`DownloadTask { url, mirrors, dest, expected_hash }`。sidecar `<dest>.part.resume.json` 记录各段进度（`url_hash` 基于 dest、镜像无关，故镜像源可复用进度），支持崩溃续传；最终整文件 SHA256 校验兜底（不注入 If-Range，避免不支持它的镜像回退 200 全文重传）。
- **`hf`（HuggingFace 适配层）**：`fetch_siblings`（GET /api/models/{repo} 解析 rfilename/etag/lfs.oid）、`should_download`（手写 fnmatch，`*` 跨 `/`，对齐 hf-cli，已 Python golden 验证）、`resolve_tasks`（构造每文件 DownloadTask：镜像 URL 在前 + 官方 fallback + LFS→Sha256(lfs.oid) / 非 LFS→Etag）。

下载到 `target_dir/{repo}/{path}`（由调用方决定）。**已接入模型管理（阶段1）**：cli `download` 子命令薄封装此 crate（构 `HfRequest` → `resolve_tasks` 解析 siblings + glob 过滤 → 逐文件 `Downloader::download`，进度经 mpsc 推送打印），`target_dir = ~/.octopus/models`，落 `~/.octopus/models/<repo>/<path>/`；`resolve_model_dir` 已加该路径为查找级（见下节）。镜像优先级 `--mirror` > config `download_mirror` > 官方源。详见 spec `superpowers/specs/2026-06-21-download-model-integration-design.md`。

## 模型管理

模型配置**唯一来源**是 `~/.octopus/octopus.db` 的 `models` 表。小模型（VAD + 默认 ASR）随应用打包到固定路径，开箱即用；大模型按需下载——`octopus-cli download <repo>`（命令行）或设置窗口「模型管理」页（GUI）下到 `~/.octopus/models/<repo>/`（阶段1 接 `octopus-download`），兼容旧 hf-cli 下到 `~/.cache/huggingface/hub/` 的模型。

**GUI 模型管理（设置窗口页面 3）**：`crates/desktop/src/model_commands.rs`（独立模块，与 `settings_commands.rs` 分离以降低与 setting-ui2 分支的合并冲突）4 个 Tauri 命令——
- `list_downloadable_models`：**v2 直读 DB** `list_all_local_asr_models`（`domain='asr' AND is_local=1`，**不过滤 is_enabled**——区别于 `load_models_at` 的引擎选择用），按 `is_enabled` 显示就绪/下载。
- `download_model(repo)`：**v2 先探查** `resolve_model_dir`——命中（文件已就绪，如 hf-cache 旧模型）则自举 sha256 清单写 `secret_key` + 置 `is_enabled=true`（**不重下**）；未命中才下载（复用 download crate：`HfRequest` + `resolve_tasks` + 逐文件 `Downloader::download`，mpsc 进度转事件 `download-progress`/`download-file`），完成后自举 + 置 true。完成 emit `download-done{already_ready}`。
- `verify_model(model_name, repo)`：**v2 新增**完整性复核——按 `secret_key` 清单逐文件 sha256 比对；空清单则自举；损坏/缺失置 `is_enabled=false` 并返回损坏清单。
- `set_download_mirror(value)`：专用命令（`set_config.apply_config_value` 无 `download_mirror` 分发，独立命令免改 `settings_commands.rs`）。

**is_enabled 语义 = 文件就绪（v2）**：`true`=文件完备可被引擎加载，`false`=未就绪/未下载。写 DB 后调 `asr::config::reload_models_config()` 刷新 AsrConfig 缓存（`RUNTIME_CONFIG` v2 改 `RwLock<Option<Arc<AsrConfig>>>`，对齐 `APP_CONFIG` 模式），让「系统设置」引擎下拉即时更新——未就绪的模型不进下拉。local 模型 `secret_key` 重载为「文件清单 + sha256」JSON（api 模型仍是 key，按 `is_local` 分支，不冲突）。前端 `dist/settings/models.js`（IIFE 隔离；卡片按 is_enabled 显示「✓ 已就绪（+重新校验）/ 下载」；`index.html` 仅两处局部改动——`#page-models` 容器 + `<script src="models.js">`）。manifest（文件清单 + sha256，map 格式存 secret_key）下沉 `asr::manifest`，desktop/cli 共用；**cli `octopus-cli sync-models`** 批量扫描就绪本地模型、自举写 secret_key + 同步 is_enabled（首次填充/批量复核）。spec `superpowers/specs/2026-06-21-model-management-gui-design.md` §9。

```
~/.octopus/
├── octopus.db          # 嵌入式 SQLite（models + transcriptions + app_config + prompts 表，唯一存储）
├── config.yaml.bak     # 旧 config.yaml 迁移后的备份（首次启动自动生成，可安全删除）
└── models/
    ├── silero_vad_v4.onnx   # VAD（1.8M，find_silero_vad 固定加载，随包）
    ├── zipformer/           # 默认 ASR（27M，随包）
    └── <HF repo>/           # ★ cli download 下的大模型（如 Systran/faster-whisper-large-v3/）

~/.cache/huggingface/hub/   # 旧 hf-cli 大模型缓存（兼容：resolve 第 4 级仍查此处）
```

**模型目录解析（`config::resolve_model_dir`）** —— source 字段四级查找（纯本地 IO，不联网 / 不下载）：
1. `~/.octopus/<source>`（随包小模型，如 `models/zipformer`）
2. 绝对路径（`source` 是绝对路径且存在）
3. `~/.octopus/models/<source>`（★ cli download 下的大模型，优先于旧 hf-cli 缓存）
4. `find_hf_cache`（`~/.cache/huggingface/hub/models--<repo>/snapshots/<hash>/`，兼容旧 hf-cli）

模型缺失时报错，提示运行 `octopus-cli download <source>`（不自动下载，保持 resolve 纯查找语义）。

**统一 DB 存储（v3+）：**
所有配置统一存储在 `~/.octopus/octopus.db`（SQLite），不再使用独立 config.yaml 文件：

| 表 | 用途 | 初始化方式 |
|----|------|------------|
| `models` | 引擎/LLM 模型配置 | db.sql seed |
| `transcriptions` | 识别历史 | 运行时写入 |
| `app_config` | 应用行为配置（22 字段） | db.sql seed + yaml 迁移 |

- **应用行为配置** `app_config` 表 → `infra::config::AppConfig`（`octopus_infra::config::load_config()` → `db::load_app_config()`，22 字段：麦克风/引擎选择/分段/润色/LLM/粘贴/硬件加速/ASR 纠错/降噪/简繁输出/工具栏显隐/降噪模式/下载镜像等；另有 `active_polish_prompt` 由 `db::load_active_prompt_id()` 独立读取，不入 AppConfig struct）。schema 统一定义在 infra，asr/desktop/cli 共享。值统一 TEXT 存储，由 `load_app_config` 按字段类型解析。
- **DB 模型目录** `models` 表 → `asr::config::AsrConfig`（`octopus_asr_local::config::load_config()`，首次 `db::ensure_db()` 自动建表 + seed，读后缓存到 `RwLock<Option<Arc<AsrConfig>>>`——v2 可刷新：模型管理页 `set_model_enabled`/`set_model_secret_key` 后调 `reload_models_config()` 从 DB 重读替换，引擎下拉即时更新；对齐 `APP_CONFIG` 模式）。
- **配置持久化**：`persist_*`（单键 `save_config_key`，ON CONFLICT 仅改 config_value）、`set_config`（全量 `save_app_config`，22 字段 ON CONFLICT），均写 DB。旧 `write_config_yaml` 已移除。
- **yaml 迁移**：首次启动（v0/v1 → v2）检测旧 `~/.octopus/config.yaml` → 解析导入 DB 覆盖 seed → 重命名为 `config.yaml.bak`。迁移逻辑在 `init_schema` 中一次性执行。
- **`write_to_clipboard`**（默认 `true`）：粘贴后是否把识别结果留在剪贴板，方便他处再粘贴；与 `paste_method`（`clipboard` / `direct` / `none`）构成三模式矩阵——`clipboard` 模式 true 时不恢复原剪贴板内容、false 时恢复（恢复前若 `read_text` 读出空——图片/富文本/文件读不出——则跳过写回，避免空文本覆盖用户的非文本剪贴板）；`direct` 模式 true 时 enigo 输入后末尾写剪贴板、false 时不碰剪贴板；`none` 模式忽略此配置（其唯一目的就是写剪贴板）。`false` 时三种粘贴行为等同重构前现状（不破坏现有用户习惯）。详见 [spec §6](superpowers/specs/2026-06-14-archived-design.md)。

**引擎选择（单一真相 = `app_config.asr_engine`）：**
- `models` 表无 `is_active` 列（开发期 schema 变更采用删库重初始化——见 `crates/infra/src/db.sql` 注释；`init_schema` 仅 `user_version < 2` 时执行建表 + seed + yaml 迁移，不做数据 migration）。
- **provider × category taxonomy**（`provider`=vendor/运行位置，与 `category`=引擎族/模型系列 正交）：

  | `provider` | ASR（`category`） | LLM（`category`） |
  |---|---|---|
  | `local` | `zipformer`/`whisper`/`sensevoice`/`paraformer`/`qwen3-asr` | —（暂无本地 LLM） |
  | `aliyun` | `Fun-ASR` / `Paraformer-Realtime`（run-task 协议，`/api-ws/v1/inference`）/ `Qwen-ASR`（OpenAI Realtime 协议，`/api-ws/v1/realtime`） | `qwen` / `deepseek`（经 DashScope 代管） |
  | `bytedance` | `Doubao-ASR`（豆包大模型 ASR 双向流式，`bigmodel_async`，二进制帧协议） | — |
  | `tencent` | `Tencent-ASR`（腾讯云实时语音识别，WebSocket HMAC-SHA1 签名鉴权） | — |
  | `baidu` | `Baidu-ASR`（百度智能云实时语音识别，WebSocket START 帧鉴权） | — |
  | `deepseek` | — | `deepseek`（直连） |
  | `bigmodel` | — | `glm`（智谱） |

- **模型选择 spec（`asr_engine` / `polish_llm` 统一 3-part 格式）**：配置字符串支持 `"{provider}:{category}:{model_name}"` 三段格式从 DB `models` 表唯一定位模型（见 [spec](superpowers/specs/2026-06-17-archived-design.md)）：
  - `"local:zipformer:zipformer-small-ctc"` → 4 字段精确匹配本地 zipformer
  - `"aliyun:Fun-ASR:fun-asr-realtime"` → 云端 DashScope FunASR（run-task 协议）
  - `"aliyun:Qwen-ASR:qwen3-asr-flash-realtime"` → 云端 DashScope Qwen-ASR Realtime（OpenAI Realtime 协议）
  - `"aliyun:Paraformer-Realtime:paraformer-realtime-v2"` → 云端 DashScope Paraformer 实时（run-task 协议）
  - `"bytedance:Doubao-ASR:doubao-asr-1.0-streaming"` → 云端豆包大模型 ASR 1.0（bigmodel_async 二进制帧协议）
  - `"tencent:Tencent-ASR:16k_zh"` → 云端腾讯实时语音识别（16k 中文通用，HMAC-SHA1 签名鉴权）
  - `"baidu:Baidu-ASR:15372"` → 云端百度实时语音识别（中文加强标点 dev_pid=15372，START 帧鉴权）
  - `"aliyun:qwen:qwen-plus"` / `"deepseek:deepseek:deepseek-v4-flash"` / `"bigmodel:glm:glm-4-flashx"` → LLM
  - 裸名 `"{model_name}"`（无冒号）→ 仅全局 fallback 路径用（跨 provider/category 搜，优先 local）
  - 旧 2-part（1 冒号）→ warn + 裸名兜底（迁移期）
  - 统一解析在 `infra::db::parse_model_spec`（返回 `ModelSpec::Full` / `NameOnly`），ASR 经 `asr::config::resolve_engine_in_config` 查找，LLM 经 `infra::db::load_llm_model` 查找。区分三段是因为 DB 唯一键是 `UNIQUE(domain, provider, category, model_name)`，不同 provider 或 category 下可有同名模型（如 `deepseek-v4-flash` 在 deepseek 直连与 aliyun 代管下各一行）。
- 全局默认引擎由 `resolve_active_engine(asr_engine)` 解析：**兜底引擎短路**（裸名为 `zipformer-small-ctc` 时跳过 DB 查找，直接返回硬构造兜底 entry，不触发 warning）→ 其余 spec 匹配命中则用；空/不匹配回退兜底 `zipformer-small-ctc`（`DEFAULT_ASR_MODEL_DIR` 本地打包路径，开箱可用）。返回 `ResolvedEngine.model_name` 始终是**裸名**（去掉前缀），下游缓存和加载按裸名工作。
- **云引擎路由（`provider='aliyun'` → `AliyunEngine`，`provider='bytedance'` → 豆包流式，`provider='tencent'` → 腾讯流式，`provider='baidu'` → 百度流式）**：`resolve_active_engine` 解析时若 `provider='aliyun'` → 返回 `EngineCategory::Aliyun`；`bytedance` → `ByteDance`；`tencent` → `Tencent`；`baidu` → `Baidu`（均由 `resolve_category(provider, category)` 按 provider 分支识别——`engine_category_from_str` 对云 provider 返回 `None`，靠 provider 而非 category 映射）。`desktop/src/main.rs` 启动时 `resolve_active_engine` → `Aliyun` 建 `AliyunEngine`（需开 `aliyun` feature）；`ByteDance` / `Tencent` / `Baidu` 不建独立 TranscriptionEngine（只支持流式），直接经 `is_cloud_engine` 路由到 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支）。云 ↔ 本地切换改 `app_config.asr_engine` 后**重启**生效。
- **流式判定数据驱动**：是否走流式识别由 `models.is_streaming` 列决定——`is_streaming_engine(cfg)` = `resolve_active_engine(cfg.asr_engine).entry.is_streaming && category != Aliyun && category != ByteDance && category != Tencent && category != Baidu`（seed：zipformer CTC×3 + Transducer×2 + paraformer + Qwen-ASR Realtime = 流式；whisper / sensevoice / qwen3-asr×2 / aliyun Fun-ASR / Paraformer-Realtime / bytedance Doubao-ASR / tencent Tencent-ASR / baidu Baidu-ASR = 非流式），不再按 category 硬编码匹配。**云端引擎（Aliyun / ByteDance / Tencent / Baidu）被显式排除**——其 `is_streaming=1` 表示支持云端 WS 流式（aliyun feature），而非本地 `StreamingSession`；aliyun feature 未启用时也不会错误进本地 streaming 路径。**流式引擎内部分流**：`StreamingSession::new` 检测 `decoder.onnx` 存在性——CTC 走 `StreamingZipformer`（单 session log_probs argmax），Transducer 走 `StreamingZipformerTransducer`（三 session RNN-T greedy decoding，跨 chunk 维持 `token_buf`）。**云端引擎走 `Stage::Streaming` cloud 分支（`CloudPipelineEngine`，cloud feature gated）**——Toggle 进 Idle 时 `is_cloud_engine`（检测 Aliyun / ByteDance / Tencent / Baidu）分支先于 `use_streaming` 判断并 `return`。**StreamingSession::new 失败降级**：引擎不可用时（如模型文件缺失 / category 不支持）自动降级到默认引擎 `local:zipformer:zipformer-small-ctc` 重试（warn 日志），再失败才放弃录音——避免用户选了不可用引擎后录音白白启动即失败。`run-octopus.sh` 默认启用 `--features "embedded aliyun"`，否则云端引擎不可用。Coordinator 的 `use_streaming` 据此在 Toggle 进入 `Idle`（切引擎 / 切模式）时重算——流式引擎走本地流式 partial，非流式引擎自动回退 VAD 分段伪流式。`StreamingSession::new` 同样走 `resolve_active_engine`（带兜底），与 `is_streaming_engine` 对称——避免 DB 未命中时 `is_streaming_engine` 兜底成功（→ 进 streaming 路径）但 `StreamingSession::new` 创建失败（→ session 错误）。
- 显式参数（cli `--model`、server 请求 `engine`、`AsrEngineManager.switch_model`）优先级更高，支持 spec 格式、**不走兜底**（匹配不到直接报错）。
- VAD 模型固定路径（`find_silero_vad` 直接返回 `~/.octopus/models/silero_vad_v4.onnx`），不进 DB、不读配置。
- **手编 `models` 表 / `app_config` 表需重启进程生效**（`OnceLock` 缓存，运行中不可热更新；运行时修改走 `RuntimeConfig` + `persist_*`）。DB schema `user_version` 当前为 3（v0/v1→v3 直跳，v2→v3 ALTER TABLE 补 category 列）。

### 云端 ASR 引擎（AliyunEngine + ByteDance 流式）

#### AliyunEngine（阿里云 DashScope，分块式）

`crates/desktop/src/engine_aliyun.rs`（`cloud` cargo feature 后，默认不开）impl `TranscriptionEngine`，接入阿里云百炼 DashScope 实时语音识别 WebSocket。与本地引擎不同：**不在 ASR crate 内**，而在 desktop crate——因为它是分块式 `TranscriptionEngine`（每段 VAD 开一条 WS 跑完整协议），与本地离线引擎共享 coordinator 的 chunk 路径接口（`is_streaming=0` → 不进本地 `StreamingSession`）。

**三套协议自动分发**（`is_qwen_realtime_endpoint` 按 endpoint 路径分流）：

| 接口 | endpoint | 协议 | model_name seed |
|---|---|---|---|
| Fun-ASR | `/api-ws/v1/inference` | 任务型（run-task） | `fun-asr-realtime` |
| Paraformer | `/api-ws/v1/inference` | 任务型（run-task） | `paraformer-realtime-v2` |
| Qwen-ASR | `/api-ws/v1/realtime` | OpenAI Realtime 风格 | `qwen3-asr-flash-realtime` |

- **Fun-ASR / Paraformer 协议流程**（`run_session`）：① parse_model_spec → 取 model_name → 查 `cfg.asr.aliyun[model_name]` 拿 endpoint + secret_key（空则 bail 明确报错含 sqlite3 命令）；② WS 握手 + `Authorization: bearer <key>`；③ 发 `run-task`（text frame，streaming=duplex，format=pcm，sample_rate=16000，language_hints，**`input:{}` 必须在 `payload` 内部**）；④ 流式发二进制 PCM 帧（f32[-1,1]→s16le，200ms 分块）；⑤ 发 `finish-task`；⑥ 收 `result-generated` 按 `sentence_id` + `sentence_end` 跨句累积（heartbeat=true 跳过）；⑦ `task-finished` 收尾。段级超时 8s。
- **Qwen-ASR Realtime 协议流程**（`run_qwen_realtime_transcribe`）：① URL 追加 `?model=<model_name>`；② WS 握手 + `Authorization: Bearer <key>`；③ 发 `session.update`（Manual 模式 turn_detection=null，pcm/16k）；④ 发 base64 PCM via `input_audio_buffer.append`（200ms 分块）；⑤ `input_audio_buffer.commit` + `session.finish`；⑥ 收 `conversation.item.input_audio_transcription.completed`（transcript 字段）；⑦ `session.finished` 收尾。
- **鉴权**：WS 握手请求经 `IntoClientRequest` + 追加 `Authorization: bearer/Bearer <secret_key>` header。
- **无运行时状态**：每次 `transcribe` 从 DB 重新解析 → 取最新 endpoint/key（运行时切引擎可即时生效）。
- **健康检查**：`health_check()` 保守返回 `true`，避免每次启动探活消耗 API 额度；真实健康度在首次 transcribe 时由错误路径暴露。

#### ByteDance（字节跳动豆包大模型 ASR，双向流式）

`crates/desktop/src/bytedance_stream.rs`（`cloud` cargo feature 后）接入火山引擎豆包大模型 ASR 1.0/2.0 的双向流式优化版（`bigmodel_async`）。**不实现 `TranscriptionEngine`**（豆包只支持流式，无 chunk 离线接口），直接经 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支），由 `bytedance_stream::open` 返回 `CloudStreamHandle` 管理一条 WSS 长连接。与其他 provider 共享 `PcmFrame` / `StreamEvent` 类型（`cloud_types.rs`）。

**协议**（二进制帧，与 Aliyun 的 JSON 文本帧完全不同）：

| 维度 | 说明 |
|---|---|
| Endpoint | 固定 `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async`（不来自 DB） |
| 鉴权 | WS 握手 headers：`X-Api-Key`（= `secret_key`）、`X-Api-Resource-Id`（= `source`，如 `volc.bigasr.sauc.duration`）、`X-Api-Request-Id`（UUID）、`X-Api-Sequence: -1` |
| 帧格式 | 4B header（ver/hdr + msg_type/flags + ser/comp + reserved）+ 4B payload_size + payload（全大端序） |
| 消息类型 | `0x1` FULL_CLIENT_REQUEST（初始 JSON config）、`0x2` AUDIO_ONLY_REQUEST（音频帧）、`0x9` FULL_SERVER_RESPONSE（JSON 结果）、`0xF` ERROR_RESPONSE |
| Flags | `0x0` NO_SEQUENCE、`0x2` NEG_SEQUENCE（末帧/负包，告诉服务端音频结束） |
| 压缩 | `0x1` GZIP（音频 PCM + config JSON 均 gzip；响应 payload 也 gzip） |
| 结果 JSON | `{"result":{"text":"全文","utterances":[{"definite":true,"text":"..."}]}}` |
| 结束信号 | 客户端发 flags=0x2 末帧 → 服务端回 flags=0x3（NEG_WITH_SEQUENCE） |

**DB 映射**：`source` = Resource ID、`secret_key` = API Key、`model_name` = `doubao-asr-1.0-streaming` / `doubao-asr-2.0-streaming`。详见 [spec](superpowers/specs/2026-06-21-bytedance-asr-design.md)。

#### Tencent（腾讯云实时语音识别，双向流式）

`crates/desktop/src/tencent_stream.rs`（`cloud` cargo feature 后）接入腾讯云实时语音识别 WebSocket API。**不实现 `TranscriptionEngine`**（只支持流式），直接经 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支），由 `tencent_stream::open` 返回 `CloudStreamHandle` 管理一条 WSS 长连接。与其他 provider 共享 `PcmFrame` / `StreamEvent` 类型（`cloud_types.rs`）。

**协议**（URL 签名鉴权 + Raw PCM binary + JSON text 响应）：

| 维度 | 说明 |
|---|---|
| Endpoint | 固定 `wss://asr.cloud.tencent.com/asr/v2/<appid>?{params}`（appid 在路径段，参数在 query） |
| 鉴权 | **URL 签名（HMAC-SHA1）**：参数按字典序排序→拼签名原文→`Base64(HMAC-SHA1(str, SecretKey))`→URL-encode→追加到 URL |
| 必填参数 | `secretid`、`timestamp`、`expired`、`nonce`、`engine_model_type`（如 `16k_zh`）、`voice_id`（UUID）、`signature` |
| 可选参数 | `voice_format=1`（PCM）、`needvad=1`、`filter_punc=1`、`vad_silence_time=1000` |
| 音频帧 | WebSocket Binary 帧，**原始 PCM s16le 字节**（无压缩/无额外头），200ms = 6400 字节 |
| 响应 | Text 帧 JSON：`code`（0=正常）、`result.slice_type`（0=开始/1=非稳态/2=稳态终态）、`result.voice_text_str`、`final=1`（全部结束） |
| 结束信号 | Text 帧 `{"type":"end"}` → 服务端返回 `final=1` → 断开 |
| 文本累积 | 客户端按 `index` 存 `slice_type=2` 稳态句（`BTreeMap<index, text>`），partial（0/1）覆盖当前句 |

**DB 映射**：`source` = `{appid}:{secretid}` 复合（冒号分隔）、`secret_key` = SecretKey（HMAC 签名密钥）、`model_name` = `engine_model_type`（如 `16k_zh` / `16k_zh_en`）。详见 [spec](superpowers/specs/2026-06-21-tencent-asr-design.md)。

#### Baidu（百度智能云实时语音识别，双向流式）

`crates/desktop/src/baidu_stream.rs`（`cloud` cargo feature 后）接入百度智能云实时语音识别 WebSocket API。**不实现 `TranscriptionEngine`**（只支持流式），直接经 `CloudPipelineEngine`（`Stage::Streaming` cloud 分支），由 `baidu_stream::open` 返回 `CloudStreamHandle` 管理一条 WSS 长连接。与其他 provider 共享 `PcmFrame` / `StreamEvent` 类型（`cloud_types.rs`）。协议最简洁——无 HMAC 签名、无 gzip 压缩、无二进制帧头。

**协议**（START 帧鉴权 + Raw PCM binary + JSON text 响应）：

| 维度 | 说明 |
|---|---|
| Endpoint | 固定 `wss://vop.baidu.com/realtime_asr?sn=<UUID>` |
| 鉴权 | **START 帧 JSON `data` 内直接传 appid + appkey**（无 HMAC、无 token、无 header） |
| 初始化 | Text 帧 `{"type":"START","data":{appid,appkey,dev_pid,cuid,format:"pcm",sample:16000}}` |
| 音频帧 | WebSocket Binary 帧，**原始 PCM s16le 字节**（无压缩/无头），160ms = 5120 字节 |
| 响应 | Text 帧 JSON：`type`（`MID_TEXT` 临时/`FIN_TEXT` 稳态/`HEARTBEAT` 心跳）、`result`（文本）、`err_no`（0=正常） |
| 结束信号 | Text 帧 `{"type":"FINISH"}` → 服务端完成识别后关闭连接 |
| dev_pid | 15372（中文加强标点，推荐）、15376（多方言，需 user 参数）、17372（英文加强标点） |

**DB 映射**：`source` = AppID（纯数字字符串）、`secret_key` = API Key（appkey）、`model_name` = dev_pid 字符串（如 `15372`）。详见 [spec](superpowers/specs/2026-06-21-baidu-asr-design.md)。

## 支持的 ASR 引擎

| 引擎 | 类型 | 特点 |
|------|------|------|
| Whisper | 离线 | 多语言；传 `auto` 且 DB `models.language` 配了具体语种时优先用后者（`entry_language` 覆盖），否则自动检测 |
| SenseVoice | 离线 | 快速，自动语言检测 |
| Paraformer | 离线/流式 | 中文优化 |
| Qwen3-ASR | 离线 | 大模型能力；`auto`/空时不注入 language 让模型自检（支持中英混合），显式语种时注入 `language X`；模型自检的 `language <词> <|asr_text|>` 前缀由 `decode_tokens` 剥离（按 token ID 定位 + `trim_start` 容忍 BPE 引导空格） |
| Zipformer | 离线/流式 | CTC + Transducer（RNN-T）；路由层检测 `decoder.onnx` 分流 |
| Moonshine | 离线 | 英文优化，轻量（24M/58M）；`optimize_for_inference` 可能引发 layout 计算错误，`MoonshineEngine` 跳过 optimize 直接用原始 session |
| Aliyun（云端） | 流式（cloud engine） | 阿里云 DashScope 三协议：Fun-ASR/Paraformer（run-task）/ Qwen-ASR Realtime（OpenAI 风格）；详见上方「云端 ASR 引擎」 |
| ByteDance（云端） | 流式（cloud engine） | 字节跳动豆包大模型 ASR（bigmodel_async，二进制帧 + gzip）；详见上方「云端 ASR 引擎」 |
| Tencent（云端） | 流式（cloud engine） | 腾讯云实时语音识别（WebSocket，HMAC-SHA1 URL 签名）；详见上方「云端 ASR 引擎」 |
| Baidu（云端） | 流式（cloud engine） | 百度智能云实时语音识别（WebSocket，START 帧鉴权）；详见上方「云端 ASR 引擎」 |

### Zipformer 引擎族（CTC vs Transducer）

`EngineCategory::Zipformer` 下有两个引擎 struct，由路由层（`engine.rs` `switch_model`）在实例化时检测模型目录下有无 `decoder.onnx` 自动分流：

| Struct | 模型目录特征 | 解码方式 | 典型模型 |
|---|---|---|---|
| `ZipformerCtcEngine` | 仅 `model.int8.onnx`（单 session） | CTC argmax + blank/repeat skip | `zipformer-ctc` / `zipformer-small-ctc` / `zipformer-multi` |
| `ZipformerTransducerEngine` | `encoder.int8.onnx` + `decoder.onnx` + `joiner.int8.onnx`（三 session） | RNN-T greedy decoding | `sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30`（154M）/ `zh-xlarge-int8-2025-06-30`（726M） |

**流式路径同样分流**——`StreamingSession` 在创建时检测 `decoder.onnx`，分流到 `StreamingZipformer`（CTC）或 `StreamingZipformerTransducer`（RNN-T）。两者实现 `ZipformerStreamOps` trait，`StreamingSession` 通过 trait 统一分发 `accept_samples` / `flush` / `finish` / `reset`。Transducer 流式引擎跨 chunk 维持 `token_buf`（decoder 上下文窗口）+ encoder 缓存状态。

**CTC**：单 session，encoder 同时输出 log_probs，逐帧 argmax，跳过 blank(0) 和重复 token。

**Transducer（RNN-T）**：三 session 协作，遵循 sherpa-onnx 流式 greedy search 约定——
- **encoder**：与 CTC 版状态管理完全相同（cached_key/N、cached_val/N、cached_conv/N、embed_states、processed_lens），输出 encoder_out `[T', enc_dim]` 而非 log_probs
- **decoder**：无状态，输入最近 `context_size`（默认 2）个 token `[N, 2]` → decoder_out `[enc_dim]`
- **joiner**：融合 encoder frame + decoder_out → logit `[vocab_size]`，argmax 决定发射 / blank
- **解码循环**：每个 encoder frame 跑 joiner → argmax；非 blank(0) 则发射 token、滑动窗口更新、重跑 decoder；blank 则移到下一 frame。内循环安全上限 20 次/frame（防理论无限循环）
- **encoder_dim 动态读**：从 encoder 输出 shape 最后一维读（zh=512, xlarge=768），不硬编码
- **token_buf 初始化**：`[-1, ..., -1, 0]`（长度 = context_size，末位 blank），结束后剥离 context padding

两个引擎共享 `load_vocab`（tokens.txt 解析）、`initial_encoder_states`（encoder 缓存初始化）、`decode_token_ids`（token ID → 文本，支持 BBPE + SentencePiece byte-fallback）三个自由函数。

**Whisper 特征归一化（per-chunk，与 sherpa-onnx 一致）**：使用 whisper 特征（`is_whisper=true`，即 Transducer 系列 + `zipformer-ctc`）的模型，其 `normalize_whisper_features` 公式为 `mel = (max(log10(clamp(x, 1e-10)), max_v - 8.0) + 4.0) / 4.0`（与 sherpa-onnx `NormalizeWhisperFeatures` 完全一致）。**关键约束**：① 归一化公式最后一步是 `(x + 4) / 4`（范围~0-2），不是简单的 shift（曾错误用 `x - clamp_min`，范围 0-8，尺度差 4 倍）；② 流式引擎做 **per-chunk 归一化**（每个 chunk 独立 normalize 后送 encoder），与 sherpa-onnx 行为一致——此前误改为 pseudo-global（每次重算 history+buffer 全局归一化）反而导致 max_v 跨 tick 不稳定；③ Transducer 流式引擎的 `history_samples` 仅保留最后 1 帧（160 samples），与 CTC 一致——此前保留全部未消费样本导致 history 无限膨胀。

## 拼音纠错与热词校正 (ASR Corrector)

为了在不引入重型深度学习模型（如 MacBERT 等动辄几百 MB 的模型）的前提下，实现极致轻量的纠错与专有名词（热词）校正，项目实现了一套基于 **“拼音映射 + 长度归一化 Bigram 转移概率”** 的轻量级后处理纠错引擎。

### 核心特性
- **纯静态与轻量化**：纠错所需的 unigram 词表与 bigram 共现表（各精简至高频的前 40,000 条，压缩后约 450KB）直接通过 `include_bytes!` 静态嵌入二进制中，无需额外网络下载，运行时解压，额外内存占用约 30MB。数据源自 jieba `dict.txt.big`（unigram）与 gotokenizer `bigram.txt`（bigram），由 `crates/asr/scripts/generate_corrector_data.py` 离线生成到 `src/corrector_data/*.txt.gz`（已提交；更新语料时手动重跑该脚本）。
- **配置开关控制**：由 `app_config` 表中的 `asr_correct` 字段控制（默认 `false`）。
- **智能排除**（两类跳过）：①「非语言原因」——Qwen3-ASR (0.6B/1.7B) 输出带标点且自带语义纠错能力，引擎经 `OfflineAsrEngine::skip_corrector()` 返回 true 跳过；②「language=en」——corrector 是中文拼音纠错器，对英文无意义且可能扰动，`transcribe_with_vad` 在注入点基于 `language=en`（desktop=`config.language`、server=请求、CLI=`--language`）自动跳过，覆盖 moonshine 等 en-only 模型。故 corrector 实际作用于 Whisper、SenseVoice、Paraformer、Zipformer 等中文引擎。

### 纠错算法逻辑
1. **滑窗候选召回 (Sliding Window)**：使用 2 字和 3 字的字符滑窗扫描识别出的文本，通过拼音库计算滑窗文本的拼音，并在此拼音的 $O(1)$ 模糊拼音倒排索引（支持南方口音混淆，如 `zh/ch/sh` <-> `z/c/s`、`in/en` <-> `ing/eng`、`n` <-> `l` 等）中召回**相同字符长度**的同音/近音候选词。
2. **局部上下文打分 (Local Context Scoring)**：每个候选词的评分取**窗口前后各 15 字**（共 ≤33 字）做 `jieba.cut` 分词 + Bigram 打分，而非全句分词。利用未登录词（typo）容易被 `jieba` 拆碎分词的特性，使用 **「句子总 log 概率 / 分词后 Token 数量」** 归一化消除长度偏置。候选词打分用**增量 gain**（候选局部分 − 原词局部分 + 惩罚），比绝对分更准确（消除无关上下文噪声）。
3. **基于 Jieba 字典的自适应惩罚**：
   - 如果原滑窗词是 Jieba 字典中的已登录词（即 `jieba.cut().len() == 1`，说明它是合法的词，如 `"坐上"`），系统施加极高的修改惩罚（`-1.5`）以保护正确表述不被误改；
   - 如果原滑窗词是未登录词（typo，如 `"以经"` 被 Jieba 拆分为 `"以"` 和 `"经"`），则修改惩罚降低（`-0.2`）以积极纠错。
4. **单次贪心扫描**：`correct_greedy` 从左到右单次 `while` 扫描，每处取最优候选词**原地替换**后步进整个窗口宽度（`i += sz`，跳过已纠正字防重叠二次纠错），未替换才 `i += 1`，替代旧 `correct_depth` 的递归回头（最多 5 轮全句扫描）。性能从 $O(N^3 \cdot K)$（全句 clone + 全句分词 × 候选数 × 递归轮数）降到 $O(N \cdot K \cdot 30^2)$（局部窗口分词 × 候选数 × 单轮）。

## ASR 输出简繁归一化 (Hans Variant Normalization)

ASR（尤其 Qwen3-ASR 在 `language=auto` 下）输出会混入繁体字；sherpa-onnx [#3509](https://github.com/k2-fsa/sherpa-onnx/issues/3509) 显示 `language` 参数不可靠。故在 ASR 输出边界做**单字级字形归一化**（保持 auto 多语言优势，不依赖 language 参数）：

- **实现**：`crates/asr/src/hans.rs`，基于「开放词典网」(kaifangcidian.com) CC-BY 3.0 单字对照表（`data/t2s.txt` 繁→简、`data/s2t.txt` 简→繁，`include_str!` 编译期嵌入，零运行时文件依赖）。仅转字形、不转地域用词（"愚能"转换）；简→繁一对多取数据首选（已消歧，如「发→發」）。
- **开关**：`app_config.output_simplified`（默认 `true`=简体）；`true`→繁转简，`false`→简转繁。
- **注入点**：`engine.rs::transcribe_with_vad` 返回前（offline 统一出口）+ `streaming_engine.rs::finish` 返回前（streaming 统一出口），在 corrector 之后、paste/入库之前。增量中间显示段不转换（短暂过程，最终输出归一化）。

## ASR 硬件加速与自动降级机制 (ASR Hardware Acceleration & Fallback)

为了最大化利用用户本机的 GPU 资源加速语音识别，同时避免因显卡驱动或算子不支持导致应用程序崩溃，系统在 `octopus-asr-local` 核心引擎中实现了一套手自动一体的硬件加速及平滑降级机制。

- **开关**：`app_config.asr_hardware_accelerated`（`bool`，默认 `false`）。`false` 直接走 CPU。
- **按平台注册 EP**（关键修正：曾跨平台全注册 CUDA+DirectML+CoreML，macOS 上 init Linux/Windows 专用 EP 的失败路径直接 segfault——SIGSEGV 绕过 Rust 的 `match Err`、进程被 OS 杀无法 catch，故必须按平台预防）：macOS 仅 CoreML、Linux CUDA、Windows 仅 DirectML（2026-06-20 起删 CUDA——DirectML 通吃 DX12 GPU，实时转写够用，YAGNI）。
- **feature-level 二道防线**（2026-06-20）：除上述代码层 `#[cfg]` 按平台注册，`crates/asr/Cargo.toml` 的 ort feature 也按平台条件化（target-specific dependency：mac=coreml / linux=cuda / win=directml，base 仅 `download-binaries`）。cuda/directml feature 在 mac 关闭 → 即便代码层 cfg gate 被退化、误在 mac 注册 CUDA EP，ort `register()` 也会因 feature off 直接返回 `MissingFeature`、不走 FFI dlopen-libcuda（segfault 那条路径），从而不崩。详见 [spec](superpowers/specs/2026-06-20-archived-design.md)（📄 `2026-06-20-ort-cross-platform-feature-design.md` §7.3，已归档）。
- **两层降级**：① EP 注册失败（驱动/库缺失）→ 捕获 `Err` 回退纯 CPU session，进程不崩；② **qwen3-asr 显式跳过 CoreML**——其动态算子 CoreML **不报错而是把图分区**跑（CoreML 跑支持的算子、CPU 跑剩下的，CPU↔CoreML 张量拷贝开销 dominate，比纯 CPU 还慢），故检测 active 引擎 `category=qwen3-asr` 时主动走 CPU。zipformer 等静态图照常吃满 CoreML。
- **VAD 免加速**：Silero VAD 极小（1.8MB）+ 实时性要求极高，上 GPU 的上下文切换开销远超收益，固定 CPU，不受 `asr_hardware_accelerated` 影响。

> 详见 [`docs/asr_archiveture_opt.md`](asr_archiveture_opt.md) §6.1（两层降级完整描述）。

## 技术栈

- **推理引擎**: ONNX Runtime（通过 ort crate）；可选硬件加速——按平台注册 CoreML/CUDA/DirectML execution provider（`app_config.asr_hardware_accelerated` 控制，默认 `false`，两层降级见上节），VAD 固定 CPU。config 经 `APP_CONFIG` OnceLock 缓存避免每次 session 构建重复读 DB。
- **音频处理**: cpal（录音）、rubato（重采样，含 denoise 48k 桥接）、nnnoiseless（RNNoise 降噪）、rustfft（各引擎 fbank STFT）、hound（WAV 读取）
- **Web 框架**: Axum + Tokio
- **桌面框架**: Tauri 2
- **模型加载**: HuggingFace Hub 本地缓存
- **嵌入式存储**: rusqlite（`bundled` feature，自带 SQLite C 库）— desktop 用，存识别历史 + 模型配置
