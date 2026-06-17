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
| `config` | DB 模型配置加载（`AsrConfig`）、模型发现、引擎路由（`resolve_engine_in_config` 按 `PREFIX:NAME` spec 解析）、全局默认引擎兜底（`resolve_active_engine`） |
| `audio` | WAV 读取、重采样（`resample_to` 一次性 / `AudioResampler` 流式，支持任意 from→to 速率，含 denoise 48k 桥接）、VAD 语音过滤 |
| `denoise` | RNNoise 流式环境降噪（`nnnoiseless`，纯 Rust 移植 Xiph RNNoise，内置默认模型，48kHz/FRAME_SIZE=480→频带特征+VAD/噪声/降噪 GRU→频带增益+OLA，GRU 状态跨帧保持），采集层前置 |
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
| `result_window` | 识别结果展示（可拖拽、多行滚动、透明无边框、置顶）。顶部悬停工具栏：鼠标移入展开（窗口高度 100→132px），移出收起；6 个工具——系统设置 / 语音模型 / 降噪模式 / 润色模型 / 润色模式 / 立即润色（后五者已接通，设置为占位）。由 `config.yaml.hide_toolbar`（默认 `true`）控制：`true`=hover 显隐，`false`=始终显示 |

**窗口加载就绪（ready）机制：** 结果窗 webview 首次加载有延迟，若后端在页面就绪前 `emit('show-result')`，事件丢失导致「文本不显示 / 不弹窗」。`result_window.rs` 以 `WINDOW_READY`（AtomicBool）+ `PENDING_TEXT`（Mutex<Option<String>>）兜底——未 ready 时暂存文本，前端 `index.html` 加载完成后发起 `result_window_ready` Tauri command → 后端置 ready 并冲刷积压文本。`show_result` / `update_result` 把「判 ready + 写 pending」收进同一把 `PENDING_TEXT` 锁，与 `result_window_ready` 的 store(true)+take 互斥，消除启动首帧 TOCTOU 文本滞留。

**核心状态机（Coordinator）：**
- 单线程 mpsc channel 串行化所有事件
- 流式模式：Streaming → (Polishing) → Pasting
- 离线模式（VadSegmented 伪流式）：VadSegmented → WaitingCompletion → (Polishing) → Pasting
- **最终润色异步化**：停止后若启用润色（mode=1/2），`start_final_polish_or_paste` 进入 `Stage::Polishing`（spawn 独立线程跑 LLM 网络请求，托盘显「处理中」、结果窗显「最终润色中」），LLM 完成回调 `Command::FinalPolishDone` 后 `do_paste` 落地；未启用润色则直接 `do_paste`。**润色期间协调器线程不阻塞**，`Cancel`（Esc）可即时回滚 Idle、丢弃在途结果，`Toggle` 被互斥忽略（防并发缓存污染）。Polishing 仅持 `id` + `raw_text`（不需 Transcript 其余字段）
- **粘贴异步化（`do_paste`）**：`do_paste` 先同步 `show_result` + 置 `Stage::Pasting`（状态机线程），再把真正的落库粘贴（`paste::paste`——含 enigo 键盘模拟 + 焦点切换 `sleep`）投递到 `tauri::async_runtime::spawn` + `tokio::task::spawn_blocking`，完成后回 `Command::PasteDone`——粘贴期间不占用 Tauri UI 主线程、不阻塞协调器线程。**macOS 键盘模拟线程安全**：`paste_via_clipboard` 的 V 键用固定虚拟键码 `Key::Other(9)`（`kVK_ANSI_V`）而非 `Key::Unicode('v')`——enigo 0.6.1 的 `Key::Unicode` 在 macOS 走 `get_layoutdependent_keycode`（循环调用非线程安全的 Carbon `TIS*`/`UCKeyTranslate` API），在 `spawn_blocking` 非主线程执行会触发 SIGTRAP（`Trace/BPT trap: 5`）；`Key::Other` 直接当 keycode 用绕过 layout 查找。详见 [spec](superpowers/specs/2026-06-17-paste-enigo-macos-sigtrap-design.md)
- **取消录音（Cancel）**：结果窗按 Esc → 前端 `invoke('cancel_recording')` → `coordinator::cancel_recording` Tauri command → `Coordinator::cancel` 发 `Command::Cancel`。`handle_cancel` 跨阶段生效——Streaming 停采集 + reset 引擎，VadSegmented 停 tick + 停采集，WaitingCompletion / Polishing 丢弃在途结果，统一回 `Idle` + 隐藏 overlay / result 窗 + 托盘置 Idle（Idle 下为 no-op）。Esc 同时 `currentWindow.hide()` 提供即时反馈（区别于 RuntimeConfig 子系统的 4 个命令，`cancel_recording` 定义在 `coordinator` 模块）
- **音频采集按需启停（替代常驻，修复菜单栏麦克风指示灯常亮）**：`cpal::Stream` 所有权收归 `SharedAudioState`（`Mutex<Option<cpal::Stream>>`），不再 `std::mem::forget` 泄漏保活——**每次录音 `start()` 现场建流（`build_stream`）+ play，`stop()` pause + drop（take 出 Option 在本线程析构）**：空闲期无流、菜单栏麦克风指示灯灭、不触发麦克风权限；录音期间流持续播放、回调内 `is_recording` 作冗余守卫。**Send-safety（已根治）**：`cpal::Stream` 为 `!Send + !Sync`，但 SharedAudioState 的 Arc 被 `move` 进 Coordinator 的 `std::thread::spawn` 循环闭包、仅该线程独占持有（`audio` 不在 Coordinator 结构体字段），故 Stream 的建（start）/ 播（play）/ 停（stop）/ 析构（stop take-drop 或循环线程退出）全程同线程、无跨线程访问；cpal 回调线程只持有独立 clone 的 `Arc<Mutex<Vec>>`/`Arc<AtomicBool>`。`unsafe impl Send/Sync` 在此前提下 sound（注释记录该不变量）。建流失败由 `start()` 返回 `Err`、上层降级
- **音频初始化防闪退**：`AudioRecorder::open()` 仅校验麦克风存在（失败 `log::error` + 仍持有静音占位 `SharedAudioState`，应用进托盘不 `expect` panic）；真正的 `build_stream` 推迟到首次 `start()`，建流失败（无设备 / 权限拒绝 / 占用）由 `start()` 返回 `Err`、上层降级（采样恒空 → 识别静默 → 空文本回 `Idle`），改配置后重启恢复
- **流式重采样器缓存**：非 16kHz 麦克风源的流式重采样经 `crates/asr/src/audio.rs` 的 `AudioResampler`（有状态 `rubato::FftFixedIn` + 跨帧 leftover 缓冲）——`desktop::SharedAudioState` 持 `Mutex<Option<AudioResampler>>`，源速率不变时**复用同一规划器**（避免每 ~300ms tick 的 FFT planner 重规划开销，并保留滤波器跨帧状态保边界 glitch-free），仅 `stop` 时 `flush` 补零吐尾 + 置 `None`；`drain_samples` 不 flush。`AudioResampler` 经编译期断言 `Send+Sync`（固化 `SharedAudioState` 的 `unsafe impl` 前提，防 rubato 升级引入非 Send 字段静默退化为 UB）
- **环境降噪（RNNoise，采集层前置）**：麦克风音频送入 VAD/ASR 前，经 `crates/asr/src/denoise.rs` 的 `DenoiseProcessor`（`nnnoiseless`，纯 Rust 移植 Xiph RNNoise，内置默认模型，48kHz FRAME_SIZE=480(10ms)→频带特征 + VAD/噪声/降噪 GRU → 频带增益 → iSTFT+OLA）降低背景噪声。样本按 nnnoiseless 契约在 `[-1,1]`↔`[-32768,32767]`（i16 PCM 等价）间转换。GRU 隐状态 + 特征缓冲 **跨 `drain_samples` 周期、跨 VAD 分段连续保持**（噪声估计是连续物理过程，与 `filter_vad` 每段 reset 故意相反）；新会话 `start()` 调 `reset()`。链路 `process_pipeline`：原生SR→(`down_sampler`)→48k→DenoiseProcessor→(`resampler`)→16k（`flush` 语义同重采样器：`drain_samples` 不 flush 保连续、`stop` flush 取尾）。由 `config.yaml.denoise_enabled`（默认 true）开关。**两级降级**：`denoise_enabled=false`→零开销直通；`DenoiseProcessor::new` 失败（罕见，仅 OOM）→持 `None` + `warn`→走原 16k 重采样直通；**不 panic**。无外部模型文件依赖（内置默认 RNNoise 模型），不进 DB、不参与引擎选择。**为何弃用 DeepFilterNet3**：`dfn3.onnx` 流式逐帧导出存在模型层缺陷（把正常语音当噪声压到 ~10%，开降噪反而损害 ASR）；RNNoise 内置成熟模型，干净语音近乎无损保留（gain≈1.0）、稳态噪声保守抑制（避免 musical noise）。详见 [spec](superpowers/specs/2026-06-16-denoise-deepfilternet-design.md)（含弃用 DF3 换 RNNoise 修订记录）
- **VAD 分段切分策略**（`handle_vad_segmented_tick`）：静音边界切分（主）+ 连续超时强制切断（兜底）
  - 静音切分：检测到语音后静音 ≥ `segment_silence`（默认 500ms）→ 切分，**无 overlap**（静音是自然语句边界，下一段从干净开始）
  - 强制切断：连续语音缓冲达 `segment_duration`（默认 20s）仍未静音 → 强制切断，**保留末尾 `segment_overlap`（200ms）作下一段 overlap**（语句被硬切，需重叠保连贯）
  - **双 VAD 实例（检测流 vs 过滤，修 LSTM 状态污染）**：SileroVad 是有状态 LSTM（`compute()` 更新 `h`/`c`，`reset()` 归零）。`VadSegmented` stage 持**两个独立实例**：① 检测用 `vad`——逐 tick 喂入顺序音频、跨 tick 有状态累积（续接上下文使语音/静音边界判定更稳），喂 `compute_speech_chunks`；② 过滤用 `filter_vad`——仅 `filter_speech_from_buffer` 用，**每次过滤前 `reset()` 归零**，恢复「每段独立冷启动」语义（等价旧代码每 buffer 新建 VAD，但 ONNX Session 在录音开始时一次性创建，过滤只 reset 不重建，兼顾正确性与性能）。分离原因：检测流已按顺序见过 `samples`，而 `send_buffer`（`overlap_tail` + `audio_buffer`）与之重叠，若共用一个有状态 VAD 会双重喂入 + 跨段污染 LSTM → 段首 gating 失真（裁掉语音起音或混入前导噪声）
  - 每段经 `filter_speech_from_buffer` 过滤静音后，由 `spawn_offline_transcription_with_seq` 派发到 **Tauri 全局异步运行时**（`tauri::async_runtime::spawn`）执行 `engine.transcribe`（底层 CPU 密集推理已 `spawn_blocking` 包裹、不阻塞 runtime worker，不再为每段新建 / 销毁 current-thread Runtime），完成回 `Command::TranscriptionDone{seq}`；协调器按 `seq` 有序拼接（`completed_results: HashMap<seq,String>` + `completed_seq` 游标连续消费）。**识别失败 / 空结果仍占位该 `seq`（写空串）以保证游标连续推进**——否则缺失序号会让消费卡死、该次录音此后所有有效段积压丢失；`Command::TranscriptionDone` 另带 `session_id`（= 录音开始毫秒时间戳 = `transcript.id`），消费它的 `VadSegmented` / `WaitingCompletion` 分支先比对 `transcript.id != session_id` → 丢弃旧会话在途结果（快速双击 Toggle / 录音中重启的竞态：新会话分配新 id，旧会话残留的异步转写回调不再污染当前结果）
- **Transcript 文本状态机**：识别文本状态由 `Transcript` 结构（`crates/desktop/src/transcript.rs`）统一管理——内部用 `full`（当前完整 ASR）+ `raw_len`（上次停顿快照的 char 长度）派生 `raw`（停顿快照，润色基准）/ `increase`（停顿后增量），避免维护三份字符串。`Stage::Streaming` / `VadSegmented` / `WaitingCompletion` 各持 `transcript: Transcript` 字段，文本流经 Transcript 方法（`set_full` / `append_segment` / `display_text` / `db_text`）。停止后 `Stage::Polishing`（最终润色中，持 `id` + `raw_text`）→ `Stage::Pasting`（持 `id` + `raw_text` + `polished_text` + `polish_status`）。入库的 `engine` / `engine_mode` 在过程入库的 raw 阶段已写（`update_transcription_raw(&config.asr_engine, ..)`），`Pasting` 不再持有。详见 [spec](superpowers/specs/2026-06-14-transcript-model-design.md)
- **停顿驱动润色**：流式 / 伪流式统一——静音 ≥ `pause_polish_threshold_ms`（默认 600ms，可配置）/ 伪流式段边界完成时，把当前完整 ASR 快照（`snapshot_for_polish()` = `raw + increase`）送 LLM 全量润色（mode=2 only），**不重置流式引擎**（只读快照送 LLM，引擎状态原样保留）。修复了流式中间润色 P0（partial 全量覆盖 polished）。默认 600ms > Active Flush 500ms（用户配置需保持 > 500ms，否则润色先于尾音冲刷、快照缺尾音），润色在 tick 流程最末执行，快照可靠
- VAD 标点：基于 SileroVad 静音检测，>0.5s 静音插入逗号
- 流式尾音冲刷（Active Flush）：流式模式累积静音 ≥0.5s 时向引擎补零，强制对齐右上下文 / 触发 CIF，把憋住的尾音即时吐出；走独立路径不插逗号，每个静音段仅触发一次（`flushed` 标志，恢复说话时重置）。详见 [spec](superpowers/specs/2026-06-13-streaming-tail-flush-design.md)
- **运行时配置子系统（RuntimeConfig）**：工具栏可运行时切换 `asr_engine` / `polish_mode` / `polish_llm`，无需重启。`runtime_config.rs` 提供 `SharedRuntimeConfig`（`Arc<RwLock<RuntimeConfig>>`，挂 `tauri::State`）作为这三个字段的**可变运行时镜像**，与启动只读的 `AppConfig` 快照互补。6 个 Tauri 命令（`toolbar_state` / `list_asr_engines` / `switch_asr_engine` / `set_polish_mode` / `list_llm_models` / `switch_polish_llm`）读写镜像 + best-effort 持久化回 `~/.octopus/config.yaml`（写盘失败仅 `warn`，本次仍生效、重启回退）。**`switch_asr_engine` / `switch_polish_llm` 前端传裸 `name`，后端查 DB 取 `category` / `is_local` 构造 spec（`"local:NAME"` 或 `"CATEGORY:NAME"`）写入 RuntimeConfig + config.yaml**——保证持久化值与 `parse_model_spec` 解析一致。`list_*` 的 current 判定经 `parse_model_spec(current).name()` 提取裸名比较，兼容 spec 和裸名两种历史格式。`switch_asr_engine` 同时经 `tray::update_tray_engine_label` 实时刷新系统托盘菜单的「引擎: <name> (<mode>)」项（`TRAY_ITEMS` 缓存 `engine_info` MenuItem handle，`set_text` 更新而非重建，规避 `MenuItem::with_id` 重复 ID panic）。Coordinator 闭包持镜像句柄，**在 Toggle 进入 `Idle` 时重读 `asr_engine` 并经 `resolve_active_engine` 解析——失效（如引擎被 `is_enabled=0` 禁用）则兜底替换为 `resolved.name`（如 `zipformer-small-ctc`）写回 `config.asr_engine`**，保证 `is_streaming_engine` 判定 / `use_streaming` 重算 / `StreamingSession::new` / 离线 `transcribe` 全用同一有效引擎名（修「禁用引擎致 `StreamingSession::new` 失败、不弹结果窗」bug）；`main.rs` 启动 preheat 同样解析（preheat 与实际工作模型一致）。**每个 tick 重读 `polish_mode` 并 `Transcript::set_mode`**（立即生效，下一次润色按新模式）。详见 [spec](superpowers/specs/2026-06-15-result-window-toolbar-design.md)

**文本持久化（嵌入式 SQLite）：**
- 存储：`~/.octopus/octopus.db`（`crates/infra/src/db.rs`，全局 `OnceLock<Mutex<Connection>>`；asr crate 经 `pub use octopus_infra::db` 以 `crate::db` 暴露；cli/server/desktop 共用）
- `transcriptions` 表：识别历史，每条存原生识别全文（`raw_text`）+ 润色版（`polished_text`）+ 润色状态（`polish_status`：`off`/`done`/`failed`）+ 元数据（engine / engine_mode / created_at / char_count / duration_ms）
- **过程增量入库（schema v3）**：`transcriptions.id` = 识别开始毫秒时间戳（`INTEGER PRIMARY KEY`，应用写入，去 `AUTOINCREMENT`），兼任主键 / 业务 key / 开始时间戳；`duration_ms = finalize_now_ms - id`。入库时机分散到识别过程各事件：首次有 ASR 文本 → `INSERT`（`insert_transcription_at_id`）；分段 / 流式 partial → `UPDATE raw_text`（`update_raw_text`）；停顿润色完成 → `UPDATE polished_text`（`update_polished`）；停止 → `finalize`（含 `duration_ms`，`finalize_transcription`）。DB 失败仅 `warn` log 不阻塞识别（best-effort）。v2→v3 migration DROP 重建（旧数据无所谓）。详见 [spec](superpowers/specs/2026-06-14-transcript-model-design.md)
- **非阻塞 DB 写入（actor 模式）**：上述 `INSERT`/`UPDATE`/`finalize` 不在协调器线程同步执行——`update_transcription_raw` / `PasteDone` 等调用方仅 `get_db_sender().send(DbCommand)` 入队后立即返回，真实落库由**后台 DB 写线程**（`static DB_SENDER: OnceLock<Sender<DbCommand>>` 懒加载 spawn）单线程消费。mpsc 的 FIFO 保证同 id 的 `Insert` 必在 `UpdateRaw` 之前被消费（故 `mark_db_inserted()` 在 send 后即置位仍安全——真实顺序由 channel 保，不由标志位保）。识别主循环不再被 SQLite I/O 阻塞。
- **关机优雅 drain**：后台写线程 `&'static Sender` 永不 drop，进程 kill 时队列里未处理命令会丢失（典型路径：录音结束 → `Finalize` 入队 → 用户立即退出 → 该条记录停留未 finalize 态）。`coordinator::shutdown_db()` 置 `DB_SHUTDOWN`（AtomicBool）→ 后台线程排空 `try_iter()` 剩余命令后退出，主线程 `JoinHandle::join` 等待落库完成；`main.rs` 挂到 `tauri::RunEvent::ExitRequested`（macOS Cmd+Q / 关闭最后一个窗口触发），保证退出前队列清空。
- `models` 表：模型目录（**唯一来源**，schema 见 `crates/infra/src/db.sql`，首次建库 `user_version=0` 时整体执行一次 seed 默认引擎集；含 `is_local` / `is_enabled` / `is_streaming` 列——`load_models_at` 仅读 `domain='asr' AND is_enabled=1`，`domain='llm'` 经 `load_llm_model(spec)` 按 `PREFIX:NAME` spec 读；引擎激活由 `config.yaml.asr_engine` 决定，无 `is_active` 列，见「模型管理」）
- `model.json` / `history.txt` / `record.txt` 已从代码彻底删除——DB 是唯一配置/存储源
- `polish_status` 基于润色调用结果：未启用→`off`；启用且返回非空→`done`；启用但返回空或失败→`failed`
- 润色三档（`polish_mode`：0 关闭 / 1 仅最终 / 2 中间+最终）：中间润色由 `check_and_trigger_polish` 在停顿点触发（流式静音 ≥ `pause_polish_threshold_ms`（默认 600ms）/ 伪流式段边界），把 `Transcript.snapshot_for_polish()`（完整 ASR）送 LLM 全量润色，节流 `polish_interval`（下限 `MIN_POLISH_INTERVAL_SEC=1.0s`）；最终润色在 `start_final_polish_or_paste`（停止后）：启用润色→`Stage::Polishing` 异步线程跑 LLM，回调 `Command::FinalPolishDone` 后 `do_paste`；未启用→直接 `do_paste`。详见 [设计](superpowers/specs/2026-06-14-transcript-model-design.md)。
- 停止空文本边界：Toggle 停止录音时若 `transcript.full()` 为空（麦克风静音 / VAD 未检出语音），`start_final_polish_or_paste` 空文本分支直接回 `Idle`，必须对称清理 `result_window::hide_result` + `overlay::hide_overlay` + `tray → Idle` 三类 UI 反馈（缺一则"正在聆听…"框残留）。详见 [设计 §4.5](superpowers/specs/2026-06-12-squid-desktop-design-v2.md)。

支持三种引擎接入模式：
- **embedded**（默认）：内嵌 octopus-asr，本地推理
- **remote-ws**：通过 WebSocket 连接远程 octopus-server
- **remote-grpc**：通过 gRPC 连接远程推理服务
- **远程超时保护**：`WsRemoteEngine` / `GrpcRemoteEngine` 的 `transcribe` 以 `tokio::time::timeout(8s)` 包裹（连接 + 收发全程），`health_check` 同样 `timeout(3s)`——规避网络断开 / 后端无响应致 ASR 队列无限期卡死。超时返回 `Err`，经序列空洞修复的空串占位分支保证 `completed_seq` 连续推进、不拖死后续分段

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
- **`write_to_clipboard`**（默认 `true`）：粘贴后是否把识别结果留在剪贴板，方便他处再粘贴；与 `paste_method`（`clipboard` / `direct` / `none`）构成三模式矩阵——`clipboard` 模式 true 时不恢复原剪贴板内容、false 时恢复；`direct` 模式 true 时 enigo 输入后末尾写剪贴板、false 时不碰剪贴板；`none` 模式忽略此配置（其唯一目的就是写剪贴板）。`false` 时三种粘贴行为等同重构前现状（不破坏现有用户习惯）。详见 [spec §6](superpowers/specs/2026-06-14-transcript-model-design.md)。

**引擎选择（单一真相 = `config.yaml.asr_engine`）：**
- `models` 表无 `is_active` 列（开发期 schema 变更采用删库重初始化——见 `crates/infra/src/db.sql` 注释；`init_schema` 仅 `user_version=0→1` 一次性建表 + seed，不做 migration）。
- **模型选择 spec（`asr_engine` / `polish_llm` 统一格式）**：配置字符串支持 `"PREFIX:NAME"` 前缀格式从 DB `models` 表唯一定位模型（见 [spec](superpowers/specs/2026-06-16-model-spec-prefix-design.md)）：
  - `"local:NAME"` → `is_local=true AND name`（特殊前缀，跨 category）
  - `"CATEGORY:NAME"` → `category AND name`（如 `"bigmodel:glm-4-flashx"`）
  - `"NAME"`（无冒号）→ 等价 `"local:NAME"`，筛 `is_local=true`
  - 统一解析在 `infra::db::parse_model_spec`，ASR 经 `asr::config::resolve_engine_in_config` 查找，LLM 经 `infra::db::load_llm_model` 查找。区分前缀是因为 DB 唯一键是 `UNIQUE(domain, name, is_local, category)`，不同 category 可有同名模型（如 `deepseek` 与 `aliyun` 下都有 `deepseek-v4-flash`）。
- 全局默认引擎由 `resolve_active_engine(asr_engine)` 解析：**兜底引擎短路**（裸名为 `zipformer-small-ctc` 时跳过 DB 查找，直接返回硬构造兜底 entry，不触发 warning）→ 其余 spec 匹配命中则用；空/不匹配回退兜底 `zipformer-small-ctc`（`DEFAULT_ASR_MODEL_DIR` 本地打包路径，开箱可用）。返回 `ResolvedEngine.name` 始终是**裸名**（去掉前缀），下游缓存和加载按裸名工作。
- **流式判定数据驱动**：是否走流式识别由 `models.is_streaming` 列决定——`is_streaming_engine(cfg)` = `resolve_active_engine(cfg.asr_engine).entry.is_streaming`（seed：zipformer×3 + paraformer = 流式；whisper / sensevoice / qwen3-asr×2 = 非流式），不再按 category 硬编码匹配。Coordinator 的 `use_streaming` 据此在 Toggle 进入 `Idle`（切引擎 / 切模式）时重算——流式引擎走流式 partial，非流式引擎自动回退 VAD 分段伪流式。`StreamingSession::new` 同样走 `resolve_active_engine`（带兜底），与 `is_streaming_engine` 对称——避免 DB 未命中时 `is_streaming_engine` 兜底成功（→ 进 streaming 路径）但 `StreamingSession::new` 创建失败（→ session 错误）。
- 显式参数（cli `--model`、server 请求 `engine`、`AsrEngineManager.switch_model`）优先级更高，支持 spec 格式、**不走兜底**（匹配不到直接报错）。
- VAD 模型固定路径（`find_silero_vad` 直接返回 `~/.octopus/models/silero_vad_v4.onnx`），不进 DB、不读配置。
- **手编 `models` 表 / `config.yaml` 需重启进程生效**（`OnceLock` 缓存，运行中不可热更新）。

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

- **推理引擎**: ONNX Runtime（通过 ort crate）；可选硬件加速——CUDA/DirectML/CoreML execution provider（由 `config.yaml.asr_hardware_accelerated` 控制，默认 `false`，注册失败自动回退 CPU），VAD 不受影响（固定 CPU）。config 经 `APP_CONFIG` OnceLock 缓存避免每次 session 构建重复读 yaml。详见 [spec](superpowers/specs/2026-06-15-asr-hardware-acceleration-design.md)
- **音频处理**: cpal（录音）、rubato（重采样，含 denoise 48k 桥接）、nnnoiseless（RNNoise 降噪）、rustfft（各引擎 fbank STFT）、hound（WAV 读取）
- **Web 框架**: Axum + Tokio
- **桌面框架**: Tauri 2
- **模型加载**: HuggingFace Hub 本地缓存
- **嵌入式存储**: rusqlite（`bundled` feature，自带 SQLite C 库）— desktop 用，存识别历史 + 模型配置
