# 代码优化 spec：asr 热路径分配 + desktop Mutex + normalize 去重

> **Status: ✅ 已完成**（2026-07-29，Phase 0/A/B/C'/D 全部完成。数值由 golden-value 测试守护，全 workspace 测试 0 failed）
>
> **背景**：rust-patterns skill 全工程扫描发现 3 类可优化项。本 spec 聚焦**有测试保护、风险可控、收益明确**的改动。结构性大改（vault sync merge 双胞胎、超大函数拆分、引擎模板抽象）已排除，留后续独立 plan。

## 1. 范围

| 项 | crate | 收益 | 风险 |
|---|---|---|---|
| **A. zipformer 热路径堆分配** | asr-local | 性能：帧循环内每帧 2-3 次 vec! → 提循环外/栈数组（30s 音频省 ~900 次堆分配） | 中（数值敏感，需 golden 测试守护） |
| **B. desktop Mutex 换 parking_lot** | desktop | 消除 ~16 处 `.lock().unwrap()`，无 poison | 低（机械改动，guard 不跨 await） |
| **C'. normalize_whisper_features 合并** | asr-local | 性能：zipformer 版 3 趟嵌套 `[[i,j]]` 索引 → slice 快路径单遍 | 中（数值敏感，需 golden 测试守护） |

**排除项**（调研后发现风险/复杂度过高，留后续）：
- fbank 去重（paraformer 副本 → canonical）：paraformer filterbank `high_freq=7600` vs canonical `8000`，直接合并会**静默改变数值**；且 fbank.rs 零单测。需先建 filterbank 参数化 + golden 测试基础设施。
- zipformer fbank 变体并入 canonical：帧居中 + 反射 padding + FLT_EPSILON log，语义差异大，不能直接复用。

## 2. 设计

### 2.1 项 A：zipformer 热路径堆分配

**问题**：`zipformer.rs` 两个特征提取函数在 `for fi in 0..n_frames` 帧循环内逐帧 `vec!` 分配 buffer：
- `compute_whisper_features_linear`（~1090 行）：每帧 `vec![0.0f32; 400]`（frame）+ `vec![Complex; 400]`（FFT buf）= 2 次堆分配
- `compute_fbank_features`（~1184 行）：每帧 `vec![0.0f32; 400]`（frame）+ `vec![0.0f32; 400]`（preemph）+ `vec![Complex; 512]`（FFT buf）= 3 次堆分配

`compute_fbank_features` 还是流式热路径（streaming_zipformer 每 `accept_samples` 调用），分配放大严重。

**对照范式**（codebase 已有的正确写法）：
- `fbank.rs:65-73`：`buf`（FFT 缓冲）提循环外复用 + `frame_buf` 用栈数组 `[0.0f32; FBANK_FRAME_LEN]`
- `paraformer.rs:461-469`：同上 + preemph **in-place** 在 frame_buf 上做（无独立 preemph buffer）
- `streaming_paraformer.rs:306-312`：同上

**重构方案**：
- `frame`（`vec![0.0f32; 400]`）→ 提循环外栈数组 `[0.0f32; Z_FRAME_LEN]`
- FFT `buf`（`vec![Complex; N]`）→ 提循环外复用（保留 Vec，因要 `&mut` 传 `fft.process`）
- `compute_fbank_features` 的 `preemph` buffer → 删除，改 in-place 在 frame_buf 上做（反向遍历，对照 paraformer.rs:491-500）

**不变量**：
- 反射 padding 逻辑（`compute_whisper_features_linear`）不变
- 帧居中 + 反射 padding + povey 窗 + FLT_EPSILON log 偏置（`compute_fbank_features`）不变
- 函数签名不变（`&[f32] -> Result<Array2<f32>>`），所有调用方零改动
- **数值完全一致**（golden 测试守护）

### 2.2 项 B：desktop Mutex 换 parking_lot

**问题**：`action_bar_commands.rs` 用 `std::sync::Mutex`，导致 12 处生产代码 + 4 处测试代码 `.lock().unwrap()`。同 crate 其他文件（tray.rs / record_annotation_window.rs / compact_editor_commands.rs）已用 `parking_lot::Mutex`（无 poison，`lock()` 直接返回 guard）。

**涉及 static**：
- `PENDING_CONTEXT: Mutex<Option<ActionBarContext>>`（行 42）— 9 处 `.lock().unwrap()`
- `TRANSLATE_RESULTS: once_cell::sync::Lazy<Mutex<HashMap<...>>>`（行 1084）— 3 处
- `TRIGGER_TEST_LOCK: Mutex<()>`（行 2094，测试内）— 4 处

**对照范式**：
- `tray.rs:34-36`：`once_cell::sync::Lazy<parking_lot::Mutex<...>>`（与 TRANSLATE_RESULTS 同构）
- `record_annotation_window.rs:152`：简单 static + `parking_lot::Mutex`

**重构方案**：
- `use std::sync::Mutex` → `use parking_lot::Mutex`
- 16 处 `.lock().unwrap()` → `.lock()`

**安全性已核实**（调研结论）：
- 所有 `.lock()` 在同步上下文或 `std::thread::spawn` worker 内
- 唯一 async 函数 `execute_action_bar_inner` 的锁（行 1715）是临时表达式，guard 在 await 之前 drop，**不跨 await**
- 无 guard 存结构体字段 / 返回 guard / 跨函数边界
- parking_lot 依赖已在 desktop Cargo.toml（`parking_lot = { workspace = true }`，0.12.5）

**正向收益**：parking_lot 无 poison——原 panic 后 `.unwrap()` 会二次 panic 传播，新代码不会。

### 2.3 项 C'：normalize_whisper_features 合并

**问题**：两份实现，公式相同（`log10 → clamp max-8 → (x+4)/4`，对齐 sherpa-onnx `NormalizeWhisperFeatures`），但内存访问方式不同：
- `qwen3_asr.rs:657`（快路径）：`as_slice_mut()` 拿连续扁平切片 + 单层 `iter_mut()`，log10 与 find-max 合并到一遍
- `zipformer.rs:1150`（慢路径）：3 趟嵌套 `for i..for j.. chunk[[i,j]]` 索引，无 slice 快路径

zipformer 版是 `pub(crate)`，被流式引擎 **per-chunk** 调用（streaming_zipformer.rs:184/278/641/724），每个 chunk 都跑 3 遍嵌套循环——重构价值高。

**重构方案**：
- 把 qwen3 版（带 `as_slice_mut()` 快路径 + `mapv_inplace` fallback）提升为共享函数，放到 `feature.rs`（特征处理共享模块），`pub(crate)`
- qwen3_asr.rs 删私有 fn，调共享版
- zipformer.rs 删慢路径版，调共享版
- 调用点不变（zipformer.rs:525/874、streaming_zipformer.rs:184/278/641/724）

**不变量**：公式不变（`(clamp(x, max-8) + 4) / 4`），仅内存访问方式优化。

## 3. TDD 策略

**核心原则**：数值敏感的重构（A、C'）必须先补 golden-value 数值快照测试（regression pin），钉住重构前的当前行为。

**当前测试缺口**：
- zipformer.rs 的 `compute_whisper_features_linear` / `compute_fbank_features` / `normalize_whisper_features` **零独立单元测试**
- 唯一走全链路的 `test_zipformer_ctc_offline_debug`（1330 行）依赖 HF snapshot + **全程 println、零 assert**——数值漂移不会失败
- 全仓无「给定输入 → 固定期望输出」的 golden-value 断言

**Phase 0 补的测试**（不依赖 HF 模型，纯数学，`cargo test` 默认跑）：
- 固定小输入（硬编码 `[f32; N]`）→ 断言输出 `Array2` 前若干帧 × 若干 bin 与 golden 期望 `approx_eq`（容差 1e-5）
- golden 值由**重构前的当前实现生成**（先跑一次打印，填入测试）——这是 regression pin
- 覆盖：whisper 路径（反射填充 + log10-mel + normalize）、fbank 路径（DC removal + preemph + povey 窗）、normalize 公式

## 4. 降级路径

- **A/C' 数值偏差**：golden 测试会立即抓住。若重构后 golden 测试失败且无法对齐，回退该 Task（buffer 管理/normalize 恢复原状）。
- **B**：无降级需要（机械改动，parking_lot 行为严格优于 std::sync::Mutex）。
