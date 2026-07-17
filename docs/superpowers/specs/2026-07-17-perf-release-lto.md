# 2026-07-17 Release Profile LTO 改造（z_perf Step 0）

## 背景

z_perf skill Step 0 指出工程 `[profile.release]` 是默认值（`lto`/`codegen-units=1`/`strip` 全注释掉），建议启用作为"性价比最高、几乎零风险"的优化。

本文档记录实际验证结果——**实测推翻了 skill 初版对收益的预期**，并把偏差回写。

## 改动

- `Cargo.toml`（root）：取消注释 `strip = true` / `lto = "fat"` / `codegen-units = 1`
- `crates/asr-local/Cargo.toml`：加 criterion dev-dep + `[[bench]] fbank`
- `crates/asr-local/src/fbank.rs`：
  - `compute_fbank` / `compute_fbank_features` 从 `pub(crate)` 提为 `pub`（bench 可达）
  - re-export `feature::{apply_lfr, hamming_window, povey_window}`（feature 模块仍 `pub(crate)`）
- 新增 `crates/asr-local/benches/fbank.rs`：fbank / fbank_features / apply_lfr 三组基准

## 测量方法（z_perf 护栏：measure before change）

```bash
cargo bench -p octopus-asr-local --bench fbank -- --save-baseline nolto   # 改前
# 改 Cargo.toml 启用 LTO，重编译
cargo bench -p octopus-asr-local --bench fbank -- --baseline nolto         # 改后，criterion 自动对比
```

机器：macOS darwin 25.5.0 arm64。LTO 重编译耗时 2m05s（符合注释 +1~3min 预期）。

## 性能总览

| benchmark | nolto（baseline） | LTO（after） | 变化 | criterion 判定 |
|-----------|-------------------|--------------|------|----------------|
| compute_fbank/1600 | 152.58 µs | 154.37 µs | +1.4% | No change (p=0.23) |
| compute_fbank/16000 | 1.8848 ms | 1.8670 ms | -1.4% | within noise |
| compute_fbank/48000 | 5.6759 ms | 5.6898 ms | +0.2% | No change (p=0.28) |
| compute_fbank_features/1600 | 153.50 µs | 153.03 µs | -0.3% | within noise |
| compute_fbank_features/16000 | 1.8728 ms | 1.8791 ms | -0.1% | No change |
| **apply_lfr_1s** | **2.13 µs** | **1.80 µs** | **-15.6%** | ✅ **Improved** (p=0.00) |

### 二进制体积

| 目标 | nolto | LTO+strip | 变化 |
|------|-------|-----------|------|
| fbank bench 二进制 | 4.8 MB | 2.8 MB | **-42%** |

## 结论：收益与预期的偏差（如实记录）

**z_perf skill 初版（setup.md §3）写的"fbank+ort 1.5-2x"是错误假设**，实测 fbank FFT 几乎无变化（±2%，统计噪声内）。

### 为什么 LTO 对 fbank 几乎无效

1. **rustfft 是高度自优化的单 crate**：fbank 热循环开销几乎全在 `rustfft::Fft::process()` 内部，rustfft 本身已充分优化，跨 crate 边界内联收益微乎其微。
2. **fbank 瓶颈是 FFT 算法本身**（O(N log N) 浮点运算），不是函数调用开销或跨 crate 边界——LTO 触及不到算法层。
3. **apply_lfr 提速 15.6%** 是因为它走 `ndarray` 跨 crate 索引循环，LTO 把 ndarray 边界内联掉了。但绝对值仅 0.33 µs，对 streaming 整体（1.87ms/chunk）影响 <0.02%，可忽略。

### 实际收益（修正后）

| 维度 | 收益 | 评价 |
|------|------|------|
| fbank FFT 性能 | ≈0% | **远低于预期** |
| 二进制体积 | -42% | 显著，对桌面 app 分发有意义 |
| apply_lfr | -15.6% | 真实但绝对值可忽略 |
| ort 推理性能 | **未测**（需独立 benchmark，ort Session 是 FFI 边界，LTO 可能更无效） | 待评估 |
| 链接时间 | +1~3min | 代价，首编慢 |

## 决策

**保留 LTO 改动**。理由：
- 体积 -42% 是确定收益（strip 是纯打包优化，零运行时影响）
- 性能无回归（cargo test 125 全过，含 ASR 正确性）
- 链接时间代价可接受（开发用 debug profile 不受影响）

**但不宣传"性能提升"** —— fbank/ASR 热路径性能收益接近零，不能误导后续优化决策。

## 验证

```bash
# 1. ASR 正确性（AGENTS.md 强制）
cargo test -p octopus-asr-local --lib
# 结果：125 passed; 0 failed; 6 ignored（含 streaming_paraformer_real_model 对照 sherpa-onnx 参考值）

# 2. Benchmark 对比
cargo bench -p octopus-asr-local --bench fbank -- --baseline nolto
# 结果：仅 apply_lfr 显著改善，其余 within noise

# 3. 编译验证
cargo build -p octopus-asr-local --release  # 0 error 0 warning
```

## 后续（真正可能提速 fbank 的方向）

既然 LTO 不是 fbank 的提速点，按 z_perf "fix algorithm first" 原则，后续优化应朝算法/数据结构方向：

1. **mel filterbank 改 flat SoA**：当前 `Vec<Vec<f64>>`（fbank.rs:27），每 bin 单独分配。改 `Vec<f64>` flat + stride 索引，cache locality 显著改善
2. **power_spectrum 转换 f32→f64 提前避免**：fbank.rs:116 用 f64 累加 power，但输入是 f32——若精度允许，全程 f32 省一半带宽
3. **并行帧处理**：n_frames 帧间独立，可用 rayon（paddle-ocr 已用），但需评估线程开销 vs 1.87ms 单核耗时
4. **ort 推理独立 benchmark**：这才是 streaming 真正大头，需建独立 criterion bench（Session::run 耗时）

这些是 z_perf 后续循环的 Step 1-5 目标，不在本次 LTO 改造范围。

## 对 z_perf skill 的回写（强制）

本次实测暴露 skill 初版 setup.md §3 的错误假设。已修正：
- `setup.md` §3：删除"ort 推理 + fbank FFT 通常快 30-100%"，改为如实描述"体积确定收益，性能收益取决于热路径是否跨 crate"
- `rust-hotpaths.md`：fbank 段补充"rustfft 内部已充分优化，LTO 收益小"
