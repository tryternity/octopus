# ASR 编辑态卡顿可观测性诊断日志

> 用户报告："识别了大段文本后，在其中插入修改时绿条不亮、识别停止/迟钝。之前有让打 log 到 `~/.octopus/logs/` 但没看到 log 出来。"
>
> 本文：定位根因（为何"停止"、为何"看不到 log"），设计**状态机诊断日志**（不改硬暂停语义），让下次卡顿时能从 `~/.octopus/logs/asr.log` 完整复盘后台状态。

## 症状

- 引擎：本地（用户当前环境）
- 场景：识别出一段文字后，用户在已识别文本中插入修改（光标定位 + 打字），**前台绿色指示条变灰、新文字不再追加**
- 之前已加 log 到 `~/.octopus/logs/`（commit `86ba5174`），但用户翻目录没看到 `asr.log`

## 根因（两个独立问题）

### 1. 绿条不亮 = **设计性硬暂停**（不是 bug）

代码：`crates/desktop/src/coordinator.rs:268,345-388`

```rust
// 编辑态：置位时 tick 跳过喂引擎、只排空丢弃音频（硬暂停）。
let mut editing = false;
...
Command::StreamingTick => {
    ...
    if editing {
        audio.trim_buffer(5.0); // 编辑期保留最后 5 秒音频（恢复后送 ASR，VAD 截静音）
    } else {
        dispatch_tick(&mut stage, &audio, &config, &app_handle, &tx);
    }
}
```

机制：
- 前端 `AsrEditor.tsx` CM6 `updateListener` 检测到用户编辑（`docChanged && isUserEdit`）→ `invoke("enter_edit_mode")`
- 后端 `coordinator.rs::handle_enter_edit_mode` 置 `editing = true` + `edit_buffer = display_text()`
- **三个 Tick 命令**（`StreamingTick` / `VadSegmentedTick` / `CloudStreamingTick`）在 `editing==true` 时全部跳过 `dispatch_tick`，只调 `audio.trim_buffer(5.0)`
- 后果：
  - `pipeline.tick()` 不被调用 → 不产生 `PipelineEvent::Speaking` → 不 emit `update-speaking`
  - 前端 `Result/index.tsx` 收不到事件 → 200ms 防抖后 `isSpeaking=false` → 绿条变灰
  - ASR 模型不做推理，麦克风继续运行、音频堆在缓冲里仅保留最近 5 秒

**恢复延迟叠加**（"迟钝"的体感来源）：
1. `AsrEditor.tsx:7` `IDLE_TIMEOUT = 2000`：用户停手 **2 秒**才自动 commit
2. 下一个 200ms tick 才恢复 `dispatch_tick`
3. 还要等 VAD `silence_duration < 0.3s` 才推 `Speaking(true)`

合计 ~2.4s+ 才能看到绿条复亮、新识别文本进来。

### 2. 看不到 log = **三个原因叠加**（都不需要"修"——是设计如此）

| 原因 | 证据 |
|---|---|
| **A. `asr.log` 是阈值日志，没出问题就不写** | `pipeline.rs:268` 唯一后端打点包在 `if total_ms > 30`；`AsrEditor.tsx:281` 前端打点阈值 `dt > 8ms \|\| length > 800` |
| **B. 编辑态期间后端打点根本跑不到** | `editing==true` 跳过 `dispatch_tick`（`coordinator.rs:345-388`），永远测不到 `total_ms`，永远不会打 `[BE tick]`——用户卡顿的那段时间恰恰是它最不可能工作的时间 |
| **C. 业务状态日志（`log::info!`）根本不写文件** | `main.rs:222-239` 的 `tauri_plugin_log::Builder` **未设 `targets`/folder**，默认 stderr。代码里大量 `info!("Entered edit mode ...")`、`debug!`、`warn!` 全在 stderr，桌面应用启动后用户抓不到 |

**`ls ~/.octopus/logs/` 验证**：只有 `action-bar.log`（独立写入逻辑，与 ASR 无关），没有 `asr.log`。

## 设计

### 目标

事后翻 `~/.octopus/logs/asr.log` 能完整复盘"绿条为何不亮"——区分 4 种情况：
- (a) **编辑态正常硬暂停**：`[STATE] editing false -> true` 持续，`[HEARTBEAT] editing=true` 持续，无 `[SPEAKING]` → 预期，commit 后恢复
- (b) **VAD 判静音**：`editing=false`，`[HEARTBEAT]` 在跑，无 `[SPEAKING] emit true` → 麦克风在收但没说话，或 VAD 阈值问题
- (c) **engine 异常**：`editing=false`，`[HEARTBEAT]` 在跑，可能伴随 `[BE tick]` 阈值告警或 `[WARN] dispatch_tick stage=...` → 真异常，需深查
- (d) **tick 线程挂了**：无 `[HEARTBEAT]`，`[STATE]` 之后无任何记录 → `streaming_active` 异常或线程 panic

### 不改的边界

- **不改硬暂停语义本身**（不改 `IDLE_TIMEOUT`、不改 editing 跳过逻辑）——那是另一轮决策，需要先有数据
- **不改 `tauri_plugin_log` 配置**——用户明确选了"扩展现有 perf_log.rs"路线
- **不动 `audio.rs`**——心跳不读 samples 长度
- **不删现有阈值打点**（`[BE tick]`/`[FE writeDoc]`）——保留 perf 监控

### 打点设计

**`perf_log.rs` API 不变**——它只是 append 写文件工具；"阈值 vs 无阈值"由调用方决定。模块 doc 明确双重职责（性能日志 + 诊断日志）。

新增 4 类正交打点（按 msg prefix 区分）：

| prefix | 触发时机 | 频率 | 字段 | 落点 |
|---|---|---|---|---|
| `[STATE]` | editing 状态翻转（enter/commit/toggle/cancel/discard + 心跳兜底检测） | 每次翻转 | `editing {prev} -> {next} (stage=...)` 或 `enter_edit stage=... transcript_id=...` 等 | coordinator.rs |
| `[HEARTBEAT]` | 每个 tick 累计，距上次 ≥1s 节流 | 1Hz | `stage={} editing={} ticks_in_window={}` | coordinator.rs |
| `[SPEAKING]` | VAD 说话状态翻转 + emit | 每次翻转 | `local {} silence={:.2}` / `vad-seg {} has_speech={} silence={:.2}` / `emit {}` | pipeline.rs + coordinator.rs |
| `[FE]` | 前端关键事件（enter/commit/isSpeaking/isRecording 翻转） | 每次翻转 | `enter_edit_mode invoked` / `doCommit text_len={}` / `isSpeaking X -> Y (200ms debounce)` / `isRecording X -> Y (event)` | AsrEditor.tsx + Result/index.tsx |
| `[WARN]` | dispatch_tick stage 不匹配（tick 到达但非活跃识别态） | 异常时 | `dispatch_tick stage={} not active, tick dropped (samples_drained={})` | coordinator.rs |

保留：
| `[BE tick]` | tick 总耗时 > 30ms | 阈值过滤 | `total={}ms infer={}ms samples={} changed={} is_cloud={}` | pipeline.rs（不变） |
| `[FE writeDoc]` | writeDoc > 8ms 或 total > 800 字 | 阈值过滤 | `{}ms total={} delta={} mode={}` | AsrEditor.tsx（不变） |

### 心跳节流说明

`[HEARTBEAT]` 1Hz 节流——既证明 tick 线程在跑（最坏 1s 内能感知到线程挂掉），又避免每 tick 写盘放大开销。Tick 间隔 `STREAMING_TICK_INTERVAL_MS = 200ms` → 每秒约 5 ticks，节流后只写 1 行。

### editing 翻转双保险

精确触发点（5 处）+ 心跳兜底检测（每 tick 比对 `last_editing_logged`）：
- **精确触发**保证快速 enter→commit（< 1s）不被心跳节流错过
- **心跳兜底**覆盖任何遗漏的间接复位路径（如未来新增的 stage 切换逻辑）

## 预期日志样例（正常流程）

```
2026-07-19 10:00:00.000 [FE] isRecording false -> true (show-result)
2026-07-19 10:00:00.200 [HEARTBEAT] stage=Streaming editing=false ticks_in_window=1
2026-07-19 10:00:01.200 [HEARTBEAT] stage=Streaming editing=false ticks_in_window=5
2026-07-19 10:00:02.000 [SPEAKING] local true silence=0.00
2026-07-19 10:00:02.000 [SPEAKING] emit true
2026-07-19 10:00:02.100 [FE] isSpeaking false -> true
2026-07-19 10:00:05.300 [FE] enter_edit_mode invoked
2026-07-19 10:00:05.300 [STATE] enter_edit stage=Streaming transcript_id=42
2026-07-19 10:00:05.300 [STATE] editing false -> true (stage=Streaming)
2026-07-19 10:00:06.200 [HEARTBEAT] stage=Streaming editing=true ticks_in_window=5
2026-07-19 10:00:07.400 [FE] doCommit text_len=1820
2026-07-19 10:00:07.400 [STATE] commit_edit stage=Streaming text_len=1820 has_edited=true
2026-07-19 10:00:07.400 [STATE] editing true -> false (stage=Streaming)
2026-07-19 10:00:08.200 [HEARTBEAT] stage=Streaming editing=false ticks_in_window=4
```

## 4 种"绿条不亮"情况的日志判读

| 情况 | 日志特征 | 含义 / 后续动作 |
|---|---|---|
| (a) 编辑态正常硬暂停 | `[STATE] editing false -> true` 持续，期间 `[HEARTBEAT] editing=true` 持续，无 `[SPEAKING]` | 预期行为，commit 后恢复。如果用户感觉太迟钝，进 Task 2（缩短 IDLE_TIMEOUT / Speaking 判定时机） |
| (b) VAD 判静音 | editing=false，`[HEARTBEAT]` 在跑，无 `[SPEAKING] emit true` | 麦克风在收但没说话，或 VAD 阈值/预滚有问题。查 `[BE tick]` 看 infer 是否在跑 |
| (c) engine 异常 | editing=false，`[HEARTBEAT]` 在跑，可能伴随 `[BE tick]` 阈值告警 或 `[WARN] dispatch_tick stage=...` | 真异常。stage 漂移说明状态机错乱；infer 飙高说明 ONNX 偶发慢 |
| (d) tick 线程挂了 | 无 `[HEARTBEAT]`，`[STATE]` 之后无任何记录 | `streaming_active` 异常或线程 panic。要查 `start_tick_thread` 的 spawn 点 + panic 信息（stderr） |

## 文件清单

| 文件 | 改动 |
|---|---|
| `crates/desktop/src/perf_log.rs` | 模块 doc 扩展（双重职责 + prefix 约定），API/实现零改动 |
| `crates/desktop/src/coordinator.rs` | `log_tick_heartbeat` helper + 3 Tick 分支调用 + 5 处精确翻转 + Speaking emit + dispatch_tick 异常 |
| `crates/desktop/src/pipeline.rs` | `StreamingPipeline::tick` local 路径 Speaking 打点；`VadSegmentedPipeline::tick` 补 perf_log |
| `crates/desktop/frontend/src/pages/Result/AsrEditor.tsx` | `enter_edit_mode` invoke 前打 `[FE]`；`doCommit` 入口打 `[FE]` |
| `crates/desktop/frontend/src/pages/Result/index.tsx` | `update-speaking` 翻转打 `[FE]`；isRecording 三处切换打 `[FE]` |

## 验证

- `cargo build -p octopus-desktop --features embedded`：0 error 0 warning ✓
- `cargo test -p octopus-desktop`：375 passed, 0 failed ✓
- 前端 `tsc -b`：只有仓库既有的 `vite.config.ts` 错误（与本次改动无关）✓

## 状态

**第二轮已实现，待用户复现验证。**

下次复现"绿条不亮""commit 后识别不恢复"时，翻 `~/.octopus/logs/asr.log` 对应时段，按下面 4+4 种情况判读。**确认根因后**，可决定是否进入下一轮：
- 缓解恢复延迟（缩短 IDLE_TIMEOUT / 优化 Speaking 判定时机）
- 改硬暂停语义（如编辑态仍跑 VAD 监听）
- 调 VAD 阈值/预滚
- 修 commit_edit 与 polish 状态机交互（见第二轮假设 A/G）
- 修 engine_cumulative 与 segments 失配（见第二轮假设 F）

---

# 第二轮：commit 后识别不恢复（用户澄清场景）

## 用户场景修正

第一轮把"识别停止"理解为"编辑期间绿条不亮"（设计性硬暂停），但用户澄清真正的 bug：

> 编辑时是识别停止，但我已经不编辑了，是需要在这里插入继续说话识别的。应该是他没有识别过来，我已经不在编辑态了？

即：**编辑完成（已退出 editing）后继续说话，识别没有恢复**。

## 用户当前配置

- **mode=0/1（不自动润色）**：第一轮的"polish_pending 卡住"假设（A）**在自动润色路径下才成立**，mode=0/1 下不会自动 `take_polish_input` → 排除假设 A 的自动触发路径（但手动点「立即润色」仍可能触发，见假设 G）

## 重新评估的根因（mode=0/1 下）

### 假设 B（高置信度）：VAD 状态冻结 + commit 后灌 5 秒静音

**证据链**：
- `StreamingRunner` 字段 `silence_duration`/`flushed`/`seen_speech`/`vad` LSTM 内部状态——**编辑期间全部冻结**（push_samples 不被调，streaming_runner.rs:226-270）
- `audio.trim_buffer(5.0)` 只裁到最近 5 秒（audio.rs:315-324），保留的是**编辑期间的音频**（多为静音/键盘噪声）
- commit 后第一 tick：`drain_samples` 拿这 5 秒 → `push_samples` 跑 VAD → 多数 chunk 静音 → `silence_duration += ~5.0`（streaming_runner.rs:116）
- `speaking = silence_duration < 0.3 = false`（pipeline.rs:237）→ 不 emit Speaking(true) → **绿条不亮**
- 引擎 `flush(true)` 出 prefix（delta 空）→ `changed=false` → 不 emit Emit → **不出字**
- **会自愈**：用户持续说话，speech_chunks≥2 时 silence_duration 清零 → speaking 翻 true → 亮。但 VAD LSTM 被 5 秒静音推到静音稳态，开口头几帧 prob 可能 <0.5 → **额外延迟**

### 假设 F（中置信度）：commit 后 engine_cumulative 与 segments 失配

**证据链**：
- `commit_edit`（transcript.rs:311-336）改 segments 但**不动 engine_cumulative / engine_consumed_chars**
- 引擎下次返回 full 仍以旧 engine_cumulative 为前缀 → `apply_engine_full` 走前缀分支（L124-126）→ delta = `full.skip(engine_consumed_chars)`
- 用户编辑删了字：engine_cumulative 比新 segments 长，新 full 与 engine_cumulative 不再是前缀关系 → **走 diverted 分支**（L127-144）→ 差异进 `diverted_pending`，<500 字不展示
- 用户编辑加了字：engine_cumulative 短，新 full 是 engine_cumulative + 编辑前的旧 delta → 提取出的 delta 是**已经被用户编辑覆盖过的旧文本** → push_delta_at_caret 把它作为 Raw 段插到 caret_gap → **文字重复或错位**

### 假设 D（中置信度）：caret_gap 落点 + 前端 CM6 滚动看不到

- commit_edit_apply 调 `set_caret(caret)` 或 `set_selection(s,e)`，caret_gap 落到中插位置
- push_delta_at_caret 把新字插到 caret_gap → 文本进 transcript → emit Emit → 前端 setText
- 但前端 CM6 滚动位置可能没跟到中插位置 → 用户视觉看不到新字

### 假设 G（低置信度）：editing=true 期间「立即润色」PolishDone 到达

- mode=0/1 下用户不会自动触发 polish_pending，但**手动点「立即润色」会**（PolishNow）
- 若用户：识别→点立即润色→立即开始编辑→commit→PolishDone 后到达 → 与假设 A 同构的 bug

## 第二轮新增打点

### 新增 4 类 prefix

| prefix | 触发 | 字段 | 落点 |
|---|---|---|---|
| `[POLISH]` | polish 状态机关键节点 | `take_polish_input` / `polish_apply` / `on_polish_failed` / `PolishDone` 各分支 / `PolishNow` / `auto-trigger` | coordinator.rs + transcript.rs |
| `[TICK-DETAIL]` | tick 详情（1Hz 节流） | pipeline-local / pipeline-vad-seg 两路径，含 silence/has_speech/speaking/changed/events/samples | pipeline.rs |
| `[APPLY]` | `apply_engine_full` 关键分支 | is_prefix / diverted / polish_pending / delta_len / cum_len / shown_len | transcript.rs |
| `[CARET]` | caret_gap 落点变化 | set_caret / set_selection / commit_edit / push_delta_at_caret | transcript.rs |

### asr-local 边界处理

asr-local 是底层 crate（不依赖 desktop，架构反向）。runner 内部状态（seen_speech / flushed）用 `log::debug!` 写 stderr，desktop 层 pipeline.rs 的 `[TICK-DETAIL]` 写文件做对账。如未来需要 asr-local 也写文件，把 perf_log 提升到 infra。

## 4 个新假设的日志判读（第二轮）

| 假设 | 日志特征 |
|---|---|
| **B (VAD 冻结)** | commit 后几个 tick `[TICK-DETAIL] speaking=false silence=X.XX has_speech=false`，silence 持续大；用户开口后逐渐 silence→0 has_speech→true。如果延迟过久（>5s）说明 VAD LSTM 状态问题严重 |
| **F (engine_cumulative 失配)** | `[APPLY] branch=diverted is_prefix=false diverted_len>0` 持续，或 `branch=prefix delta_len>0` 但 `[CARET] insert` 后前端 text 没变（重复字 / 错位） |
| **D (caret 落点)** | `[CARET] commit_edit caret_gap=<中间值>` 后 `[CARET] insert gap=<中间值>` 正常但前端看不见（CM6 滚动问题）—— 后端日志看似正常 |
| **G (润色冲突)** | `[POLISH]` 系列事件出现，特别是 `take_polish_input` 后用户编辑期间 `polish_apply` 到达 |

## 完整 prefix 列表（第一轮 + 第二轮）

### 阈值性能日志（临时，根因定位后移除）
- `[BE tick]`：tick 总耗时 > 30ms
- `[FE writeDoc]`：writeDoc dispatch > 8ms 或文本 > 800 字

### 状态机诊断日志（长期保留）
- `[STATE]`：editing 翻转（5 处精确 + 心跳兜底）
- `[HEARTBEAT]`：tick 线程 1Hz 节流
- `[SPEAKING]`：VAD 翻转 + emit
- `[FE]`：前端 enter/commit/isSpeaking/isRecording 翻转
- `[WARN]`：dispatch_tick stage 不匹配
- `[POLISH]`（第二轮）：润色状态机
- `[TICK-DETAIL]`（第二轮）：tick 详情 1Hz 节流
- `[APPLY]`（第二轮）：apply_engine_full 分支
- `[CARET]`（第二轮）：caret_gap 落点
