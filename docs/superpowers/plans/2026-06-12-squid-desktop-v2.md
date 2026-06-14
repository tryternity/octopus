# octopus-desktop V2 实施计划

> **Goal:** 实现非流式引擎（SenseVoice/Whisper/Qwen3-ASR）的 VAD 伪流式分段识别，让所有引擎都能"边说边识别"。

**Architecture:** 在 Coordinator 中新增 VadSegmented/WaitingCompletion 阶段，替代原有 Recording+Processing 阶段。VAD 驱动分段识别，seq 序号保证拼接顺序。

**设计文档:** `docs/superpowers/specs/2026-06-12-squid-desktop-design-v2.md` §8

---

## 前置条件

以下功能已在 V1 和 V1.5 中完成：

- [x] 流式识别（Paraformer/Zipformer）— StreamingSession + tick 驱动
- [x] 结果展示窗口 — 可拖拽、多行滚动、透明无边框
- [x] VAD 标点 — 基于 SileroVad 静音检测
- [x] overlay — 离线模式状态提示
- [x] 自动粘贴 — clipboard/direct/none 三种模式

---

## Task 1: Command 变体更新

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: 新增 VadSegmentedTick 命令**

在 `Command` enum 中新增：

```rust
enum Command {
    Toggle,
    Cancel,
    StreamingTick,
    VadSegmentedTick,                                    // 新增
    TranscriptionDone { text: Result<String, String>, seq: u64 },  // 新增 seq
    PasteDone,
}
```

- [x] **Step 2: 匹配新命令**

在 Coordinator loop 中添加 `Command::VadSegmentedTick` 分支，调用 `handle_vad_segmented_tick()`。

更新 `Command::TranscriptionDone` 匹配分支，传递 `seq` 参数。

---

## Task 2: Stage 变体更新

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: 新增 VadSegmented 和 WaitingCompletion 阶段**

```rust
enum Stage {
    Idle,
    Streaming { /* 已有 */ },
    /// VAD 伪流式：tick 驱动分段识别
    VadSegmented {
        vad: octopus_asr::vad::SileroVad,
        audio_buffer: Vec<f32>,           // 累积音频缓冲区
        overlap_tail: Vec<f32>,           // 前一窗口末尾 0.2s
        accumulated_text: String,         // 累积识别文本
        silence_duration: f64,            // 当前静音持续时长
        has_speech: bool,                 // 缓冲区是否包含语音
        active_count: u32,                // 正在进行的识别数
        next_seq: u64,                    // 下一个发送序号
        completed_seq: u64,               // 已消费到的序号
        completed_results: HashMap<u64, String>,  // 缓存乱序结果
        tick_active: Arc<AtomicBool>,     // tick 线程控制
    },
    /// 等待所有识别完成
    WaitingCompletion {
        accumulated_text: String,
        active_count: u32,
        completed_seq: u64,
        completed_results: HashMap<u64, String>,
    },
    Recording,    // 保留，作为 fallback
    Processing,   // 保留，作为 fallback
    Pasting,
}
```

- [x] **Step 2: 新增常量**

```rust
const VAD_SEGMENTED_TICK_INTERVAL_MS: u64 = 300;
const SEND_DURATION_SAMPLES: usize = 80000;   // 5s @ 16kHz
const OVERLAP_SAMPLES: usize = 3200;          // 0.2s @ 16kHz
```

> 📝 **实现演进**（2026-06-14）：上述硬编码常量在实际代码中已改为 `config.yaml` 驱动（`segment_duration` / `segment_silence` / `segment_overlap`）。切分策略也由「固定时长 + 静音双触发、均带 overlap」演进为 **静音边界切分（主，无 overlap）+ 连续超时强制切断（兜底，带 overlap）**；并修正了 overlap 设置/克隆顺序（原草案先设 `overlap_tail` 再 clone，会把当前段末尾重复拼入；现改为先 clone 再更新）。详见 spec §8.2 与 [`architecture.md`](../../architecture.md)。

---

## Task 3: handle_toggle 改造

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: Idle + 非流式 → VadSegmented**

在 `handle_toggle()` 的 `Stage::Idle` 分支中，当 `!use_streaming` 时：

1. 初始化 SileroVad
2. 初始化 VadSegmented 阶段
3. 显示 result window
4. 启动 tick 线程（300ms 间隔）
5. 删除原有 Recording 阶段的逻辑

- [x] **Step 2: VadSegmented + Toggle → WaitingCompletion 或 Pasting**

在 `handle_toggle()` 中新增 `Stage::VadSegmented` 分支：

1. 停 tick 线程
2. 发送剩余缓冲区（如有语音）
3. 如果 `active_count > 0` → WaitingCompletion
4. 如果 `active_count == 0` → 直接 Pasting

- [x] **Step 3: WaitingCompletion 忽略 Toggle**

`Stage::WaitingCompletion` 分支中 debug 忽略。

---

## Task 4: handle_vad_segmented_tick 核心逻辑

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: 实现 tick 核心函数**

```rust
fn handle_vad_segmented_tick(stage: &mut Stage, audio: &Arc<SharedAudioState>, app_handle: &tauri::AppHandle) {
    if let Stage::VadSegmented {
        vad, audio_buffer, overlap_tail, accumulated_text,
        silence_duration, has_speech, active_count,
        next_seq, completed_seq, completed_results, ..
    } = stage {
        // 1. drain 音频
        let samples = audio.drain_samples();
        if samples.is_empty() { return; }

        // 2. 追加到缓冲区
        audio_buffer.extend_from_slice(&samples);

        // 3. VAD 检测本段语音/静音
        let speech_ratio = compute_speech_ratio(vad, &samples);
        if speech_ratio >= 0.3 {
            *silence_duration = 0.0;
            *has_speech = true;
        } else {
            *silence_duration += samples.len() as f64 / 16000.0;
        }

        // 4. 判断是否发送
        let buffer_duration = audio_buffer.len() as f64 / 16000.0;
        let should_send = *has_speech && (
            buffer_duration >= 5.0 ||  // 满 5s
            *silence_duration >= 0.5   // 静音超 0.5s
        );

        if should_send {
            // 保存末尾 0.2s 作为下一段 overlap
            let overlap_start = audio_buffer.len().saturating_sub(OVERLAP_SAMPLES);
            *overlap_tail = audio_buffer[overlap_start..].to_vec();

            // 发送识别（带 overlap_tail 前缀）
            let mut send_buffer = overlap_tail.clone();  // 实际应该是上一轮的
            send_buffer.extend_from_slice(audio_buffer);
            *has_speech = false;
            audio_buffer.clear();
            *silence_duration = 0.0;

            // spawn 识别线程
            // ...
        }
    }
}
```

- [x] **Step 2: 实现 compute_speech_ratio 辅助函数**

- [x] **Step 3: 实现 start_vad_segmented_tick_thread**

---

## Task 5: handle_transcription_done 改造

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: VadSegmented 阶段处理转录完成**

1. 将结果缓存到 `completed_results[seq]`
2. 消费 `completed_seq` 连续的序号，追加到 `accumulated_text`
3. 更新 result window 显示
4. `active_count -= 1`

- [x] **Step 2: WaitingCompletion 阶段处理**

同上，额外判断：`active_count == 0` 时进入 Pasting。

---

## Task 6: handle_cancel 改造

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: VadSegmented 取消**

1. 停 tick 线程
2. 停录音
3. 清 result window
4. 回到 Idle

---

## Task 7: main.rs 更新

**Files:**
- Modify: `crates/desktop/src/main.rs`

- [x] **Step 1: 更新非流式引擎提示**

将 warn 改为 info：`"引擎 '{}' 使用 VAD 分段伪流式模式"`

---

## Task 8: 分段参数配置化

**Files:**
- Modify: `crates/desktop/src/config.rs`
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: config.rs 新增配置字段**

```rust
segment_duration: f64,   // 默认 5.0 秒
segment_silence: f64,    // 默认 500 毫秒
segment_overlap: f64,    // 默认 200 毫秒
```

- [x] **Step 2: coordinator.rs 使用配置值**

删除硬编码常量 `SEND_DURATION_SAMPLES` / `OVERLAP_SAMPLES`，改为从 config 计算：
- `segment_samples = config.segment_duration * 16000.0`
- `overlap_samples = config.segment_overlap * 16.0`
- 静音阈值比较 `silence_ms >= config.segment_silence`

---

## Task 9: 结果窗口可编辑 + 文本持久化

**Files:**
- Modify: `crates/desktop/dist/result/index.html`
- Modify: `crates/desktop/src/result_window.rs`
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: HTML 文本区域可编辑**

- 添加 `contenteditable="true"` 到 `#result-text`
- 聚焦时浅蓝背景提示
- 用户编辑时 300ms 防抖发送 `result-edited` 事件到 Rust
- 流式更新时若用户正在编辑，追加新文本而非覆盖

- [x] **Step 2: record.txt 持久化**

在 `result_window.rs` 中：
- `save_record(text)` — 覆盖写入 `~/.octopus/record.txt`
- 识别更新（`update_result`）、最终粘贴（`start_pasting`）时同步写入
- 用户编辑事件 `result-edited` → Rust 写入 record.txt

- [x] **Step 3: history.txt 归档**

在 `result_window.rs` 中：
- `archive_to_history()` — 清空时将 record.txt 归档到 history.txt
- 格式：`--- YYYY-MM-DD HH:MM:SS ---\n文本内容\n`
- `parse_history_entries()` — 按 `--- ` 分隔符解析
- 最多保留 20 条，超出删除最早的记录
- `clear_result()` 中调用 `archive_to_history()` 后删除 record.txt

- [x] **Step 4: coordinator.rs 集成 save_record**

在所有 `update_result` / `show_result` 调用后添加 `save_record(accumulated_text)`：
- `handle_streaming_tick` — 流式 tick 更新
- `handle_vad_segmented_tick` — 伪流式 tick 更新
- `handle_transcription_done` — VadSegmented / WaitingCompletion 消费结果后
- `start_pasting` — 最终粘贴前

---

## Task 10: Bug 修复

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`
- Modify: `crates/desktop/src/tray.rs`

- [x] **Step 1: 结果窗口不可见**

启动时 `show_result("")` 传空文本导致透明窗口不可见。改为传入 `"正在聆听…"` 占位文本。

- [x] **Step 2: Tray 点击退出**

`update_tray_label` 中 `MenuItem::with_id` 重复创建同 ID 项可能 panic。改为存储 toggle MenuItem handle，用 `set_text()` 更新文本。

---

## Task 11: 编译验证

- [x] **Step 1: 编译**

```bash
cargo build --package octopus-desktop --features embedded
```

- [x] **Step 2: 手动测试**

> ✅ **测试结果**（2026-06-14）：sensevoice 引擎伪流式分段识别通过——静音切分（无 overlap）/ 强制切断（带 overlap）均正常，结果窗口实时追加、快捷键粘贴、SQLite 入库全部 OK。
> ⚠️ **已知问题**（暂搁置）：Qwen3-ASR 中英混合识别失败——疑似 `config.language="auto"` 经 `qwen3_asr::transcribe`（qwen3_asr.rs:82-90）被强制为 `zh`，prompt 里写入 `language zh` 导致英文丢失。修复方向：`auto` 时不应硬编码为 `zh`，应透传 `auto` 或不注入 language 段。

```bash
# config.yaml 配 sensevoice 引擎
cargo run --package octopus-desktop --features embedded
```

测试场景：
1. 按快捷键 → result window 出现
2. 说话 5s → 第一段识别结果出现
3. 停顿 0.5s → 自动发送识别
4. 继续说话 → 新结果追加显示
5. 再按快捷键 → 粘贴全部累积文本

---

## Spec Coverage Check

| Spec Section | Task | Status |
|---|---|---|
| §8.1 VAD 伪流式目标 | Task 1-6 | ✅ |
| §8.2 核心逻辑（配置化阈值） | Task 4, 8 | ✅ |
| §8.3 状态机 | Task 2, 3 | ✅ |
| §8.4 顺序保证 | Task 5 | ✅ |
| §6.3 可编辑结果窗口 | Task 9 | ✅ |
| §6.5 文本持久化（record.txt + history.txt） | Task 9 | ✅ |
| §10 配置（segment_* 参数） | Task 8 | ✅ |
