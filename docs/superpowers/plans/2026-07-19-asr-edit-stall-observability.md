# ASR 编辑态卡顿可观测性诊断日志 — 实施计划

> 配套 spec：[`2026-07-19-asr-edit-stall-observability.md`](../specs/2026-07-19-asr-edit-stall-observability.md)
>
> Plan 是「实施记录」而非「一次性待办」——最终反映实际实现。

## 任务分解

### Task 1: 扩展 `perf_log.rs` 模块 doc ✅

**变更点**：
- 顶部 `//!` 文档：从"临时性能打点"改为"可观测性日志（双重职责）"
- 明确两类调用方 + 6 个 prefix 约定（`[STATE]` / `[HEARTBEAT]` / `[SPEAKING]` / `[FE]` / `[WARN]` / `[BE tick]` / `[FE writeDoc]`）
- 调整"根因定位后可整体移除"措辞——诊断部分是长期保留

**验证**：API/实现零改动，编译无变化。

**实际偏差**：无。

---

### Task 2: `coordinator.rs` 后端状态机打点 ✅

**2.1 心跳 + editing 翻转检测**

新增局部状态（`build_coordinator_loop` 闭包内，~L278 附近）：
```rust
let mut hb_last = std::time::Instant::now();
let mut hb_ticks: u64 = 0;
let mut last_editing_logged: Option<bool> = None;
```

新增 helper `log_tick_heartbeat`（放在 `stage_name` 附近）：
- editing 翻转 → `[STATE] editing {prev} -> {next} (stage=...)`
- 距上次心跳 ≥ 1s → `[HEARTBEAT] stage={} editing={} ticks_in_window={}`

三个 Tick 分支调用（在 polish_mode 同步之后、editing 分支判断之前）：
- `StreamingTick`（原 L345）
- `VadSegmentedTick`（原 L374）
- `CloudStreamingTick`（原 L360，cfg cloud）

**2.2 editing 翻转精确触发（5 处）**

- `handle_enter_edit_mode`（活跃 + Idle 两个分支）：`[STATE] enter_edit stage={} transcript_id={}`
- `Command::CommitEdit`：`[STATE] commit_edit stage={} text_len={} has_edited={}`
- `Command::Toggle` editing=true 分支后：`[STATE] toggle-during-edit committed then stopping (stage=...)`
- `Command::Cancel` editing=true 分支后：`[STATE] cancel-during-edit cleared`
- `Command::Discard` editing=true 分支后：`[STATE] discard-during-edit cleared`

**2.3 Speaking emit**

`apply_pipeline_events` 的 `PipelineEvent::Speaking` 分支：
```rust
crate::perf_log::log(&format!("[SPEAKING] emit {}", speaking));
```

**2.4 dispatch_tick 异常分支**

`_ => {}` 改为：
```rust
_ => {
    crate::perf_log::log(&format!(
        "[WARN] dispatch_tick stage={} not active, tick dropped (samples_drained={})",
        stage_name(stage), samples.len(),
    ));
}
```

**验证命令**：
```bash
cargo build -p octopus-desktop --features embedded  # 0 error 0 warning ✓
cargo test -p octopus-desktop                        # 375 passed ✓
```

**实际偏差**：无。

---

### Task 3: `pipeline.rs` Speaking 翻转打点 ✅

- `StreamingPipeline::tick`（local 路径）：翻转时补 `crate::perf_log::log("[SPEAKING] local {} silence={:.2}", ...)`，与 vad-seg 对称
- `VadSegmentedPipeline::tick`：现有 `log::info!` 旁补一行 `crate::perf_log::log("[SPEAKING] vad-seg {} has_speech={} silence={:.2}", ...)`

**验证**：`pipeline::tests::*` 全部通过（含 Speaking 相关 4 个测试），说明打点不影响行为。

**实际偏差**：无。

---

### Task 4: 前端关键事件打点 ✅

**4.1 `AsrEditor.tsx`**
- `doCommit` 入口（早期 return 后）：`void invoke("perf_log_cmd", { msg: \`[FE] doCommit text_len=${textLen}\` })`
- `enter_edit_mode` invoke 前：`void invoke("perf_log_cmd", { msg: "[FE] enter_edit_mode invoked" })`

**4.2 `Result/index.tsx`**
- `update-speaking` 监听器：用 `setIsSpeaking((prev) => { ... })` 函数式更新，翻转时打 `[FE] isSpeaking X -> Y`
- `isRecording` 三处切换：show-result / clear-result / hide-result，同样用函数式更新打 `[FE] isRecording X -> Y (event)`

**设计决定**：用函数式 setState `setX((prev) => { log; return next; })`，避免引入额外 ref 跟踪前值，且只在真正翻转时打点（防同值重复打）。

**验证**：前端 `npx tsc -b` 无新错误（只有仓库既有的 `vite.config.ts` 错误，与本次改动无关，main 分支同样错）。

**实际偏差**：无。

---

### Task 5: 编译 + 测试验证 ✅

```bash
cargo build -p octopus-desktop --features embedded  # 1m25s, Finished, 0 warning
cargo test -p octopus-desktop                        # 375 passed; 0 failed
npx tsc -b                                           # 仅 vite.config.ts 既有错误（与改动无关）
```

main 分支对照 `npx tsc -b` 同样报 `vite.config.ts(29,5): error TS2769`——确认非本次引入。

**实际偏差**：无。

---

### Task 6: 文档同步 ✅

- 新建 spec `docs/superpowers/specs/2026-07-19-asr-edit-stall-observability.md`
- 新建本 plan
- 更新 `docs/architecture.md` L468「编辑态：硬暂停」段落：补诊断日志指针

**实际偏差**：无。

---

### Task 7: 提交（不 push） ⏳

在 worktree 分支 `fix/asr-edit-stall-observe` 上 commit，**不 push、不开 PR**——等用户复现验证后再决定是否合并。

## 不做的事

- 不改硬暂停语义（不改 IDLE_TIMEOUT / editing 跳过逻辑）
- 不改 `tauri_plugin_log` 配置
- 不动 `audio.rs`
- 不删现有阈值打点（`[BE tick]` / `[FE writeDoc]`）

## 状态

**实现 + 验证完成。** 等用户在 worktree 分支跑应用、复现问题、翻 `~/.octopus/logs/asr.log` 验证诊断效果，再决定下一步（缓解恢复延迟 / 改硬暂停语义 / 调 VAD）。
