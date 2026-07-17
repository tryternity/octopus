# paddle-ocr NEON Port Backlog

> 2026-07-17 · 性能审查报告 P1-7 记录
>
> **状态**：backlog（未实施）—— 需 ARM SIMD 经验 + 充分 OCR 回归验证

## 1. 问题

`crates/paddle-ocr/` 在多个 det 流水线函数用 `#[cfg(target_arch="x86_64")]` gate AVX2 实现，`not(x86_64)` 走 scalar fallback，**全仓无 NEON 分支**：

| 文件 | 函数 | 影响 |
|---|---|---|
| `det/preprocess.rs` | `write_normalized_row_avx2`（275-332） | OCR 预处理归一化，>=512×512 时 rayon 行并行 |
| `det/postprocess/box_score.rs` | `sum_f32_slice_avx2`（219-256） | box_score 求和 |
| `det/postprocess/contour.rs` | `dilate_row_2x2_avx2`（293-392） | mask dilate 热路径 |
| `det/postprocess/threshold.rs` | 多个 AVX2 + SSE4.1 双路（48/77/99/121/152） | 阈值后处理 |

## 2. 影响

- **主目标平台**：`aarch64-apple-darwin`（Apple Silicon，项目主 binary `octopus-desktop` 跑托盘 app）
- **AVX2 完全失效**：所有 SIMD 函数退化为 scalar，仅 rayon 行并行兜底
- **预热后每次 OCR 都跑**：模型加载是 lazy（idle 60s 释放），但一旦识别就走全套 det 流水线
- **预估占比**：OCR 推理主导（ONNX Runtime 内部已优化），预处理/后处理估占 10-30%（需 profiling 确认）

## 3. 为何进 backlog（不在性能 batch 修）

1. **工作量巨大**：4 个文件多个函数，每个都要手写 `std::arch::aarch64` NEON intrinsics（`vdupq_n_f32` / `vmlaq_f32` / `vmaxq_f32` 等）替代 AVX2（`_mm256_set1_ps` / `_mm256_add_ps` / `_mm256_max_ps`）
2. **需要 ARM SIMD 经验**：NEON 与 AVX2 lane 数（4 vs 8）、掩码处理、tail handling 差异
3. **必须 OCR 回归测试**：det 流水线精度敏感，NEON 实现的舍入差异可能影响 box 检测，需对比真实图片的 OCR 结果
4. **无法静态判断收益**：10-30% 预处理占比是估算，需先 profile 确认是瓶颈再投入

## 4. 实施方向（供后续接手）

### 4.1 前置：profile 确认瓶颈

在 Apple Silicon 上用 `cargo bench` 或 instruments 测一次完整 OCR，确认 det 预处理/后处理的真实占比。若 <5% 不值得动手。

### 4.2 NEON 实现策略

每个 AVX2 函数对照实现 NEON 版本：

```rust
#[cfg(target_arch="aarch64")]
#[target_feature(enable="neon")]
unsafe fn write_normalized_row_neon(...) {
    use std::arch::aarch64::*;
    // 4 lane（vs AVX2 8 lane），循环展开 2 倍补偿
    // ...
}
```

运行时分发：

```rust
#[cfg(target_arch="x86_64")]
let use_avx2 = std::arch::is_x86_feature_detected!("avx2");
#[cfg(target_arch="aarch64")]
let use_neon = std::arch::is_aarch64_feature_detected!("neon"); // Apple Silicon 总是 true

if cfg!(target_arch="x86_64") && use_avx2 {
    unsafe { write_normalized_row_avx2(...) }
} else if cfg!(target_arch="aarch64") && use_neon {
    unsafe { write_normalized_row_neon(...) }
} else {
    write_normalized_row_scalar(...)
}
```

### 4.3 验证

每个函数加对照测试：scalar vs NEON 输出位一致（或允许 ε 浮点误差）。完整 OCR e2e 测试用真实截图比对识别结果。

## 5. 备选方案

若 NEON port 投入产出比不划算：
- **广度优先 vs 深度优先**：只 port 最热的 1-2 个函数（如 `dilate_row_2x2`），不全 port
- **依赖 ONNX Runtime 加速**：让 ORT 跑 CoreML EP（已在 `55f86ee5` benchmark 里验证），det 后处理用图像库的 SIMD 实现（image crate 的 `imageproc` 有 NEON 优化）
- **完全去掉 SIMD gate**：纯 scalar + rayon——简化代码，性能靠多核兜底。劣势是单线程场景退化

## 6. 引用

- 性能审查报告 P1-7（2026-07-17 性能 batch 第二轮）
- 类似优化范式：whisper.rs 的 WHISPER_CACHE_NAMES / WHISPER_PRESENT_NAMES（leak &'static str 优化）——但那是键名预算，非 SIMD port，仅供参考方法论
