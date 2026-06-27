# 流式 ASR 首/尾字诊断与修复实施计划

> 配套 spec：[`2026-06-27-asr-streaming-token-diagnostic-design.md`](../specs/2026-06-27-asr-streaming-token-diagnostic-design.md)
> 状态：**全部完成**（Phase 1–4 + 收尾）。本文档为事后归档，记录实际执行路径与决策。

## 总览

流式 ASR（`StreamingPipeline`）首字缺失 / 启动 spurious「嗯」/ 停顿后丢字 / 尾字中段重复，按「**丢字 > 叠字**」优先级修复。4 个 Phase，10 个 commit。

```
Phase 1  zipformer 首字（确凿，先行）              fc2d387
Phase 2  诊断日志（[asr-diag]，驱动后续）           08b190f 0d452cd
Phase 3  日志驱动精准修（paraformer 5 项）          4ca57bb 5df19fc a9795ff 2dae4c8 7d66887 144812c a0464dd
Phase 4  文档 + 诊断日志清理                       本 plan + 日志清理 commit
```

## Phase 1 — zipformer 首字（确凿）

**症结**：`accept_samples` Zipformer 分支 `if was_silent { finish+reset }`，`was_silent` 取更新前 `silence_duration`；开口前静音 > 0.5s + 开口瞬间 `has_speech=false` → 每 tick 反复 reset → 清空 `token_ids` 冲首字。

**步骤**（commit `fc2d387`）：
1. `step_silence` / `detect_silence_gap` 额外返回 `has_speech` → `(was_silent_for_punct, should_flush, has_speech)`。
2. `StreamingEngine::accept_samples` trait 加 `has_speech` 参数。
3. `streaming_engine.rs` ZipformerCtc / Transducer 分支条件改 `if was_silent && !has_speech`；Paraformer 分支忽略。
4. mock 同步签名：`streaming_runner::FakeStreamingEngine`、server `pipeline.rs::FakeEngine`。
5. 单测：`streaming_runner` 加 `has_speech` 路径用例（过渡 tick 不 reset、持续静音仍分段）。

**改动是机械签名传播**（约 7 处），不触碰底层 `ZipformerStreamOps` trait。

## Phase 2 — 诊断日志（驱动后续修复）

**步骤**（commit `08b190f` + `0d452cd`）：
- `log::debug!`（热路径）/ `log::info!`（reset / force-fire 一次性），统一 `[asr-diag]` 前缀。
- **paraformer**：`process_chunk_at` mask 决策、CIF `fired`/`alpha_cache`、force-fire、跨边界去重命中、fresh_segment 消费；`run_cif` / `run_cif_final`。
- **zipformer**：reset 前段文本快照、CTC / Transducer token emit。
- **文本层哨兵**（`5df19fc`，commit 于 Phase 3）：`diag_text_dup_sentinel`——decode 后扫描相邻 CJK 叠字，验证 token 层去重是否漏网。附 `scan_cjk_dups` / `is_cjk_char` 单测。

> **为什么不走文本层去重**：paraformer 重复在 `all_token_ids` / `full_text` 内部，`prefix|delta` 拼接边界永不触发（prefix 空或 commit 后逗号隔开）；且全文折叠分不清「别别」artifact vs「爸爸」合法叠字。安全去重只能在 token 层（有 chunk 边界），文本层改观测哨兵。

## Phase 3 — 日志驱动精准修（paraformer）

复现「我想说话 / 开始语音识别」后据日志定位，逐项修：

### 3.1 跨边界 token 去重（`4ca57bb`）
`process_chunk_at` step 8：本 chunk 首个有效 token == 上 chunk 末 token → CIF 双 fire，`continue` 跳过。不影响单 chunk 内合法重复。

### 3.2 mask 策略迭代（`a9795ff` → `2dae4c8` → `7d66887`）
e2e 三轮收敛（见 spec §4.2）：
- `a9795ff`：首 chunk 不 mask left（frame0 fired 0→1）。
- `2dae4c8`：首 chunk 不 mask right（过度，中段退化）。
- `7d66887`：`mask_right = !(is_first || is_final)`，仅中段 mask right。
- 最终：`mask_left = !(is_first || fresh)`，`mask_right = !is_first && !is_final`。

### 3.3 启动「嗯」门控（`144812c`）
`streaming_runner` 加 `seen_speech` 锁存：VAD 检出首个语音前不喂 engine（丢弃启动噪声）；VAD=None 不门控；`finish_with_tail` / `reset` 同步。

### 3.4 停顿后丢字 `fresh_segment`（`a0464dd`）
`flush()` 末尾置 `fresh_segment=true`（零 padding 已把 `feat_cache` 冲成静音 → 新段首 chunk 不 mask left 安全）；锁存到新段首个 fire 的 chunk 才清。`flush()` 开头先清避免误 mask 段尾。

## Phase 4 — 文档 + 诊断日志清理（本步）

### 4.1 文档（CLAUDE.md 要求）
- 新建 spec：`2026-06-27-asr-streaming-token-diagnostic-design.md` ✓
- 新建 plan：`2026-06-27-asr-streaming-token-diagnostic.md`（本文档）✓
- `docs/architecture.md`：流式章节为宏观模块描述，本次为内部状态机细节（`seen_speech` / `fresh_segment` / mask 策略）+ bug fix，**不涉及接口 / 架构变更**（`has_speech` 是 Phase 1 已纳入的 trait 参数，module 表已涵盖 `StreamingEngine` trait），故不改。

### 4.2 清理诊断日志
诊断已完成、修复已 e2e 验证，删除全部诊断期临时代码：

- **paraformer**（12 处 + 哨兵）：`accept chunk` / `flush chunk` / `mask` / `decode` / `run_cif` / `force-fire` / `cif-final` / `fresh_segment 消费` / `跨边界去重` 共 9 处 `log!`；`diag_text_dup_sentinel` 调用 ×2；哨兵函数 `is_cjk_char` + `scan_cjk_dups` + `diag_text_dup_sentinel` + 注释整块；配套 2 个单测（`scan_cjk_dups_*` / `is_cjk_char_*`）。
  - **注意**：`fresh_segment 消费` / `跨边界去重` / `force-fire` / `cif-final` 4 处 log 在 `if` 块内，块内含实际副作用语句（`self.fresh_segment = false;` / `continue;` / `acoustic.extend…; alpha_cache=0; fill(0);` / `alpha_cache = integrate;`）——**只删 log，保留副作用**。
- **zipformer**（4 处）：CTC / Transducer 各 `reset` + `emit` log。
  - **注意**：两处 `let snap = self.decode_tokens/current(false);` 仅服务于 log（纯查询无副作用），删 log 须连 `let snap` 一起删，否则 unused 警告。
- **runner**（1 处）：`if !feed { log! }` 整块删（`feed` 变量在后续 `if feed {` 仍用）。

### 4.3 验证
- `cargo test -p octopus-asr-local`：全绿（哨兵单测随函数删除）。
- `cargo build -p octopus-server`：mock 签名同步编译通过。
- grep 复核：`grep -rn '\[asr-diag\]' crates/` 应为 0。

## 收尾 — 合并 main

`worktree-model-mgmt-ui` 相对 main 超前 10 commit + 文档 + 日志清理，**全量 ff-merge** 到 main（线性历史，合并与推送独立命令）。

## 验证清单

1. `cargo test -p octopus-asr-local`（streaming 单测绿）+ `cargo build -p octopus-server`。
2. paraformer e2e：连说「开始语音识别」，首字「开」在、启动无「嗯」、停顿后段间首字不丢；查日志确认 `seen_speech` 门控 / `fresh_segment` 消费 / 跨边界去重命中（清理前）。
3. 回归：长静音分段、停顿逗号行为不变。
