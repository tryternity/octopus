# 录音协调器（Coordinator）

> 单线程 mpsc channel 串行化所有录音生命周期事件，消除竞态条件——这是 octopus 桌面端语音输入的核心状态机。

源文件：`crates/desktop/src/engine/coordinator/`（目录，2026-07-29 拆分为 mod.rs + 8 子模块）、`crates/desktop/src/core/db_queue.rs`、`crates/desktop/src/engine/transcript.rs`、`crates/desktop/src/engine/pipeline.rs`、`crates/desktop/src/engine/cloud_pipeline.rs`、`crates/desktop/src/engine/audio.rs`、`crates/desktop/src/engine/ptt.rs`（单键三模式 FSM）。

---

## 1. 核心架构

**单线程 mpsc channel 串行化所有事件。** `Coordinator` 结构体只持一个 `tx: parking_lot::Mutex<Sender<Command>>`，所有 Tauri 命令（`toggle_recording` / `cancel_recording` / `discard_recording` / `polish_now` / `set_caret` / `set_selection` / `start_recording` 等）通过 `tx.send(Command)` 投递事件后立即返回，真正的状态机逻辑在后台线程内串行消费。

- `Coordinator::new`：仅创建 channel + clone tx + 调 `build_coordinator_loop`。
- `build_coordinator_loop`：`std::thread::spawn` 启动独占线程，内部 `loop { rx.recv() → match cmd }`，本地变量 `stage: Stage` / `editing: bool` / `edit_buffer: Option<String>` / `pending_prepare: Option<i64>` 全在闭包栈上，无共享可变状态。
- Tick 线程（streaming / vad-segmented / cloud）独立 spawn，仅向同一 `tx` 发 tick 命令，不直接改 stage。
- `tx` 包 `Mutex` 只为满足 Tauri `Send + Sync` 托管状态约束；`mpsc::Sender` 本身 `Send + !Sync`。

**Coordinator 闭包持共享 `AppConfig` 句柄**（`runtime_config: SharedRuntimeConfig = Arc<RwLock<AppConfig>>`），Toggle 进入 `Idle` 时重读 `polish_mode` 等运行时字段。激活引擎统一经 `resolve_active_engine("asr")`（coordinator helper `active_asr_engine_name()`），不再写回 `config.asr_engine`（字段已删，2026-07-17 重构后）。`is_streaming_engine()` 判定 / `use_streaming` 重算 / 引擎构造全用此 helper。

---

## 2. Stage 状态机

`enum Stage`——协调器在任一时刻处于且仅处于一个 Stage。

| Stage | 进入条件 | 退出条件 | 持有的数据 |
|---|---|---|---|
| `Idle` | 初始 / Cancel / Discard / finalize 空文本 / paste 完成 | Toggle（→ `pending_prepare`）/ Toggle 活跃会话（→ Streaming / VadSegmented） | 无 |
| `Streaming` | `begin_recording` 选 streaming 或 cloud 路径 | Toggle 停止（→ StoppingPolish / Polishing / Pasting）/ Cancel / Discard | `pipeline: StreamingPipeline`、`transcript: Transcript`、`streaming_active: Arc<AtomicBool>` |
| `VadSegmented` | `begin_recording` 选离线引擎路径 | 所有段完成（→ `WaitingCompletion`）/ Toggle 停止 / Cancel / Discard | `pipeline: VadSegmentedPipeline`、`transcript`、`tick_active: Arc<AtomicBool>` |
| `WaitingCompletion` | VadSegmented 所有段已派发、等 mpsc rx 回填 | `completed_seq` 游标追齐（→ finalize 路径）/ Cancel | 从 VadSegmented move 过来的 `pipeline` + `transcript` + `tick_active` |
| `StoppingPolish` | Toggle 停止时 `transcript.polish_pending()=true`（立即润色在途） | `PolishDone` 到达（→ finalize_after_stop） / Cancel | `transcript` |
| `Polishing` | `start_final_polish_or_paste` 启用润色（mode=1/2） | `FinalPolishDone` 到达（→ do_paste） / Cancel | `id`、`raw_text`、`segments`、`fallback_text` |
| `Pasting` | `do_paste` 置位（润色完成或未启用润色直接进入） | `PasteDone` 到达（→ Idle） | `id`、`raw_text`、`segments`、`polished_text`、`polish_status` |
| `CloudClosing`（cfg cloud） | cloud Streaming 停止、`close_async` 在飞 | `CloudStreamingDone` 到达（→ finalize_cloud） / Cancel | `transcript`、`current_partial` |
| `pending_prepare`（变量，非 Stage 变体） | Idle 下 Toggle（跨会话选中两阶段握手） | `StartRecording` 校验 `prepare_id` 匹配（→ begin_recording）/ 看门狗 200ms 超时发 `FallbackStart` / 再按 Toggle·Cancel·Discard 取消 | `pending_prepare: Option<i64>`（prepare_id） |

---

## 3. Command 枚举

| Command | 触发源 | 语义 |
|---|---|---|
| `Toggle` | 全局热键 / 托盘「开始/停止」 | Idle → 发 `prepare-record` 握手；活跃 → 停止录音走 finalize |
| `Cancel` | 结果窗 Esc → `cancel_recording` 命令 | 丢弃一切 + 删 DB 过程记录 |
| `Discard` | 工具栏「关闭」按钮 → `discard_recording` | 停止 + finalize DB（保留历史） + 跳过粘贴 |
| `StreamingTick` / `VadSegmentedTick` | 各自 tick 线程定时发 | 驱动 `drain_samples` → `pipeline.tick` |
| `CloudStreamingTick`（cfg cloud） | cloud tick 线程 100ms | 驱动 `CloudPipelineEngine::tick` |
| `CloudStreamingDone { text, session_id }`（cfg cloud） | `close_async` spawn 完成 | 非阻塞 close 的结果回传，`session_id` 跨会话护栏 |
| `PolishDone { result, session_id }` | `spawn_polish_thread` 完成 | 停顿润色 / 立即润色结果，`session_id` 护栏 |
| `FinalPolishDone { result, session_id }` | 最终润色 spawn 完成 | 最终润色结果，`session_id` 护栏 |
| `PasteDone` | `do_paste` 内 spawn_blocking 完成 | 粘贴落地 → Idle |
| `PolishNow` | 工具栏「立即润色」→ `polish_now` 命令 | 忽略 `polish_mode` 立即润色 |
| `CommitEdit { text, dirty_ranges, has_edited, caret, selection }` | 前端 CM6 提交（`edit_shortcut` / 保存按钮 / 2s idle / flush-edit） | 按 dirty ranges 劈段标 `Edited`；旧 `EnterEditMode`/`UpdateEditBuffer`/`CancelEdit` 已随 CM6 化移除 |
| `EditFlushed { flush_id }` / `FlushTimeout { flush_id }` | 前端响应 `flush-edit` 事件 commit 后回传 / 200ms 超时 | 停止录音 / 立即润色前强制提交编辑器防抖内容（flush_id 校验，四路径：Toggle/InstantStop/HandsFreeStop/PolishNow） |
| `InstantStart` / `InstantStop` | PTT 长按松开（`ptt.rs` FSM） | PTT 模式开/停——`INSTANT_MODE=true` 用 instant 视图（底部指示卡）不走 result 小条 |
| `HandsFreeStart` / `HandsFreeStop` | 短按单键 / 静音超时 10s | hands-free 常驻录音模式开/停 |
| `StartAgentRecording { task_id }` | Action Bar agent 动作（含 `{{voice}}`） | 录音给 agent 当输入，不粘贴 |
| `SetCaret { offset }` | 前端非编辑态点击 | 劈段定位 `caret_gap` |
| `SetSelection { start, end }` | 前端非编辑态拖选 | 记录 `pending_delete` + `selection_insert_offset` |
| `StartRecording { prepare_id, record_type, selection }` | 前端响应 `prepare-record` 事件 | 校验 prepare_id 后 `begin_recording(selection)`（`record_type` 区分 toggle/PTT/hands-free 视图） |
| `FallbackStart { prepare_id }` | 看门狗 200ms 超时 | 前端未响应兜底普通开 |
| `RestartCapture { stage_kind }` | 音频采集看门狗 3s 断推（§13.2） | 中断重连 + 复用 transcript |
| `UpdateRuntime` | 设置窗口/工具栏改 RuntimeConfig | 同步 `polish_mode` / `asr_correct` / `output_simplified` / `hide_toolbar` 等到 config 快照（模型激活走 switch_active_model，不经此路径） |

> **三模式层**（2026-08-01 单键化）：`ptt.rs` 的 `PttFsm` 6 态状态机用同一个键（`asr_shortcut` 单键名）按按键时长区分——长按（≥260ms）= PTT / 双击 = toggle / 短按 = hands-free；`INSTANT_MODE` + `RECORDING_MODE` static 标记视图与模式。**stage→Idle 出口统一 `reset_idle_flags()`**（2026-08-14）收口 `INSTANT_MODE` + `TRANSLATION_ACTIVE` + `RECORDING_MODE` 三 flag 清理（此前跨 6 轮审查修同型遗漏），单测守护。

---

## 4. 三种引擎分支

`begin_recording` 按引擎类型三分支对称调用（均接 `selection` 参数支持跨会话选中）：

### 4.1 Streaming（本地流式）

- 引擎：`LocalPipelineEngine` → `asr::StreamingRunner`（Paraformer / Zipformer 流式）
- Tick：200ms，`start_tick_thread`
- 数据流：`pipeline.tick(&samples, &mut transcript) → Vec<PipelineEvent>`，pipeline 内 `engine.tick → TranscriptEvent`（Partial/Committed/Final/Error）

```
mic → SharedAudioState.samples
   → drain_samples → 16k 降噪样本
   → StreamingPipeline.tick
       → LocalPipelineEngine.tick
           → StreamingRunner.push_samples → VAD 静音检测
               → accept_samples(→Partial)
               → 静音≥0.5s → flush(insert_comma=true)(→Committed)
       → apply_engine_full（取尾部 delta 在 caret_gap 生长）
   → apply_pipeline_events（PersistRaw→DB / Emit→update_result / Polish→停顿润色）
```

### 4.2 VadSegmented（离线伪流式）

- 引擎：离线 ONNX 引擎（SenseVoice / Whisper / Qwen3-ASR / FireRed 等），经 `VadSegmentedPipeline`（`pipeline.rs`）
- Tick：100ms，`start_vad_segmented_tick_thread`
- 数据流：`run_tick` 内 `audio_buffer.extend` + `compute_speech_chunks(vad)`（检测 VAD 跨 tick 有状态累积，**切段后 reset+preroll 归零防漂移**）→ 静音 ≥ `segment_silence`（默认 400ms）/ 缓冲 ≥ `SEGMENT_DURATION_S`（20s，**force_cut 不门控 has_speech**——detect_vad 漂移失灵时强制清空防堆积、由 filter_vad 独立兜底）→ `filter_speech_from_buffer`（过滤 VAD 每段 reset）→ `spawn_blocking(engine.transcribe)` → mpsc rx 按 `seq` 有序回填 `completed_results`

```
mic → drain_samples → 16k 降噪样本
   → VadSegmentedPipeline.tick
       → audio_buffer.extend + compute_speech_chunks(检测VAD)
       → 静音≥400ms / 持续≥20s → filter_speech_from_buffer(过滤VAD reset)
           → spawn_blocking(engine.transcribe)
           → mpsc rx → completed_results[seq] 有序回填
       → consume_completed_results → append_segment → [PersistRaw, Emit]
       → segment_cut → [Polish{INFINITY}]（段边界润色）
   → 所有段派发 → Stage::WaitingCompletion
```

### 4.3 Cloud Streaming（cfg cloud）

- 引擎：`CloudPipelineEngine`（`cloud_pipeline.rs`），不调 `TranscriptionEngine::transcribe`，直接管 WSS 长连接
- Tick：100ms 独立线程 `start_cloud_streaming_tick_thread`
- 四 provider：Aliyun（DashScope，三套协议自动分发）/ ByteDance（豆包 bigmodel_async）/ Tencent / Baidu，统一返回 `CloudStreamHandle`
- VAD 门控连接生命周期（不切分过滤——云端服务端自切句），onset 连续 2 tick 确认才开 WSS（`speech_confirm_count` 抗噪）

```
mic → drain_samples → 16k 降噪样本
   → CloudPipelineEngine.tick
       → pre_roll_buffer 滚动追加（保留后 200ms）
       → compute_speech_chunks(vad) → onset 检测（≥2 speech chunks 确认）
       ├─ 无活跃 WSS + onset → open_cloud_session → CloudStreamHandle
       │     → push_pcm(&samples)
       ├─ 有活跃 WSS + 持续语音 → push_pcm + drain_cloud_session
       │     → StreamEvent::Text(partial) → current_partial（预览层，不进 transcript/DB）
       │     → StreamEvent::Finished → committed_text 逗号拼接 → Committed 事件
       └─ 有活跃 WSS + 静音≥pause_polish_threshold_ms → finish()（非阻塞）→ is_closing=true
   → stop → take_close_handle → spawn close_async → Stage::CloudClosing
       → CloudStreamingDone 回传 → finalize_cloud
```

---

## 5. 音频处理流水线

三种 stage 共用同一前处理路径，所有降噪 / 重采样都在 `SharedAudioState::drain_samples` 内部完成，coordinator 层从不直接调 `DenoiseProcessor`。详见 `crates/desktop/src/audio.rs::process_pipeline`。

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
  │     └─ 降噪路径（denoise_mode=1 RNNoise / 2 DeepFilterNet3）：
  │           原生 SR ──down_sampler──▶ 48k
  │             ──DenoiseProcessor.process_samples──▶ 48k 已降噪
  │             ──resampler────────────────────────▶ 16k 已降噪
  │
  │  GRU 隐状态跨 tick / 跨段连续保持（flush=false）；
  │  仅会话级 start() 调 reset()（DF3 = 重载模型）
  ▼
samples: Vec<f32>（16k 单声道，已降噪 或 直通）—— 三种 stage 看到同一份
```

**音频采集按需启停**：`cpal::Stream` 所有权收归 `SharedAudioState`（`Mutex<Option<cpal::Stream>>`），每次录音 `start()` 现场建流 + play，`stop()` pause + drop。空闲期无流、菜单栏麦克风指示灯灭。`build_stream` 支持 F32 / I16 / U16 三类采样格式。

---

## 6. Transcript 段模型文本状态机

`Transcript`（`transcript.rs`）——识别文本状态统一管理者。内部用段模型作结构化真相源：

| 字段 | 类型 | 语义 |
|---|---|---|
| `segments` | `Vec<Segment>` | 每段 `{kind: Raw/Polished/Edited, text}`，后态覆盖前态 |
| `caret_gap` | `usize`（0..=segments.len()） | 新语音生长缝隙；==len 即末尾追加，默认零回归 |
| `pending_delete` | `Option<(usize, usize)>` | 选中替换待删范围（扁平 char [start,end)），延迟消费 |
| `selection_insert_offset` | `Option<usize>` | 选中替换插入点 = selection start，跨润色持久 |
| `engine_cumulative` | `String` | 引擎累积全量，仅 delta 提取基准，不显示不落库 |
| `diverted_pending` | `String` | 引擎纠正延迟确认暂存（上限 500 char 强制 flush） |
| `polish_snapshot` | `Vec<Segment>` | 润色发起时快照，PolishDone 回填比对用 |
| `pending_delta` | `String` | pending 期间缓存的新 delta，PolishDone 后 flush |

- `finish_text()` 段扁平化为唯一展示/落库/复制文本（派生，不另存）
- `apply_engine_full` 取尾部 delta 在 `caret_gap` 生长（VadSegmented 走 `append_segment`）
- `set_caret` / `set_selection` 经 `split_at` 劈段定位
- `Stage::Streaming` / `VadSegmented` / `WaitingCompletion` 各持 `transcript`；停止后 `Polishing` 持 `id`+`raw_text`，`Pasting` 持 `id`+`raw_text`+`polished_text`+`polish_status`

---

## 7. 停顿驱动润色 + 立即润色（PolishNow）

**停顿驱动润色**（`check_and_trigger_polish`）：
- 流式 / 伪流式统一：静音 ≥ `pause_polish_threshold_ms`（默认 600ms，GUI 约束 >= 600）/ 伪流式段边界完成时
- 经 `take_polish_input()` 取 segments 快照送 LLM **全篇一次润色**（mode=2 only）
- **不重置流式引擎**（只读送 LLM，引擎状态原样保留）
- 默认 600ms > Active Flush 500ms（须大于句间停顿最大值，否则润色先于尾音冲刷）
- pending 期间新 delta 缓存到 `pending_delta`，PolishDone 后 flush
- 节流 = `polish_min_interval`（可配，默认 **5.0s**）`.max(1.0s)` 下限兜底
- **应用感知润色路由**（2026-08-01，`prompt_route.rs::resolve_polish_prompt`）：prompts 表 `app_bundle_ids` 绑定前台 app 选专用 prompt（空=全局），`inject_context=1` 时 user prompt 头部注入「当前应用：名称」

**立即润色（PolishNow）**（`handle_polish_now`）：
- **忽略 `polish_mode`**（不受 mode=0/1/2 限制，区别于停顿润色的 mode=2 限制）
- 经 `llm_config_ignore_mode(config)` 取 LLM 配置，复用 `take_polish_input` → `spawn_polish_thread(ignore_mode=true)`
- **支持全部活跃 stage**：`Streaming` / `VadSegmented` / `WaitingCompletion` / `CloudClosing`（cloud 流式走 `Stage::Streaming`、`CloudClosing` 同样是活跃会话，必须支持）
- **所有早退路径都 emit `polish-done`**（stage 不匹配 / transcript 空 / 已 pending / LLM 配置缺失）——否则前端 `btnPolishNow.disabled=true` 永久卡死

---

## 8. 选中替换（产品核心特色）

> 用户选中已识别文本的任意一段，继续说话——新语音自动替换选区，而非追加末尾。这是本产品区别于传统语音输入工具的关键能力。

**延迟删除**（`set_selection` → `pending_delete`）：
- 拖选 → `invoke("set_selection", {start, end})` → 记录 `pending_delete` + `selection_insert_offset`，**不立即删字**（保留浏览器原生高亮，用户可重新选择）
- `pending_delete` 在**下次引擎 tick**（`apply_engine_full` / `append_segment`）被消费——在所有 early return 之前无条件消费
- **消费即返回 true**：即使 delta 为空，只要 `pending_delete` 被消费，返回 `true` → pipeline 产 `Emit` → 前端即时刷新

**`selection_insert_offset` 跨润色持久**：
- 停顿润色时 `take_polish_input` 检查此字段——有值 → `polish_caret_at_tail=false`（强制精确恢复 caret 到 selection start）
- `polish_apply` 恢复 caret 后回写此字段（多次润色仍有效）
- `set_caret` / `clear_pending_delete` / `commit_edit` 清零

**跨会话选中（Idle → Toggle 两阶段握手）**：
1. Idle 下选中文字 → 前端 `currentSelectionRef` 缓存 `{start, end, text}`（**不存后端**）
2. Toggle 开新会话**不直接开录音**：`emit("prepare-record", prepare_id)` + spawn 200ms 看门狗 + 进 `pending_prepare` 等待态
3. 前端 listen prepare-record → `invoke("start_recording", {prepareId, selection})`
4. 后端 `StartRecording` 校验 `prepare_id` 匹配 `pending_prepare` → `begin_recording(selection)`
5. cloud/streaming/vad 三分支对称：`Some` → `commit_edit(text)` + `set_selection(start, end)` 种子 transcript；`None` → 普通开
6. 看门狗 200ms 超时发 `FallbackStart` 兜底普通开（冷启动前端未 mount / 未响应）
7. 等待态中断：再按 Toggle / Cancel / Discard 取消等待；SetCaret / SetSelection no-op

---

## 9. 最终润色异步化

`start_final_polish_or_paste`——停止后润色路径：

- `polish_pending()=true` → 进 `StoppingPolish`（等 `PolishDone` 到达再走 final 路径）
- 无 pending 且已润色覆盖全部（`!has_raw()`）→ **跳过最终润色直接 paste**（mode=1/2 也跳过，避免多一次 LLM 调用）
- 启用润色（mode=1/2）→ `Stage::Polishing`（spawn 独立线程跑 LLM 网络请求，托盘显「处理中」、结果窗显「最终润色中」），`FinalPolishDone` 回来后 `do_paste` 落地
- 未启用润色 → 直接 `do_paste`
- **润色期间协调器线程不阻塞**，`Cancel`（Esc）可即时回滚 Idle、丢弃在途结果，`Toggle` 被互斥忽略

**跨会话护栏**：`FinalPolishDone` 携带 `session_id`（= 发起润色时的 transcript.id），`handle_final_polish_done` 校验当前 Polishing id 匹配才落地——Cancel+重开+再润色时旧结果匹配新 Polishing 的污染被拦（与 `PolishDone` 同理）。

---

## 10. 粘贴异步化（do_paste）

`do_paste`：
- 先同步 `show_result` + 置 `Stage::Pasting`
- 把真正的落库粘贴（`paste::paste`——含 enigo 键盘模拟 + 焦点切换 `sleep`）投递到 `tauri::async_runtime::spawn` + `tokio::task::spawn_blocking`
- 完成后回 `Command::PasteDone` → Idle
- **粘贴期间不占用 Tauri UI 主线程、不阻塞协调器线程**

**macOS 键盘模拟线程安全**：`paste_via_clipboard` 的 V 键用固定虚拟键码 `Key::Other(9)`（`kVK_ANSI_V`）而非 `Key::Unicode('v')`——enigo 0.6.1 的 `Key::Unicode` 在 macOS 走 `get_layoutdependent_keycode`（循环调用非线程安全的 Carbon `TIS*` / `UCKeyTranslate` API），在 `spawn_blocking` 非主线程执行会触发 SIGTRAP（`Trace/BPT trap: 5`）；`Key::Other` 直接当 keycode 用绕过 layout 查找。

**剪贴板联动**：`do_paste` 先调 `store::touch_created_at`（录音过程已创建 voice 条目，paste 时只需 touch 顶到列表顶部）→ 成功后主动 `emit("clipboard://changed")`（paste 写剪贴板设 suppress flag，watcher 命中后直接 return 不 emit，故 ASR 记录需主动广播）→ 再调 `paste::paste`。

---

## 11. 取消（Cancel） vs 放弃（Discard）

| 维度 | Cancel（Esc） | Discard（工具栏关闭按钮） |
|---|---|---|
| 命令 | `cancel_recording` → `Command::Cancel` | `discard_recording` → `Command::Discard` |
| 停止逻辑 | 停采集 + reset 引擎 / 断 WSS | 同 Cancel（共享停止逻辑） |
| DB 记录 | **删除**（`DbCommand::Delete`）——已 INSERT 的过程记录删掉 | **finalize 保留**（`DbCommand::Finalize`：raw_text + duration_ms + polish_status="off" 入库） |
| 粘贴 | 不粘贴 | 跳过 `do_paste`（不粘贴、不入剪贴板） |
| 适用阶段 | 跨阶段（Streaming/VadSegmented/WaitingCompletion/Polishing/StoppingPolish/CloudClosing）；Pasting no-op；Idle no-op | 同；Pasting no-op；Polishing 丢弃润色结果 |

`handle_cancel` 清理 DB 脏数据：`transcript.db_inserted()=true` 则 `DbCommand::Delete`；`Polishing` / `Pasting` 阶段（仅有 `id` 无 transcript）直接删除。`handle_discard` 额外 finalize DB 记录。

---

## 12. DB 写入队列 actor（db_queue.rs）

`crates/desktop/src/core/db_queue.rs`——ASR 识别结果的 DB 写入 actor。

**`DbCommand` enum**：

| 变体 | 字段 | 语义 |
|---|---|---|
| `Insert` | id, text, segments, engine, engine_mode | 首次有 ASR 文本时 INSERT voice 条目 |
| `UpdateTextSegments` | id, text, segments | 分段 / 流式 partial 增量更新 |
| `UpdatePolished` | id, text, status, model, segments | 停顿润色 / 立即润色完成 |
| `Finalize` | id, raw_text, segments, polished_text, polish_status, polish_model, duration_ms | 停止时完整写入 |
| `UpdateEditedSegments` | id, text, segments | 用户编辑提交 |
| `UpdateMetaField` | id, key, value | meta_info 单字段写入（cloud-gated，`update_meta_field`） |
| `Delete` | id | Cancel 时删除未完成记录 |

**后台线程**：
- `DB_SENDER: OnceLock<Sender<DbCommand>>` 懒初始化 spawn（`get_db_sender`）
- 后台线程 `recv_timeout` 轮询 `DB_SHUTDOWN: AtomicBool` 关机标志
- mpsc 的 FIFO 保证同 id 的 `Insert` 必在 `UpdateTextSegments` 之前被消费
- 调用方非阻塞 `send` 后即返回，落库在后台线程，识别主循环不被 SQLite I/O 阻塞

**关机优雅 drain**（`shutdown_db`）：
- 置 `DB_SHUTDOWN` → 后台线程排空 `try_iter()` 剩余命令后退出
- `DB_HANDLE: OnceLock<Mutex<Option<JoinHandle>>>` take 出来 `join` 等待落库完成
- `main.rs` 挂到 `tauri::RunEvent::ExitRequested`（macOS Cmd+Q / 关闭最后一个窗口触发），保证退出前队列清空

---

## 13. 空文本边界 + 云端 WSS 失败处理

**空文本边界**：
- Toggle 停止录音时若 `transcript.full()` 为空（麦克风静音 / VAD 未检出语音）
- 空文本分支直接回 `Idle`，必须对称清理 `result_window::hide_result` + `tray → Idle` 两类 UI 反馈（缺一则"正在聆听…"框残留）

**云端 WSS 连接失败**：
- `CloudPipelineEngine::tick` 中 `open_cloud_session` 返回 `Err` 时，除 `error!` 日志 + 复位 `is_speaking=false` 外
- 产 `TranscriptEvent::Error("⚠️ 云端连接失败：<msg>")`（承载层 `last_error` → 下 tick `PipelineEvent::Error` → `apply_pipeline_events` → `update_result`）
- 让用户即时感知错误而非卡在"正在聆听…"假死状态
- session 由 `!is_closing && !is_speaking` 分支自动 take，下次语音 onset 重开 WS（瞬时抖动自动重试；持续失败如 Key 无效每次 onset 报错，用户可见可排查）

## 13.1 诊断日志（perf_log 9 类 prefix）

`perf_log.rs` 承载双重职责：阈值性能日志（`[BE tick]`/`[FE writeDoc]`，根因定位后可移除）+ 状态机诊断日志（长期保留）。9 类 prefix：

| prefix | 含义 |
|---|---|
| `[STATE]` | editing 翻转（5 处精确 + 心跳兜底） |
| `[HEARTBEAT]` | tick 1Hz 心跳（stage + editing 状态） |
| `[SPEAKING]` | VAD 翻转 + emit |
| `[FE]` | 前端事件 |
| `[WARN]` | dispatch_tick 异常 |
| `[POLISH]` | 润色状态机 |
| `[TICK-DETAIL]` | tick 详情 1Hz（pipeline 两路径 silence/has_speech/samples） |
| `[APPLY]` | apply_engine_full 分支（prefix/diverted + delta/cum/shown 长度） |
| `[CARET]` | caret_gap 落点 |
| `[WATCHDOG]` | 音频采集看门狗（见 §13.2） |

**判读指南**：「绿条不亮」看 `[STATE]` editing 是否翻转 + `[SPEAKING]` 是否 emit；「commit 后不恢复」看 `[CARET]` commit 后 gap + `[APPLY]` 是否 delta 空。asr-local 不能依赖 desktop，`streaming_runner` 内部状态走 `log::debug!` stderr，desktop 层写文件对账。详见 [spec](../superpowers/specs/archived/2026-07-19-asr-edit-stall-observability.md)。

## 13.2 音频采集看门狗 + 自动重连

修复 cpal 断推后 pipeline 永久空转（日志 `samples=0 has_speech=true` 持续 30s 无提示）。

**看门狗**：`SharedAudioState` 持 `last_sample_time`，cpal 回调每次 extend 后更新；`sample_stall_duration()` 返回距上次回调时长；`dispatch_tick` 在 Streaming/VadSegmented 分支 tick 后调 `check_audio_stall()`，`>= AUDIO_STALL_THRESHOLD(3s)` → 发 `Command::RestartCapture`。3s 阈值不误判正常静音（静音时 cpal 仍推底噪 samples≠0）。

**自动重连**（`restart_capture_keep_transcript`）：中断+重启录音，**复用 transcript**（两次录音文本拼一起，识别框不隐藏）。流程：停 tick + audio.stop 取尾 + 喂尾给旧 pipeline + finish flush → 取出 transcript 保留 → `reset_engine_baseline()` 清引擎基准 → audio.start 重连（失败则二次降级 emit mic-error + finalize 粘贴）→ 重建 pipeline + transcript 放回 Stage + 重启 tick。cloud 引擎 no-op（独立 WS 连接）。详见 [spec](../superpowers/specs/archived/2026-07-24-audio-watchdog.md)。

---

## 14. 关键不变量

1. **降噪在 `drain_samples` 内部完成**——三种 stage 拿到的 `samples` 都是 16k 已降噪（或降级直通）样本；VAD 与 ASR 用同一份降噪后信号，避免参数 / 状态不一致致 VAD 误判而 ASR 准的解耦 bug。云端引擎的 pre-roll 同样从 `drain_samples` 取，云端收到的是干净音频。

2. **降噪 GRU 与 VAD LSTM 状态语义相反**：
   - 降噪 GRU **跨 tick / 跨段连续保持**（`flush=false`，噪声估计是连续物理过程，仅会话 `start()` 才 reset）
   - 检测 VAD **跨 tick 有状态累积**（看完整流，稳语音/静音边界），**切段后 `reset()`+preroll 归零**（防 LSTM 跨段漂移致真实语音持续判静音 → "几段后不吐字"）
   - 过滤 VAD **每段 reset**（独立冷启动，等价每段新 VAD 但复用 ONNX Session）

3. **降级不 panic**：`denoise_mode=0` / 后端模型缺失 / 单帧推理失败 → `process_pipeline` 走直通分支（原生→16k），仅 warn 日志，识别继续不阻断录音。

4. **cloud engine 的 VAD 用法与 VadSegmented 一致但更轻**：同一个 `compute_speech_chunks` + `SileroVad` 检测 onset，但**不切分过滤**（不调 `filter_speech_from_buffer`）——云端服务端自己有切句逻辑，客户端 VAD 只负责「何时开 / 何时关 WSS」的生命周期门控。onset 抗噪：连续 2 个 tick（~200ms）检测到语音才开 WSS。

5. **pending_delete 无条件消费**：在 `apply_engine_full` / `append_segment` 所有 early return 之前无条件消费 `pending_delete`——旧代码在 delta 空 / diverted 分支 `return false` 跳过消费会导致选区永远不删。

6. **session_id 跨会话护栏**：`PolishDone` / `FinalPolishDone` / `CloudStreamingDone` 均携带 `session_id`，handler 校验当前 stage 的 transcript.id 匹配才落地——润色 / close 线程不持 transcript 引用，回来时当前 transcript 可能已是新会话。

7. **process 落库 FIFO 保证**：`mark_db_inserted()` 在 `send` 后即置位仍安全——真实顺序由 mpsc channel 保，不由标志位保。同 id 的 `Insert` 必在 `UpdateRaw` 之前被消费。
