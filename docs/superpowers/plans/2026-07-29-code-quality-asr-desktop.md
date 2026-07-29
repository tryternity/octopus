# 代码优化 plan：asr 热路径分配 + desktop Mutex + normalize 去重

> **Status: ✅ 已完成**（2026-07-29，Phase 0/A/B/C'/D 全部完成。验证：asr-local 146 + desktop 438 + 全 workspace 测试 0 failed，clippy 无新 warning）
>
> **Spec**: [`2026-07-29-code-quality-asr-desktop.md`](../specs/2026-07-29-code-quality-asr-desktop.md)

## Phase 0：TDD 前置——补 golden-value 数值快照测试（regression pin）

### Task 0.1: zipformer 3 函数 + normalize golden 测试

**Files:** `crates/asr-local/src/zipformer.rs`（`#[cfg(test)] mod tests` 内）

- [x] **Step 1: 给 `compute_whisper_features_linear` 补 golden 测试**
  - 固定小输入（硬编码 `[f32; 1600]`，~10 帧），调函数拿 `Array2<f32>`
  - 先 `println!` 打印输出前 3 帧 × 前 5 bin，跑一次拿 golden 值
  - 断言 `assert_eq!`/`approx_eq`（容差 1e-5）钉住 golden
- [x] **Step 2: 给 `compute_fbank_features`（zipformer 版）补 golden 测试**
  - 同上，覆盖 DC removal + preemph + povey 窗 + 帧居中 + 反射 padding
- [x] **Step 3: 给 `normalize_whisper_features` 补 golden 测试**
  - 构造固定 `Array2`（如 2×3 已知值），调归一化，断言输出数值
- [x] **Step 4: 验证 golden 测试通过**（`cargo test -p octopus-asr-local --lib zipformer::tests`，重构前的 baseline）

## Phase A：zipformer 热路径堆分配优化

### Task A.1: compute_whisper_features_linear buffer 优化

**Files:** `crates/asr-local/src/zipformer.rs:~1090`

- [x] **Step 1: `frame` 提循环外栈数组**（`vec![0.0f32; 400]` → `[0.0f32; Z_FRAME_LEN]`）
- [x] **Step 2: FFT `buf` 提循环外复用**（`vec![Complex; 400]` 提到 for 前）
- [x] **Step 3: 保留反射填充逻辑不变**
- [x] **Step 4: golden 测试通过**（数值一致）

### Task A.2: compute_fbank_features buffer 优化

**Files:** `crates/asr-local/src/zipformer.rs:~1184`

- [x] **Step 1: `frame` 提循环外栈数组**
- [x] **Step 2: `preemph` 删除，改 in-place 在 frame_buf 反向遍历**（对照 paraformer.rs:491-500）
- [x] **Step 3: FFT `buf` 提循环外复用**
- [x] **Step 4: 保留帧居中 + 反射 padding + povey 窗 + FLT_EPSILON log 不变**
- [x] **Step 5: golden 测试通过**（数值一致）

### Task A.3: 全量回归

- [x] **Step 1: `cargo test -p octopus-asr-local --lib` 全过**（golden + 现有测试无回归）

## Phase B：desktop action_bar_commands Mutex 换 parking_lot

### Task B.1: PENDING_CONTEXT 换 parking_lot

**Files:** `crates/desktop/src/action_bar_commands.rs`

- [x] **Step 1: `use std::sync::Mutex` → `use parking_lot::Mutex`**（行 4）
- [x] **Step 2: `PENDING_CONTEXT` static 类型改 parking_lot::Mutex**（行 42）
- [x] **Step 3: 9 处 `.lock().unwrap()` → `.lock()`**（行 310/330/334/338/448/471/485/697/1715/2015）

### Task B.2: TRANSLATE_RESULTS 换 parking_lot

- [x] **Step 1: static 类型 `once_cell::sync::Lazy<Mutex<...>>` 内层换 parking_lot::Mutex**（行 1084）
- [x] **Step 2: 3 处 `.lock().unwrap()` → `.lock()`**（行 1090/1119/1133）

### Task B.3: 测试锁一并换

- [x] **Step 1: `TRIGGER_TEST_LOCK` 换 parking_lot::Mutex**（行 2091/2094）
- [x] **Step 2: 4 处 `.lock().unwrap()` → `.lock()`**（行 2405/2413/2427/2441）

### Task B.4: 验证

- [x] **Step 1: `cargo build -p octopus-desktop --features embedded` 0 error 0 warning**
- [x] **Step 2: `cargo test -p octopus-desktop` 438 测试全过**

## Phase C'：normalize_whisper_features 合并

### Task C.1: 提升到 feature.rs 共享函数

**Files:** `crates/asr-local/src/feature.rs` + `qwen3_asr.rs`

- [x] **Step 1: feature.rs 加 `pub(crate) fn normalize_whisper_features`**（带 `as_slice_mut()` 快路径 + fallback，搬自 qwen3_asr.rs:657）
- [x] **Step 2: qwen3_asr.rs 删私有 fn，改调 `crate::feature::normalize_whisper_features`**

### Task C.2: zipformer 改调共享版

**Files:** `crates/asr-local/src/zipformer.rs` + `streaming_zipformer.rs`

- [x] **Step 1: zipformer.rs 删慢路径 `normalize_whisper_features`（~1150）**
- [x] **Step 2: zipformer.rs 改调 `crate::feature::normalize_whisper_features`**（行 525/874）
- [x] **Step 3: streaming_zipformer.rs 的 `use crate::zipformer::normalize_whisper_features` 改 `use crate::feature::normalize_whisper_features`**（行 184/278/641/724 调用点不变）

### Task C.3: 验证

- [x] **Step 1: golden 测试通过**（normalize 数值一致）
- [x] **Step 2: `cargo test -p octopus-asr-local --lib` 全过**

## Phase D：全量验证 + 文档同步

- [x] **Step 1: `cargo clippy --workspace`（warning 不增）**
- [x] **Step 2: `cargo test`（核心层）+ `cargo test -p octopus-asr-local --lib` + `cargo test -p octopus-desktop`**
- [x] **Step 3: 更新 `docs/architecture.md` ASR 后处理链段（normalize 合并 + buffer 优化注记）**
- [x] **Step 4: review plan（回填偏差）**
