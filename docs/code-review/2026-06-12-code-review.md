# 代码审查报告 — 2026-06-12

> Worktree: `.worktrees/code-review-2026-06-12` @ HEAD `795620d` (main)
> Scope: 全工作区死代码 / 重复代码 / 超长代码
> 工具: `cargo check`、`cargo clippy`、`codegraph` SQL、`rg`

## 总体结论

**健康度：良好。** Rust 编译器 0 warning，clippy 仅 3 个 nit（全在 `paddle-ocr`，都是无关紧要的 `.into_iter()`/`&` 多余）。无 critical 死代码——`codegraph` 报告的零 caller 函数经逐个验证，全部是 `#[test]` 函数或 axum route handler（通过 `.route()` 间接注册，静态分析识别不到）。

主要可改进点是**几个超长函数**和**paddle-ocr crate 内的工具函数重复**。

---

## 1. 死代码

### 结论：基本无显著死代码

| 验证手段 | 结果 |
|---|---|
| `cargo check --workspace --exclude octopus-desktop` | 0 warning（desktop 因前端 `dist/` 未构建无法编译，与代码无关） |
| `cargo clippy --workspace --exclude octopus-desktop` | 3 warning，全在 `paddle-ocr`，均为 nit |
| codegraph "零 caller" 函数（605 个候选） | 经验证全部为 `#[test]`、axum handler、或 codegraph CALLS edge 漏报 |

### 已验证的误报样例

- `decode_wav_to_mono_16k` (asr-local/src/audio.rs:12) — 被 `read_wav_16k` / `read_wav_16k_from_bytes` 调用，codegraph 漏报 CALLS edge
- `normalize_fuzzy_pinyin` / `get_fuzzy_pinyin` (corrector.rs:21/46) — 被 `find_candidates` 调用，codegraph 漏报
- `run_ws_session` / `run_qwen_realtime_session` (aliyun_stream.rs) — 由 `open()` 内 match 分发调用，3+ 处引用
- `health` / `models` / `transcribe` / `ws_stream` / `handle_ws` (server/main.rs) — axum `.route()` 注册
- 所有 `*_returns_*`、`*_finds_*`、`*_routes_*` 等命名的零 caller 函数 — 均为 `#[test]`

### Clippy 3 个 nit（建议顺手修）

```
crates/paddle-ocr/src/det/detector.rs:115    needless_borrow (&model_path)
crates/paddle-ocr/src/det/postprocess/mod.rs:1924   redundant .into_iter()
crates/paddle-ocr/src/rec/word_boxes.rs:76           redundant .into_iter()
```

---

## 2. 重复代码

### 2.1 paddle-ocr crate 内工具函数重复（明确）

| 函数 | 位置 | 状态 |
|---|---|---|
| `l2(a: [f32;2], b: [f32;2]) -> f32` | `rec/word_boxes.rs:391`、`det/postprocess/mod.rs:2014`、`vision/rotate_crop.rs:301` | **3 处完全相同**（4 行实现逐字一致） |
| `saturate_cast_i16_from_f32(v: f32) -> i16` | `vision/resize.rs:60`、`vision/rotate_crop.rs:204` | **2 处完全相同** |
| `cv_round_ties_even_f32` (f32→i32) | `vision/resize.rs:46` | 与 `rotate_crop.rs:176 saturate_cast_i32_round` (f64→i32) 仅类型不同 |
| `saturate_cast_i16` (i32→i16) | `vision/rotate_crop.rs:190` | 单独存在但与上面 saturate 家族同源 |

**建议**：在 `crates/paddle-ocr/src/vision/` 下新增 `mod numeric { ... }`（或类似），集中放置：
- `l2`、`saturate_cast_*`、`cv_round_ties_even_*`、`clamp/clip` 系列

### 2.2 clamp / clip 命名不一致

| 函数 | 位置 | 语义 |
|---|---|---|
| `clip_i32(x, lo, hi_exclusive)` | `vision/resize.rs:64` | hi 是 exclusive（返回 hi-1） |
| `clamp_i32(v, min_v, max_v)` | `vision/rotate_crop.rs:172` | hi 是 inclusive |

虽然语义不同（一个 hi_exclusive 一个 hi_inclusive），但**命名同义混乱**，调用方很容易踩坑。建议：保留语义差异，但改名让语义在名字上可见，例如 `clip_i32_exclusive_upper` / `clamp_i32_inclusive`。

### 2.3 compute_fbank_features 三处定义（潜在去重，但有风险）

| 位置 | 用途 | 参数 |
|---|---|---|
| `asr-local/src/fbank.rs:32` | SenseVoice / FireRed | hamming 窗、无 pre-emphasis、+LFR |
| `asr-local/src/paraformer.rs:448` | 离线 Paraformer | hamming 窗、pre-emph 0.97、+LFR |
| `asr-local/src/zipformer.rs:1187` | Zipformer | 中心点帧 + reflect padding、无 LFR |

`paraformer.rs:462` 已经把 `compute_fbank` 参数化为 `(samples, window, preemph_coeff)`，**理论上可以替代 `fbank.rs:40` 那个简单版本**（传 preemph_coeff=0.0 即等价于无 pre-emphasis）。Zipformer 版本结构不同，不建议合并。

⚠️ AGENTS.md 中明确警告："修改 fbank 实现前务必对照 sherpa-onnx 参考实现"——**合并前需做数值回归测试**（对比 LFR 后 560 维向量）。

---

## 3. 超长代码

### 3.1 超长函数（Rust，按行数）

| 行数 | 函数 | 文件 | 建议 |
|---|---|---|---|
| **502** | `start_scroll_recording` | `desktop/src/screenshot_commands.rs:834` | **必须拆**——见下方详细建议 |
| **471** | `run` | `desktop/src/main.rs:48` | main 入口，承担 panic_hook + config 加载 + DB init + 引擎校验 + Tauri builder，建议按"启动阶段"拆 4-5 个辅助函数 |
| **331** | `Coordinator::new` | `desktop/src/coordinator.rs:190` | 初始化 12+ 字段，建议按字段分组提取 builder helper |
| **228** | `begin_recording` | `desktop/src/coordinator.rs:712` | 见下方详细建议 |
| **192** | `pin_window::create` | `desktop/src/pin_window.rs:696` | macOS Cocoa 桥 + 窗口创建混合，建议拆 platform 层 |
| 190 | `qwen3_asr::transcribe` | `asr-local/src/qwen3_asr.rs:142` | 可接受，ASR 推理流程自然较长 |
| 188 | `whisper::transcribe` | `asr-local/src/whisper.rs:311` | 同上 |
| 181 | `run_bytedance_session` | `asr-cloud/src/bytedance_stream.rs:100` | WS 三方接入流程，难拆 |
| 181 | `transcribe_url` | `cli/src/main.rs:200` | 可拆 download / decode / transcribe / print 四段 |
| 178×2 | `run_ws_session` / `run_qwen_realtime_session` | `asr-cloud/src/aliyun_stream.rs` | WS session 主循环 |
| 171 | `run_baidu_session` | `asr-cloud/src/baidu_stream.rs:68` | 同上 |
| 167 | `run_e2e_streaming_paraformer` | `cli/src/main.rs:627` | CLI 端到端测试 |
| 153 | `handle_toggle` | `desktop/src/coordinator.rs:943` | 状态机切换 |
| 152 | `start_screenshot` | `desktop/src/screenshot_commands.rs:59` | |
| 150 | `handle_clipboard_change` | `clipboard/src/watcher.rs:80` | |

#### 重点拆分建议：`start_scroll_recording` (502 行)

位于 `crates/desktop/src/screenshot_commands.rs:834-1336`，单函数承担：

1. 互斥锁（`SCROLL_RECORDING.swap`）
2. tokio::spawn 异步任务
3. **选区窗口定位**（`get_webview_window` + outer_position + scale_factor）
4. **窗口原点坐标换算**（CGDisplay bounds / Cocoa frame / Tauri 物理↔逻辑）
5. **显示器命中检测 + DPI scale 计算**（遍历 `available_monitors`）
6. **macOS Quartz 特定代码**（`CGDisplay::active_displays`、`get_window_number`、`CGWindowList` exclude）
7. 物理像素裁剪参数（`px/py/pw/ph`）
8. 滚动录制主循环（截图 → 拼接 → emit）

**建议拆分为**：
```rust
start_scroll_recording(...) -> Result<()>   // 仅做参数校验 + 互斥 + spawn
  └─ async move {
       let ctx = resolve_selection_geometry(&ah, win_label, x, y, w, h)?;
       let quartz = resolve_macos_quartz_exclusion(&ctx)?;     // #[cfg(target_os="macos")]
       scroll_capture_loop(ctx, quartz, interval_ms, ...).await;
     }
```

预计可降到 80-100 行主函数 + 3-4 个 ~80 行辅助函数。

#### 重点拆分建议：`begin_recording` (228 行)

位于 `coordinator.rs:712-940`，建议按状态机阶段拆：
- `prepare_recording_state(...)` — 重置 buffers、state、cancel token
- `select_engine_and_branch(...)` — embedded/cloud/VAD 分段路径选择
- `spawn_streaming_pipeline(...)` — 启动后端流式任务
- `register_cancel_handler(...)` — 注册取消回调

### 3.2 超长文件

| 行数 | 文件 | 性质 |
|---|---|---|
| 2439 | `desktop/src/coordinator.rs` | 业务中枢，22 个 pub/private 函数，**可拆为 `coordinator/` 子模块**（state、recording、cloud、polish、paste 五块） |
| 2226 | `paddle-ocr/src/det/postprocess/mod.rs` | DBNet 后处理，单模块过大，可拆 `postprocess/{contour, mask, nms, box}.rs` |
| 2005 | `infra/src/db.rs` | 多表 DDL + CRUD，可按表拆 `db/{models, transcriptions, migrations}.rs` |
| 1842 | `asr-local/src/whisper_mel_matrix.rs` | ⚪ 预生成常量表（`[[f64;201];80]`），**非代码逻辑，不算超标** |
| 1539 | `asr-local/src/zipformer.rs` | 单引擎实现，可接受 |
| 1419 | `capx/src/stitch.rs` | 滚动截图拼接 |
| 1353 | `desktop/src/screenshot_commands.rs` | 截图命令集合 |
| 1183 | `cli/src/main.rs` | CLI 多子命令 |

### 3.3 超长 React 组件（前端）

| 行数 | 组件 | 文件 |
|---|---|---|
| **1011** | `Screenshot` | `desktop/frontend/src/pages/Screenshot/index.tsx` |
| **807** | `ImagePreview` | `desktop/frontend/src/pages/ImagePreview/index.tsx` |
| **739** | `Result` | `desktop/frontend/src/pages/Result/index.tsx` |
| 436 | `CompactEditor` | `desktop/frontend/src/pages/CompactEditor/index.tsx` |
| 310 | `ClipboardItemRow` | `desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` |

`Screenshot`、`ImagePreview`、`Result` 三个单文件组件均超 700 行，建议按子组件 + hook 拆分（如 `Screenshot/useScrollRecording.ts`、`Screenshot/SelectionOverlay.tsx`、`Screenshot/StatusBar.tsx`）。

---

## 4. 优先级建议

### 高（影响可维护性）
1. **拆分 `start_scroll_recording` (502 行)** — 收益最大
2. **paddle-ocr 工具函数集中** — 新建 `vision/numeric.rs` 收纳 `l2` / `saturate_cast_*` / `cv_round_*` / `clamp/clip`，消除 3 处 `l2` 完全重复

### 中
3. **拆分 `Coordinator::new` (331 行)** 和 `begin_recording` (228 行)
4. **clamp/clip 命名统一** — 让 inclusive/exclusive 语义在名字上可见
5. **拆 `coordinator.rs` (2439 行)** 为子模块
6. **修 clippy 3 个 nit**（`cargo clippy --fix --lib -p octopus-paddle-ocr`）

### 低（有风险需评估）
7. **合并 fbank.rs / paraformer.rs 的 `compute_fbank`**（参数化已经做好，仅差调用方迁移 + 数值回归）
8. **拆 `paddle-ocr/src/det/postprocess/mod.rs` (2226 行)**
9. **前端 3 个超长组件拆分**（Screenshot/ImagePreview/Result）

### 不建议动
- ASR `transcribe` 系列（whisper/qwen3_asr）— 长度合理，拆了反而打断推理流程阅读
- `whisper_mel_matrix.rs` — 数据表，不是代码
- 各种 `run_*_session` — WS 三方接入流程，业务上必然长

---

## 附：方法学说明

**为什么不用 `cargo udeps` / `cargo dead`？** 这类工具只对二进制 crate 有效，对 workspace 内 library crate（pub API）会大量误报。本项目大部分 crate 是 lib，用 codegraph + 编译器 + clippy + 手工 grep 验证更可靠。

**codegraph CALLS edge 漏报说明**：本审查发现 codegraph 对同文件内函数调用、trait 方法动态分发、宏注册的路由/命令（axum `.route`、tauri `generate_handler`）追踪不完整。未来若用 codegraph 做"死代码扫描"，需要补充：
- 装饰器识别（`#[tauri::command]`、`#[axum::debug_handler]`、`#[test]`、`#[tokio::test]`）
- 同文件 caller 边解析强化
