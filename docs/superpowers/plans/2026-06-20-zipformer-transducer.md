# Zipformer Transducer (RNN-T) 引擎 Implementation Plan

> 状态：**已实现**（离线 commit `465e901` merge `f238c47`；流式 commit `415e89c` merge `109cccb`；归一化 fix commit `0d7ef5c`）
> Spec：`docs/superpowers/specs/2026-06-20-zipformer-transducer-design.md`

**Goal:** 将原 `ZipformerEngine`（仅 CTC）重命名为 `ZipformerCtcEngine`，新增 `ZipformerTransducerEngine`（RNN-T 三 session 架构，离线 + 流式），支持两个新中文模型。

---

## File Structure（实际）

- `crates/asr/src/zipformer.rs` — 重命名 + 新增 Transducer struct + 共享函数提取
- `crates/asr/src/streaming_zipformer.rs` — 新增 `StreamingZipformerTransducer`（流式 RNN-T）
- `crates/asr/src/streaming_engine.rs` — `StreamingSession` 枚举新增 `ZipformerTransducer` 变体 + `ZipformerStreamOps` trait
- `crates/asr/src/engine.rs` — 路由更新（`decoder.onnx` 检测分流）
- `crates/infra/src/db.sql` — seed 新增两个 Transducer 模型
- `docs/architecture.md` / `docs/configuration.md` — 文档同步

---

## Tasks（已完成）

### Task 1: 重命名 ZipformerEngine → ZipformerCtcEngine ✅

- [x] `pub struct ZipformerEngine` → `pub struct ZipformerCtcEngine`
- [x] `impl ZipformerEngine` → `impl ZipformerCtcEngine`
- [x] `impl OfflineAsrEngine for ZipformerEngine` → `for ZipformerCtcEngine`
- [x] `transcribe()` 公开函数 + 测试中的引用更新
- [x] 验证：7 处引用全部改名，grep 无 `ZipformerEngine` 残留

### Task 2: 提取共享函数 ✅

- [x] `load_vocab(hf_path) -> Result<Vec<String>>` — tokens.txt 解析（从 CTC `new()` 提取）
- [x] `initial_encoder_states(session) -> Vec<(String, StateValue)>` — encoder 缓存初始化（从 CTC `new()` 提取）
- [x] `decode_token_ids(vocab, is_bbpe, ids) -> String` — token ID → 文本（BBPE + SentencePiece byte-fallback，从 CTC `transcribe()` 提取）
- [x] CTC `new()` / `transcribe()` 重构使用共享函数

### Task 3: 实现 ZipformerTransducerEngine ✅

- [x] struct 定义（三 `Mutex<Session>` + chunk_len/shift/context_size/vocab/is_bbpe/initial_states/is_whisper）
- [x] `new()`：发现 encoder/decoder/joiner 三文件，加载三 session，读 metadata（T/decode_chunk_len/feature/context_size），encoder_dim 从输出 shape 动态读
- [x] `run_decoder()` / `run_joiner()` 辅助方法
- [x] `impl OfflineAsrEngine::transcribe()`：
  - [x] 特征提取（compute_whisper_features_linear + normalize）
  - [x] Chunked encoder 推理（同 CTC：chunk 循环 + state 管理）
  - [x] RNN-T greedy decoding（token_buf 初始化 `[-1,...,-1,0]`，每 frame joiner→argmax，非 blank 发射 + 重跑 decoder）
  - [x] 内循环安全上限 20 次/frame
  - [x] token 解码（decode_token_ids 共享函数）

### Task 4: 路由更新 ✅

- [x] `engine.rs` import 改为 `{ZipformerCtcEngine, ZipformerTransducerEngine}`
- [x] `EngineCategory::Zipformer` 分支：检测 `decoder.onnx` 存在性分流

### Task 5: DB seed + 文档同步 ✅

- [x] `db.sql` seed 新增 `zipformer-zh-transducer`（154M）和 `zipformer-xlarge-transducer`（726M）
- [x] `architecture.md`：Zipformer 引擎族表格（CTC vs Transducer 对比）+ 流式判定描述更新
- [x] `configuration.md`：seed 表 + is_streaming 说明 + 模型下载命令
- [x] spec 文档：`docs/superpowers/specs/2026-06-20-zipformer-transducer-design.md`

### Task 6: 验证 ✅

- [x] `cargo build -p octopus-asr`：clean（0 warning）
- [x] `cargo build --release -p octopus-desktop --features "embedded dashscope"`：clean
- [x] `cargo build --release -p octopus-server -p octopus-cli`：clean
- [x] zh-int8 测试：`"对我做了介绍哈那么我想说的是大家如果对我的研究感兴趣呢"` ✓
- [x] xlarge 测试：`"给我做了介绍我想说的是大家如果对我的研究感兴趣"` ✓
- [x] `cargo test -p octopus-desktop --features "embedded dashscope"`：48 passed
- [x] ASR 测试：淗41 passed（3 pre-existing failures 因 HF cache 缺文件，非本次改动）

### Task 7: 流式 Transducer 引擎 ✅

- [x] `StreamingZipformerTransducer` struct（三 `Mutex<Session>` + 跨 chunk 状态 token_buf / emitted_ids / states）
- [x] `new_from_entry(entry)` — 避免 `StreamingSession::new` 双重 DB 查找
- [x] `process_chunks()` / `flush()` / `finish()` / `reset()` 生命周期方法
- [x] `run_chunk()` 两阶段借用（encoder output 提取为 owned Vec<f32> 后再调 decoder/joiner）
- [x] 流式 RNN-T greedy decoding（per frame joiner→argmax，非 blank 发射 + 重跑 decoder，内循环上限 20）
- [x] `StreamingSession` 枚举新增 `ZipformerTransducer` 变体
- [x] `ZipformerStreamOps` trait 抽象 CTC/Transducer 统一接口（accept/flush/finish/reset）
- [x] `streaming_engine.rs::new()` 检测 `decoder.onnx` 分流
- [x] 测试：`test_streaming_zipformer_transducer` 流式 partial 逐 chunk 增量输出 ✓

### Task 8: 流式 Whisper 特征全局归一化 fix ✅

**Bug**：流式引擎的 `normalize_whisper_features` 此前按 per-chunk（每 ~45 帧）执行，静音/语音 chunk 的 max_v 差异巨大 → encoder 输入尺度不一致 → 输出乱码。

- [x] `StreamingZipformerTransducer::process_chunks` — 全局归一化（整段特征一次 normalize 再切片）
- [x] `StreamingZipformerTransducer::finish` — 全局归一化
- [x] `StreamingZipformer::process_chunks`（CTC）— 同步修复（当前 fbank 不受影响，但保证 whisper-CTC 即插即用）
- [x] `StreamingZipformer::finish`（CTC）— 同步修复
- [x] `cargo build -p octopus-asr`：clean（0 warning）
- [x] 流式 Transducer 测试：输出从乱码（"回 月 因 同"式重复）变为可识别中文 ✓

---

## 实际实现偏离原 plan

1. **`encoder_dim` 字段移除**：原 plan 设计 struct 含 `encoder_dim` 字段，但 `transcribe()` 实际从 encoder 输出 shape 动态读 `enc_dim`（每 chunk 都读），struct 字段闲置触发 dead_code warning。删除 struct 字段，保留 `new()` 中的 shape 读取（仅用于初始化日志）。

2. **`ort::inputs!` 宏返回 Vec 非 Result**：原闭包写法 `ort::inputs!{...}?` 编译失败（ort 2.0.0-rc.12 的 `inputs!` 宏直接返回 `Vec`，不是 `Result`）。去掉 `?`。

3. **Session::run 需 &mut self**：原设计 decoder/joiner 用裸 `Session`，但 `run(&mut self)` 要求可变借用。改 `Mutex<Session>` 包裹（encoder 已是 `Mutex`），辅助方法内 `lock().unwrap()` 拿 `&mut Session`。

4. **闭包改方法**：原设计在 `transcribe()` 内用闭包 `run_decoder` / `run_joiner`，但 `&self` 借用 + `Mutex::lock` 生命周期复杂。改为 `impl` 方法 `fn run_decoder(&self, ...)` / `fn run_joiner(&self, ...)`，更清晰。

5. **`new_from_entry` 避免双重 DB 查找**：流式引擎原设计接收 bare_name 内部查 DB，但 `StreamingSession::new` 已通过 `resolve_active_engine` 解析出 entry。改为 `new_from_entry(entry)` 直接接收已解析 entry——避免双重查找 + 可能选错 entry（同名跨 provider 场景）。

6. **两阶段借用（run_chunk）**：ort 2.0.0-rc.12 的 `SessionOutputs` 持有 session 借用，调 decoder/joiner 前必须结束该借用。`run_chunk` 先从 encoder `SessionOutputs` 提取 encoder_out 到 owned `Vec<f32>`（借用结束），再用 owned 数据调 decoder/joiner session。

7. **`ZipformerStreamOps` trait**：原设计在 `StreamingSession` 的 accept/flush/finish/reset 中为 CTC 和 Transducer 各写一套分支，重复严重。提取 trait 统一分发，`StreamingSession` 仅持 `Box<dyn ZipformerStreamOps>`。

8. **流式 Whisper 特征全局归一化（P0 bug fix）**：流式引擎原实现按 per-chunk 调 `normalize_whisper_features`，静音/语音 chunk 的 max_v 差异巨大导致 encoder 输入尺度不一致 → 输出乱码。改为在整段可用特征上一次性全局归一化（与离线引擎一致）。覆盖 CTC + Transducer 两套流式引擎共四处。

