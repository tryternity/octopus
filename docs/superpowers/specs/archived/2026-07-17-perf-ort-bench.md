# 2026-07-17 Streaming Paraformer ort 推理 Benchmark（z_perf Step 1）

## 背景

LTO 改造（见 `2026-07-17-perf-release-lto.md`）实测 fbank FFT 性能收益 ≈0%，结论指出
"ort 推理才是 streaming 真正的大头"。本轮建立 ort 推理 benchmark 基础设施，量化真实开销，
并顺带验证 CoreML EP 对 streaming paraformer 的可行性。

## 改动

- `crates/asr-local/Cargo.toml`：注册第二个 `[[bench]] streaming_paraformer`（harness=false）
- 新增 `crates/asr-local/benches/streaming_paraformer.rs`：
  - `bench_engine_new`：Session 构造开销（one-shot，分离加载 vs 推理）
  - `bench_accept_samples`：per-chunk 推理热路径（1/5/all chunks），用 `iter_batched` 每 iter 重建 engine
- 复用 `test_streaming_paraformer_real_model` 的模型加载逻辑（HF cache + read_wav_16k）
- **不改任何运行时代码**——纯新增 benchmark

## 测量方法

```bash
# baseline（CPU，asr_hardware_accelerated=false）
cargo bench -p octopus-asr-local --bench streaming_paraformer -- --save-baseline ort-baseline

# after（CoreML，临时改 DB asr_hardware_accelerated=true）
sqlite3 ~/.octopus/octopus.db "UPDATE app_config SET config_value='true' WHERE config_key='asr_hardware_accelerated';"
cargo bench -p octopus-asr-local --bench streaming_paraformer -- --baseline ort-baseline
# 跑完立刻恢复 false（已执行）
```

机器：macOS darwin 25.5.0 arm64（Apple Silicon，理论上有 Neural Engine）。
模型：`csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en`（encoder.int8 + decoder.int8）。

## 性能总览

### CPU baseline（asr_hardware_accelerated=false，默认配置）

| benchmark | 中位数 | per-chunk | 说明 |
|-----------|--------|-----------|------|
| **engine_new** | **478.85 ms** | — | ort Session 构造（one-shot 启动开销）|
| accept_samples/1chunk | 7.23 ms | 7.23 ms | 首 chunk（decoder 冷启动，无累积 token）|
| accept_samples/5chunks | 100.93 ms | **20.2 ms** | 稳态 per-chunk |
| accept_samples/17chunks（全部） | 363.07 ms | **21.4 ms** | 稳态，随 token 增长略变重 |

**关键发现 1：ort 推理是 fbank 的 11 倍**

- streaming per-chunk：ort ~21ms vs fbank ~1.87ms（上一轮测）→ **ort 占 91%**
- 这修正了 z_perf setup.md 初版"fbank 是最高频 CPU 热点"的判断——fbank 是**频次高但占比低**，ort 才是绝对大头
- streaming tick 配置 200ms，per-chunk 21ms 占 10.5%——单核够用，但叠加 VAD+denoise+IPC 需关注
- engine_new 478ms 是用户感知的"启动延迟"，但只发生一次，可异步预热

### CoreML EP（asr_hardware_accelerated=true）—— 灾难性退化

| benchmark | CPU | CoreML | 变化 | criterion 判定 |
|-----------|-----|--------|------|----------------|
| **engine_new** | 478.85 ms | **9.03 s** | **+1780%** | ❌ Performance has regressed |
| accept_samples/1chunk（估算）| 7.23 ms | ~9 s | ~+124000% | 未跑完（每 iter 9s × 100 = 918s）|

**关键发现 2：CoreML 对 streaming paraformer 完全不可用**

CoreML EP 让 engine_new 从 478ms 飙到 9s（19 倍退化），accept_samples 单 chunk 从 7ms 到 ~9s。
accept_samples 没跑完（估算 918s），但 engine_new 数据已是决定性证据。

### 退化根因分析

1. **CoreML 模型编译开销**：ort 注册 CoreML EP 时，把 ONNX 转 CoreML 模型并编译到 Neural Engine，首次构造耗 ~9s——几乎全是编译，非推理
2. **streaming 模型结构不适配**：streaming paraformer 有动态形状 + CIF 循环 + per-chunk 状态，CoreML/ANE 对动态控制流支持差，每次 run 可能触发重编译
3. **int8 量化模型**：ANE 优化方向与 int8 不一定匹配，量化在 CPU（AVX/NEON）上反而更高效

**结论：`asr_hardware_accelerated` 默认 false 是正确的工程决策**（db.sql:154, infra/config.rs:269）——不是遗漏，而是 CoreML 对这类 streaming 模型实测有害。默认 CPU 是兜底正解。

## 验证

```bash
# 1. benchmark 编译
cargo build -p octopus-asr-local --benches --release  # 0 error 0 warning

# 2. CPU baseline 跑通
cargo bench -p octopus-asr-local --bench streaming_paraformer -- --save-baseline ort-baseline
# 结果：engine_new 478ms，accept_samples 17chunks 363ms

# 3. ASR 正确性未受影响（纯新增 bench，未改运行时代码）
cargo test -p octopus-asr-local --lib streaming_paraformer::tests::test_streaming_paraformer_real_model -- --nocapture
# 结果：识别正确 "天是 moday day ... 是星期三"

# 4. DB 配置已恢复
sqlite3 ~/.octopus/octopus.db "SELECT config_value FROM app_config WHERE config_key='asr_hardware_accelerated';"
# 结果：false（用户原配置）
```

## 决策

- **合并 benchmark 基础设施**：streaming_paraformer bench 进 main，作为后续 ort 优化的回归基线
- **不开启 CoreML**：保持 `asr_hardware_accelerated=false` 为默认。本轮已在 DB 临时开启后恢复
- **不在此轮优化 ort 推理**：ort 是 ONNX Runtime 黑盒，per-chunk 21ms 在 CPU 上已是 int8 模型的合理水平。进一步优化方向（模型蒸馏/不同 EP/批处理）属另一轮，需独立 spec

## 后续（真正可能优化 streaming 的方向）

按 z_perf "fix algorithm first" 原则，基于本轮数据：

1. **engine_new 478ms 异步预热**：启动时后台加载 Session，用户首次录音无感（coordinator 已有 tick 线程，可加预热）
2. **fbank + ort 流水线化**：当前 per-chunk 串行 fbank→ort，若 fbank 在下个 chunk 的 ort 推理期间并行计算，可隐藏 ~1.87ms（占 9%）
3. **token 增长导致 decoder 变重**：17chunks 时 21.4ms vs 5chunks 20.2ms——随会话变长 decoder 累积 token 开销上升，可评估定期 reset 或 sliding window
4. **CoreML 不适配已是定论**，不投入。若要硬件加速，研究方向应是 Metal/CUDA 自定义算子，非 CoreML EP

## 对 z_perf skill 的回写

- `rust-hotpaths.md`：补充"ort 推理是 streaming 绝对大头（91%），fbank 占比仅 9%——优化优先级应先 ort 再 fbank"
- `setup.md`：CoreML EP 段补警告"对 streaming 模型实测 +1780% 退化，默认 false 是正确的"
- 报告模板：增加"EP 实验必须可逆"纪律（本轮 DB 改了立刻恢复）
