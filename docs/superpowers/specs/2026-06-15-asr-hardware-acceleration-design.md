# 设计文档：ASR 硬件加速（手动开关 + 优雅回退）

> 为 ASR 推理增加硬件加速（CUDA / DirectML / CoreML execution provider）的手动开关与平滑降级：配置开启时在 ONNX Runtime 注册 EP，加载失败/异常自动回退 CPU；关闭时纯 CPU 推理。VAD 不受影响（固定 CPU）。

> **实现状态（2026-06-15）**：已实现并经 macOS CoreML 手动验证。`cargo test -p octopus-asr` 16 passed / 0 failed。

## 1. 背景与目标

ASR 大模型（Qwen3-1.7B、SenseVoice 等）CPU 推理慢（RTF ~1.3x）。目标：可选启用 GPU/CoreML/DirectML 加速。

**为何手动开关而非自动**：部分大模型（如 Qwen3-1.7B）含大量动态 Shape，CoreML 不完全支持其算子，构建 session 时会被 EP 拦截中止。若默认开启会导致此类模型无法加载。因此提供显式开关 `asr_hardware_accelerated`，默认 `false`（CPU，稳定），用户按需开启。

## 2. 设计

### 2.1 EP 注册顺序
开启时按序尝试注册（ort 按序匹配首个可用）：

1. `CUDAExecutionProvider`（NVIDIA GPU）
2. `DirectMLExecutionProvider`（Windows GPU）
3. `CoreMLExecutionProvider`（macOS Apple Silicon/GPU）

### 2.2 优雅回退
- 注册成功 → 用加速 session。
- 注册失败（EP 不可用 / 算子不支持）→ `log::warn` + **重建一个干净的 CPU session builder**（非降级到部分 EP，而是整体回退 CPU），保证识别不中断。

### 2.3 VAD 不受影响
VAD（silero_vad_v4，1.8M 微小模型）保持纯 CPU——`find_silero_vad` 固定路径加载，不走 `apply_session_acceleration`。微小模型加载加速器的额外开销（EP 初始化）不划算。

## 3. 配置

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `asr_hardware_accelerated` | bool | `false` | true 时 ASR 推理启用 CUDA/DirectML/CoreML EP（失败回退 CPU）；VAD 不受影响 |

```yaml
# ~/.octopus/config.yaml
asr_hardware_accelerated: false  # true 启用 GPU/CoreML/DirectML（失败回退 CPU）
```

## 4. 变更点

### 4.1 infra（config schema）
- `crates/infra/src/config.rs`：`AppConfig` 加 `asr_hardware_accelerated: bool`（`#[serde(default = "default_asr_hardware_accelerated")]`，默认 `false`）+ `default_asr_hardware_accelerated()` + `Default` impl + 测试 `asr_hardware_accelerated_defaults_to_false`。

### 4.2 asr（依赖 + 加速包装 + config 缓存）
- `crates/asr/Cargo.toml`：ort features 加 `cuda` / `coreml` / `directml`（编译平台特定 EP）。
- `crates/asr/src/config.rs`：
  - `pub fn apply_session_acceleration(builder: SessionBuilder) -> Result<SessionBuilder>`：查 `asr_hardware_accelerated`，true 则注册 3 个 EP，失败重建 CPU builder。
  - **config 缓存**：`static APP_CONFIG: OnceLock<AppConfig>` + `load_app_config_cached() -> &'static AppConfig`。首次读 config.yaml 后缓存，避免每次 session 构建重复读文件 + 解析 yaml（paraformer 一次识别建 encoder+decoder 两个 session，streaming 引擎更频繁）。读失败回退 `AppConfig::default()`（ASR 保持 CPU）。手编 config.yaml 需重启进程生效（与 `RUNTIME_CONFIG` 一致）。

### 4.3 引擎接入
8 个引擎的 `Session::builder()?` 改为 `apply_session_acceleration(Session::builder()?)?`：

- `whisper.rs`（encoder / dec_init / dec_past，3 处）
- `qwen3_asr.rs`（conv_session / encoder_session / decoder_session，3 处）
- `paraformer.rs` + `streaming_paraformer.rs`（encoder_session / decoder_session）
- `zipformer.rs` + `streaming_zipformer.rs`（session）
- `sensevoice.rs`（session）

## 5. 验证

### 5.1 自动化回归
`cargo test -p octopus-asr` → 16 passed / 0 failed。

### 5.2 macOS CoreML 手动
| 模型 | 加速 | 结果 |
|---|---|---|
| qwen3-asr-1.7B | false（CPU） | 正常，4.13s（RTF 1.36x） |
| SenseVoice | true（CoreML） | 成功加载 CoreML EP，识别正确，4.15s（RTF 1.35x） |
| qwen3-asr-1.7B | true（CoreML） | encoder_session 构建时被 CoreML 拦截（动态 Shape 算子不支持）→ **印证手动开关的必要性** |

## 6. 约束与风险

- **config 缓存**：`APP_CONFIG` OnceLock 首次读取后固化，手编 `asr_hardware_accelerated` 需重启进程（与 DB 配置一致）。
- **大模型 EP 兼容性**：Qwen3-1.7B 等动态 Shape 模型在 CoreML 下会失败 → 用户需按模型特性决定是否开启（小/中模型可开，大动态模型建议关）。
- **EP 注册顺序固定**：CUDA → DirectML → CoreML，不可配置（YAGNI；多数平台只有一个可用）。
