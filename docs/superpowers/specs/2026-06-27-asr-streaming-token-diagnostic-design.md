# 流式 ASR 首/尾字诊断与修复设计

> **目标**：修复流式 ASR（`StreamingPipeline`）的首字缺失、启动 spurious token、停顿后丢字、尾字/中段重复问题。
> **性质**：行为修复（bug fix），**无接口破坏**（`StreamingEngine` trait 的 `has_speech` 参数是本期内新增，所有实现已同步）。
> **范围**：`crates/asr-local`（`streaming_runner` / `streaming_engine` / `streaming_paraformer` / `streaming_zipformer`），server `pipeline.rs` mock 同步签名。
> **诊断方法**：`[asr-diag]` 字级 / frame 级 `log::debug!`（验证完成后已清理，见 plan Phase 4）。

## 1. 背景

用户实测流式 ASR（paraformer 为主，中文识别率高）：

1. **启动 spurious「嗯」**：开始录音、尚未说话时 final 就以「嗯，…」开头。
2. **首字缺失**：「开始语音识别」→「始语音识别」。
3. **停顿后丢字**：连续说多句，停顿后的下一句首字丢失（段 2 丢「开」、段 4 丢「始」）。
4. **尾字 / 中段重复**：「开始语音识别」→「始语音**识识**别」/「开**语语**音识别」/「**开开**语音识别」。

zipformer 同样有首字缺失（机理独立，见 §3.1），但用户重点用 paraformer，zipformer 尾字诊断搁置（从未复现）。

> **优先级（用户 2026-06-27）**：**丢字 > 叠字**。丢字使模型不可用；叠字可由 LLM 润色后处理兜底。故修复重心在「不丢字」，叠字尽力而为、剩余交 LLM。

## 2. 诊断方法：`[asr-diag]` 日志（诊断期临时，已清理）

在流式 token 生成路径插入 frame 级 / chunk 级 / token 级 `log::debug!`（统一 `[asr-diag]` 前缀），观测：

- paraformer：`process_chunk_at` 的 mask 决策（`is_first`/`enc_len`/`mask 前 alpha_sum`/`mask_left`/`mask_right`/`fresh`）、CIF `fired`/`alpha_cache`、force-fire 条件、跨边界去重命中、fresh_segment 消费。
- zipformer：reset 前段文本快照、CTC/Transducer token emit。

辅助：`diag_text_dup_sentinel`（文本层重复哨兵）——decode 后扫描相邻 CJK 叠字，验证 token 层跨边界去重是否漏网。

**这些日志是诊断工具，验证完成后已全部删除**（17 处 `log!` + 哨兵函数及其单测），见 plan Phase 4。

## 3. 根因分析

| # | 症状 | 引擎 | 根因 | 置信度 |
|---|---|---|---|---|
| 3.1 | 首字缺失 | **zipformer** | `accept_samples` Zipformer 分支 `if was_silent { finish+reset }`；`was_silent` 取 `step_silence` 的 `prev`（**更新前** silence_duration）。开口前静音 > 0.5s、开口瞬间能量低 `has_speech=false` → silence 持续 > 0.5s → 每 tick `was_silent` 恒真 → 反复 `finish+reset`，`reset` 清空 `token_ids` 冲掉首字音头 | 确凿 |
| 3.2 | 首字缺失 | **paraformer** | `mask_alphas_selective` 把首 chunk 的 left 5 帧 + right 3 帧 alpha 置 0；但首 chunk 的 left 帧（`feat_cache` 初始全 0 = padding）与 right 帧（alpha 集中在 right 3 帧）恰恰承载首字能量 → mask 后累积不到阈值 → fired=0 → 首字丢失 | 确凿（e2e） |
| 3.3 | 启动「嗯」 | **paraformer** | #3.2 修复后首 chunk 不 mask，副作用：首 chunk 是 ~0.6s 启动噪声（用户尚未说话），`alpha_sum≈1.3` 误 fire 出「嗯」，被首次静音 flush commit 成首段。**非**真首字近音，是启动噪声的 spurious fire | 确凿（e2e） |
| 3.4 | 停顿后丢字 | **paraformer** | 停顿 flush 后下一句首 chunk **非 `is_first`**（`num_processed_frames > 0`）→ `mask_left=true` 砍新句音头。会话首句首字（#3.2）已修，但段间首字仍丢 | 确凿（e2e） |
| 3.5 | 尾字/中段重复 | **paraformer** | **跨边界**：CIF 在音节跨 chunk 时相邻两 chunk 各 fire 一次同一 token（如「别」跨 chunk → 「识别别」）。**chunk 内**：高 alpha（~2.0）单 chunk 积分两次过阈值 → decoder 两次输出同 token（「语语」「开开」），模型层偏差，mask 无关 | 跨边界确凿；chunk 内为模型层 |

paraformer 的 `was_silent` 只插逗号、**不** `finish+reset`（`streaming_paraformer.rs` 流式不 reset），故不复用 zipformer 的首字症结。

## 4. 修复方案与设计决策

### 4.1 zipformer 首字（#3.1）— `fc2d387`

把「本轮是否有语音」传进 engine，让 `finish+reset` 只在**持续静音**（真·段边界）触发，排除「静音→语音过渡」tick：

- `step_silence` / `detect_silence_gap` 额外返回 `has_speech` → `(was_silent_for_punct, should_flush, has_speech)`。
- `StreamingEngine::accept_samples(samples, was_silent, has_speech)` trait 加 `has_speech` 参数（所有实现同步：local `StreamingSession`、`streaming_runner` 的 `FakeStreamingEngine`、server `pipeline.rs` 的 `FakeEngine`）。
- `streaming_engine.rs` ZipformerCtc / ZipformerTransducer 分支条件改 `if was_silent && !has_speech { finish+reset }`；Paraformer 分支忽略 `has_speech`（标点逻辑不变）。

### 4.2 paraformer 首字（#3.2）— mask 策略迭代

`process_chunk_at` 的 mask 决策，经三轮 e2e 迭代收敛：

| commit | 改动 | 效果 |
|---|---|---|
| `a9795ff` | 首 chunk 也置零 left → 改为**首 chunk 不 mask left** | frame0 fired=0→1，首字 fire；但 right 仍 mask |
| `2dae4c8` | 首 chunk 也**不 mask right** | 首字能量保住；但关了**所有** chunk 的 right → 中段退化（中段 right 是 overlap 边界帧，不 mask → fired 增多 → 叠字/错字涨） |
| `7d66887` | `mask_right = !(is_first \|\| is_final)`，**仅中段** mask right | 首字改善保留 + 中段质量回稳 |

**最终 mask 策略**（`process_chunk_at`）：

```rust
let is_first_chunk = self.num_processed_frames == 0;
let fresh = self.fresh_segment;          // 见 §4.4
let mask_left  = !(is_first_chunk || fresh);
let mask_right = !is_first_chunk && !is_final;
```

- **mask_left**：首 chunk 与 fresh 段首 chunk 关（保音头）；中段/final 开（去上 chunk overlap）。
- **mask_right**：仅中段开（去下 chunk overlap 边界帧，acoustic 不准）；首/final 关（保首字能量 + 尾音 fire）。

### 4.3 paraformer 启动「嗯」（#3.3）— `seen_speech` 门控 `144812c`

`StreamingRunner` 加 `seen_speech: bool` 锁存：

- VAD 在场时，首个 `has_speech` tick 锁存 `seen_speech=true`；**未锁存前不喂 engine**（丢弃启动噪声）。
- 首个 `has_speech` tick 整体喂入（含该 tick 内开头静音），故**不丢真实首字音头**；与 #4.2 配合：首 speech chunk → `is_first=true` → 真首字 fire。
- VAD=None（无 silero 模型）**不门控**，退回原行为喂全部，兼容测试 / 模型缺失环境。
- `finish_with_tail` 同步门控：纯噪声会话（`seen_speech=false`）不喂 tail → finish 返回空。
- `reset()` 清零。

### 4.4 paraformer 停顿后丢字（#3.4）— `fresh_segment` `a0464dd`

`StreamingParaformer` 加 `fresh_segment: bool`：

- `flush()` 末尾置 `fresh_segment=true`。**关键安全性**：flush 用零 padding 收尾，结束后 `feat_cache` 已被冲成静音（非上段语音尾巴）→ 新段首 chunk 不 mask left **安全**——静音 alpha≈0 不会重 fire 上段尾，却保住新句音头。
- `process_chunk_at` 对 `fresh` 段首 chunk `mask_left=false`（即 §4.2 的 `mask_left = !(is_first || fresh)`）。
- **锁存语义**：`fresh_segment` 锁存到**新段首个 fire 的 chunk** 才清（若首 chunk 静音没 fire，`num_tokens=0`，保留 `fresh` 给下个 chunk），确保音头不错过；fire 后 `self.fresh_segment = false` 恢复正常 mask。
- `flush()` 开头先清 `fresh_segment=false`，避免上段 unconsumed 的 fresh 误 mask 当前段尾 chunk。
- `reset()` 清零。

### 4.5 paraformer 跨边界重复（#3.5 跨边界）— token 层去重 `4ca57bb`

`process_chunk_at` step 8 累积 token 时跨边界去重：

```rust
if !seen_first_valid && (tid as i64) == self.last_emitted_token {
    seen_first_valid = true;
    continue;   // 本 chunk 首个有效 token == 上 chunk 末 token → CIF 双 fire，跳过
}
```

- 命中条件：本 chunk **首个有效** token == 上 chunk **末** token（CIF 双 fire 的特征）。
- **不影响**单 chunk 内合法重复（「爸爸」「常常」：两相同字在同一 chunk fire，不跨边界）。

## 5. 已知限制（接受，不进一步修）

| 现象 | 根因 | 决策 |
|---|---|---|
| chunk 边界中段音节偶发丢失（「始」） | 分块 CIF 固有限制：音节横跨 chunk 边界时被切 | 接受（彻底治需 flush 后全量 reset，风险大，违背「丢字优先已解」的现状） |
| chunk 内 CIF 双 fire 叠字（「语语」「开开」） | 模型层 alpha 偏差，单 chunk 积分两次过阈值 | 交 LLM 润色后处理；代码层文本去重有「爸爸」误杀风险，不做 |

## 6. 验证

- **单测**：`cargo test -p octopus-asr-local`——`streaming_runner`（10 例，含 `push_samples_gates_silence_until_speech_when_vad_present`）、`streaming_paraformer`（84 例，含真实模型 flush→accept 路径）全绿。
- **e2e**（paraformer，连说 6 句「开始语音识别」）：首字「开」稳定保留，启动「嗯」消失，停顿后段间首字不再丢；剩余「开开」/偶发「始」丢属 §5 已知限制。final 形如 `开始语音了识别，开语语音识别，开始语音识别，开开语音识别，开始语音识别，`（2/5 完全正确，其余可 LLM 兜底）。
- **回归**：长静音分段、停顿标点（逗号）行为不变；zipformer 首字（`was_silent && !has_speech`）逻辑正确。

## 7. 涉及 commit

| commit | 内容 |
|---|---|
| `fc2d387` | zipformer 首字：`has_speech` 区分段边界与开口过渡 |
| `08b190f` / `0d452cd` | 加 `[asr-diag]` 流式 token 诊断日志（诊断期） |
| `4ca57bb` | paraformer 跨 chunk 边界 token 去重 |
| `5df19fc` | paraformer 文本层重复哨兵（验证用，诊断期） |
| `a9795ff` / `2dae4c8` / `7d66887` | paraformer mask 策略迭代收敛 |
| `144812c` | paraformer 启动「嗯」：`seen_speech` 开口前门控 |
| `a0464dd` | paraformer 停顿后丢字：`fresh_segment` |
