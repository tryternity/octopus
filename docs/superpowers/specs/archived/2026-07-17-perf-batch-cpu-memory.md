# 2026-07-17 性能优化批次（CPU / 内存）

> 2026-07-17 · 多轮性能审查后的集中优化
>
> **状态**：已实现（P0/P1/P2/P3 共 4 个批次，本 spec 覆盖核心改动）

## 1. 背景

基于 4 个并行审查 agent 的两轮报告（共 60+ 条发现），按 "Measure First → 核实 → 修 → 验证" 流程逐项处理。本 spec 集中记录所有已实施的优化项，与既有 perf specs 互补：

- [`2026-07-17-perf-release-lto.md`](./2026-07-17-perf-release-lto.md)：release profile LTO/strip（测量基础设施）
- [`2026-07-17-perf-ort-bench.md`](./2026-07-17-perf-ort-bench.md)：streaming paraformer ort 推理 benchmark
- [`2026-07-17-paddle-ocr-neon-port-backlog.md`](./2026-07-17-paddle-ocr-neon-port-backlog.md)：paddle-ocr NEON port（未实施，backlog）

## 2. P0 — 内存峰值

### 2.1 Stitcher canvas 链式 clone（滚动截图 stop 路径）

**问题**：`screenshot_commands.rs` stop 路径调 3 次 `stitcher.canvas().clone()`，每次复制整张画布（1920×5000 RGBA ≈ 38MB），3 次 ≈ 114MB 峰值。`canvas()` 内部 `canvas_buf.clone()` 重建 `RgbaImage` 缓存。

**修复**：`Stitcher::into_canvas(self) -> RgbaImage` 消费式 API——优先 move `canvas_cache`，否则从 `canvas_buf` 重建一次（`std::mem::take` 无 clone）。主路径改用 `into_canvas` 消费 stitcher，3 次 clone → 1 次必要 clone（PNG 编码块作用域）+ 1 次 move。

**文件**：`crates/capx/src/stitch.rs`、`crates/desktop/src/screenshot_commands.rs`

### 2.2 ALL_CAPTURES 初始化时双 clone

**问题**：截图初始化循环 `captures.iter()` 借用，每个 capture 的 `rgba_bytes` 被 clone 两次（一次给 JPEG 编码、一次入 ALL_CAPTURES），双屏 4K+1080p ≈ 82MB/屏。

**修复**：`captures` 改 `mut` + `iter_mut`，`std::mem::take` 取走 `rgba_bytes`——第一次 clone 给 JPEG 编码（用完丢弃），原始数据 move 入 ALL_CAPTURES。两次大 clone → 1 次。

**核心诉求"延迟重截 / 只保留选中屏"不可行**：用户框选时需看到全屏背景画面，重截会丢背景（光标/动画已变）；多屏间可跨屏框选无法预知。ALL_CAPTURES 常驻是截图功能固有需求。

**文件**：`crates/desktop/src/screenshot_commands.rs`

### 2.3 流式 paraformer raw_samples + fbank_cache 无界增长（**未实施**）

**问题**：单会话内 `raw_samples` 不 drain（绝对帧索引设计），1 小时会议录音 ≈ 345MB。

**未实施原因**：涉及 AGENTS.md 警告的 ASR 不变量区域，drain 需重设计帧索引体系 + 真实音频回归测试，不在性能 batch 范围。

## 3. P1 — CPU 热路径

### 3.1 滚动预览 PNG → JPEG（P1-4）

**问题**：滚动录制每帧 spawn_blocking 内做双编码——实时画面 JPEG + 预览 PNG。预览 PNG 1-2MB 编码慢，仅视觉反馈不入库。

**修复**：预览改 JPEG（100-300KB，编码快 3-5×，肉眼无差）。前端 `ScrollPreview.tsx` 同步 mime `image/png` → `image/jpeg`。

**文件**：`crates/desktop/src/screenshot_commands.rs`、`crates/desktop/frontend/src/pages/Screenshot/ScrollPreview.tsx`

### 3.2 Sobel 特征图自写 + Welford 单 pass（P1-5）

**问题**：`to_feature_map` 走 imageproc `sobel_gradients` 内部 `filter3x3` 三次分配（i16 horizontal + i16 vertical + u16 output）+ `GrayBuf.to_gray_image` 一次 clone + max 全扫 + mean_stddev 双遍历 sum + from_fn 第三次分配。每帧调 2 次 = 8 次扫描 + 8 次分配。

**修复**：自写 Sobel——直接索引 `GrayBuf.data` 读 3×3 邻域（跳过 to_gray_image clone），kernel + border clamp 与 imageproc 0.25.1 完全一致，单 pass 算 Sobel + max + Welford 在线均值方差（数值稳定），单 pass 归一化输出。4 pass + 4 次分配 → 2 pass + 1 个 Vec<u16>。

**行为不变性**：4 个单测对比自写 Sobel vs imageproc 原实现（reference_feature_map 在测试内调 imageproc）——常数图/梯度图/真实模拟/极小图全部像素级一致（max_diff ≤ 1，浮点 Welford vs 两遍 sum 精度差异）。

**文件**：`crates/capx/src/stitch.rs`

### 3.3 whisper 输出侧 present.* 键名预算（P1-6）

**问题**：`update_decoder_kv` 每 token step 每层 `format!("present.{}.decoder.key/value", layer) × 2`——base 模型 6 层 × 100 token = 1200 次堆分配。输入侧已用 `WHISPER_CACHE_NAMES` leak 优化，输出侧漏。

**修复**：加 `WHISPER_PRESENT_NAMES` 全局 `Lazy` leak `&'static str`（同 `WHISPER_CACHE_NAMES` 范式），热路径直接索引。1200 次 format! → 0。`to_vec()` 保留——ORT 张量生命周期跟随 SessionOutputs，跨 token step 存 KV 必须 owned。

**对照**：moonshine.rs（`ort::value::Value::view` 零拷贝）、qwen3_asr.rs（`ArrayView4` + delta 写回）已零拷贝，whisper 现已对齐键名部分（to_vec 仍保留）。

**文件**：`crates/asr-local/src/whisper.rs`

### 3.4 clipboard dock 状态内存缓存（P1-8）

**问题**：`window_position::load_dock_state` 每次走 DB SELECT，clipboard_window 的 Moved 事件拖拽期间 ~60Hz 触发 → 拖一次产生数百次 DB round-trip。

**修复**：加 `DOCK_CACHE: Lazy<Mutex<HashMap<String, String>>>` 内存镜像——`save_dock_state` 同步更新缓存 + DB，`load_dock_state` 优先读缓存（首次或未命中回退 DB 并填缓存，空值也缓存避免反复查 DB）。

**文件**：`crates/desktop/src/window_position.rs`

### 3.5 search identity_key 跨 batch 缓存（P1-9）

**问题**：`dedup_by_identity` 每次搜索全量 `retain`，`identity_key` 每条 `serde_json::from_str::<Value>` 整解 action_data。流式 `search_streaming` 每 batch 对累积 collected 重跑 → 已解析过的旧结果反复解析。

**修复**：加 `key_cache: HashMap<String, String>`（action_data → identity_key），同 action_data 跨 batch 不重解析。用 owned String key 避免 retain 闭包的 &r 逃逸。

**文件**：`crates/search/src/engine.rs`

### 3.6 paddle-ocr NEON port（**未实施**）

**问题**：paddle-ocr 多个 det 函数 AVX2 gate，主目标 aarch64-apple-darwin 完全退化 scalar。

**未实施原因**：需手写 `std::arch::aarch64` NEON intrinsics（4 个文件多个函数）+ OCR 回归测试，工作量大、需 ARM SIMD 经验、需 profile 确认收益。详见 [`2026-07-17-paddle-ocr-neon-port-backlog.md`](./2026-07-17-paddle-ocr-neon-port-backlog.md)。

## 4. P2 — allocation/CPU 噪音（main 上的 commit `f3a6d61d`）

由他人在 main 上实施，本 spec 仅引用记录。涉及：qwen3_asr/streaming_paraformer/streaming_zipformer/ocr engine 等热路径零风险项。

## 5. P3 — 前端 re-render（main 上的 commit `0371dc12`）

由他人在 main 上实施。涉及：ImagePreview/Screenshot 前端组件。

## 6. 早期批次（同会话）

本次会话早期还做了以下性能修复，散落在 daily-bug-fix-actionbar-launch 分支的多个 commit：

- **cpal 回调 extend 替代 collect**（`audio.rs`）：录音热路径每帧省一次 Vec<f32> 分配
- **SystemStatusSampler 订阅计数门控**（`system_status_commands.rs`）：闲置时不采样 sysinfo
- **高频事件 emit_to 定向**（`result_window.rs` / `system_status_commands.rs`）：update-result/show-result/clear-result/system-status 不再全局广播
- **charCount 用 text.length**（前端 CompactEditor）：避免 `[...text]` 每键 O(n) 展开
- **ActionBar resize height 差分门控**（前端 ActionBar）：搜索 batch 时避免重复 setSize
- **click-through poller 双频率状态机**（`result_window.rs` / `clipboard_dock.rs`）：闲置时 200ms 慢检测，仅精简态可见时 33ms 高频

## 7. 不变量

1. **自写 Sobel 与 imageproc 像素级一致**——4 个对照单测钉死（max_diff ≤ 1）
2. **WHISPER_PRESENT_NAMES leak 上限 32 层**——覆盖 whisper 系列（tiny=4 / base=6 / small=12 / medium=24 / large=32）
3. **DOCK_CACHE 一致性**——所有 dock 写入都走 `save_dock_state`，缓存与 DB 同步
4. **search key_cache 生命周期**——单次 dedup 调用内，跨调用重建（无长期残留）
5. **Stitcher::into_canvas 消费 self**——调用后不可再用，主路径在 stop 时调用一次

## 8. 验证

每个 commit 单独验证：
- `cargo build --release`：0 error 0 warning
- `cargo test --bin octopus-desktop`：311 passed
- `cargo test -p octopus-capx`：含 Sobel 对照 4 测试
- `cargo test -p octopus-asr-local --lib`：125 passed
- `cargo test -p octopus-search --lib`：69 passed

## 9. 待办

- **P0-1 paraformer 缓冲无界增长**：需重设计帧索引体系 + 真实音频回归测试
- **P1-7 paddle-ocr NEON port**：手写 ARM SIMD intrinsics + OCR 回归
- **实际运行时验证**：本批改动需重启 octopus 跑新二进制实测 RSS + idle CPU delta
