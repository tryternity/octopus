# VadSegmentedPipeline detect_vad LSTM 跨段漂移修复（SenseVoice "几段后不吐字"）

## Context

SenseVoice 引擎录音几段后偶发「再说话不吐字」，重启录音恢复。复现靠运气（依赖 LSTM 漂移累积，log 加点难稳定复现）。

### 根因

`VadSegmentedPipeline.detect_vad`（Silero LSTM，`h`/`c` 有状态）在一个录音会话内**跨段累积、从不 reset**（`reset()` 是 `#[allow(dead_code)]`，coordinator stage 切换时 drop pipeline 而非 reset）：

1. 几段后 LSTM 漂移 → 对真实语音持续 `prob < 0.5`
2. `compute_speech_chunks` 返回 `< 2` → `has_speech` 卡 `false`（run_tick L438 不再置真）
3. `silence_cut`（`has_speech && ...`）和 `force_cut`（`has_speech && ...`）**都因 `&& has_speech` 永不触发**
4. `audio_buffer` 无限增长、永不切段 → 不吐字

重启 = drop pipeline + `new`（`detect_vad` h/c=0 + preroll）→ 恢复。

> coordinator drop 重建已确认：stop / discard / cancel / finalize 四路径全 drop、无 reset 复用。修复纯在 pipeline.rs 内，不动 coordinator。

## 方案（A 治本 + B 兜底）

### A. 切段后 reset+preroll detect_vad（治本）
切段清理后插入 `detect_vad.reset() + vad_preroll(&mut self.detect_vad)`。切段点天然是 LSTM 安全重置点（段已切完、下段从零开始），消除跨段漂移。preroll 与构造时（L371）对称，防段首丢字。

### B. force_cut 解绑 has_speech（兜底）
`force_cut` 去掉 `&& has_speech`。达上限必切，由 `filter_vad`（每段 reset、不受漂移污染）独立兜底判定有无语音：检出则 spawn（双 VAD 保险），未检出则不 spawn 但 buffer 已清（防内存堆积）。

`silence_cut` 保持 `&& has_speech`（原意：检测到停顿才切）。

## 改动

仅 `crates/desktop/src/pipeline.rs` `run_tick`（L428-482）：
- L447 `let force_cut = buffer_duration_s >= SEGMENT_DURATION_S;`（去 `&& has_speech`）
- 切段清理后（audio_buffer.clear / has_speech=false / silence_duration=0）插入 `detect_vad.reset() + vad_preroll(&mut self.detect_vad)`

## 测试

- [x] 新增 `force_cut_clears_buffer_when_no_speech_detected`（try_new skip 模式，模型缺失则 skip、CI 友好）：灌 `SEGMENT_DURATION_S`(20s) 纯静音 → detect_vad 已 preroll 静音稳态判无语音 → has_speech 保持 false → force_cut 触发 → 断言 `audio_buffer.is_empty()`。本地模型存在时 0.22s 真跑通过（B 防堆积验证）。
- A（reset+preroll）依赖真实 LSTM 漂移，难单测，靠 e2e 长会话 + `[vad-seg]` 日志验证。

## 验证

- [x] `cargo test -p octopus-desktop 'pipeline::'` —— 17 passed（含新测试，现有无回归）
- [ ] e2e：长会话连续说多段，观察「几段后不吐字」是否消失（依赖漂移复现，靠运气）

## 风险

- B 改变纯静音会话行为：达上限触发一次 force_cut，filter_vad 判无语音则不 spawn、buffer 清空（无害，只防堆积）。
- B 理论乱码风险：detect_vad 失灵 + filter_vad 误判有语音 → spawn 噪声段。但 A 修复后 detect_vad 不漂移，B 仅兜底；乱码优于「完全不吐字 + 内存爆涨」。
