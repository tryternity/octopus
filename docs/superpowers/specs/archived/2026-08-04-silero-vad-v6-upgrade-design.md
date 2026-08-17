# Silero VAD v4 → v6 升级

- 日期：2026-08-04
- 分支：`feat/vad-v6`
- Worktree：`.worktrees/feat-vad-v6`
- 类型：模型升级（破坏性 ONNX 签名变更 + 输入格式变更）
- Baseline：`cargo test -p octopus-asr-local --lib vad` → 14 passed

---

## 1. 背景与动机

内嵌 Silero VAD 从 v4 升级到 v6（官方 v6.2.0）。v6 相对 v4 的改进：

- 噪声数据错误率 **-16%**，多人讲话 **-11%**（相对 v5，v5 已优于 v4）
- 灵敏度更高：实测模拟人声 prob **0.90**（v4 0.15-0.32），静音 prob **0.002**（v4 0.04）

## 2. v6 vs v4 ONNX 签名差异

| 项 | v4 | v6 | 破坏性 |
|---|---|---|---|
| 状态输入 | `h` + `c`（两个 `[2,1,64]`） | `state`（一个 `[2,1,128]`） | ✅ |
| 状态输出 | `hn` + `cn`（两个） | `stateN`（一个） | ✅ |
| 状态维度 | 64 ×2 | 128 ×1 | ✅ |
| **输入拼接** | 直接 `[1, 512]` | **context 拼接** `[1, 576]` = context(64) + samples(512) | ✅ |
| `input`/`sr`/`output` | 同名 | 同名 | ❌ |
| 窗口 | 480 可用 | **必须 512**（480 拼出 544 ≠ 576，输入 shape 不匹配） | ✅ |
| 阈值 0.5 / 16kHz | 有效 | 有效 | ❌ |

## 3. 关键不变量（防止回退踩坑）

### 3.1 context 拼接（最关键）

v6 的 `compute()` **必须**在输入前面拼上一帧末尾 64 个样本（context）：

```
输入 = [context(64) + samples(512)] → shape [1, 576]
```

**漏拼 context 的后果**：模型输入分布失配 → prob 恒近零（实测 0.0008），完全不区分语音/静音。

推理后更新 context：取本帧输入末尾 64 样本，供下帧拼接。参考官方实现 `silero-vad/examples/rust-example/src/silero.rs::calc_level`。

### 3.2 窗口必须 512

v6 要求窗口 512 样本（16kHz = 32ms）。v4 用的 480 **不能用**——拼 context 后 480+64=544 ≠ 576，ONNX 输入 shape 不匹配，`filter_speech` 逐窗口判定全静音 → buffer 空 → `no speech detected in buffer`。

**所有 VAD 调用点的窗口必须统一 512**：
- 流式检测：`VAD_CHUNK_SIZE = 512`（desktop pipeline / streaming_runner，已正确）
- 离线分段：`segment_audio_vad` / `filter_speech` / `segment_audio_vad_with_offsets` 的 `frame_size` 参数
- 桌面过滤：`filter_speech_from_buffer`

### 3.3 模型文件选择

选用官方 `silero_vad_16k_op15.onnx`（16kHz 专用精简版）：

| 候选 | 体积 | opset | sr 输入 | voice prob | silence prob |
|---|---|---|---|---|---|
| **16k_op15（选用）** | **1.2 MB** | 15 | ✅ 有 | 0.90 | 0.002 |
| half | 1.25 MB | 16 | ❌ 无（硬编码 16k） | 0.96 | 0.012 |
| 完整版 silero_vad.onnx | 2.3 MB | 16 | ✅ 有 | 0.90 | 0.002 |

选 16k_op15 理由：保留 `sr` 输入签名（代码改动最小）、静音误判更低（0.002 vs 0.012）、opset 15 兼容性好。体积比 v4（1.8MB）小 33%。

⚠️ **不能用 HF 第三方打包**（如 `BricksDisplay/silero-vad-6.2`）——其 ONNX 导出的 `If` 节点分支逻辑有 bug，Python 直接跑也返回近零值。只从官方仓库 `snakers4/silero-vad` 的 `src/silero_vad/data/` 获取。

## 4. 实现细节

### 4.1 SileroVad struct（`crates/asr-local/src/audio/vad.rs`）

```rust
pub struct SileroVad {
    session: Arc<Mutex<Session>>,
    state: Array3<f32>,     // [2, 1, 128]——LSTM 状态
    context: Array1<f32>,   // [64]——上一帧末尾样本
    sr: Array1<i64>,        // [16000]
}
```

### 4.2 compute 流程

```
1. 拼接 buf = context(64) + samples → [1, 576]
2. session.run({ input, sr, state })
3. 读 output → prob
4. 读 stateN → 更新 self.state
5. 更新 self.context = samples 末尾 64 样本
6. 返回 prob
```

### 4.3 reset

```rust
self.state = zeros([2, 1, 128]);
self.context = zeros([64]);
```

### 4.4 窗口统一

v4 时期多处硬编码 480（离线 segment/filter + 桌面 filter_speech_from_buffer），v6 全部改为 512。

## 5. 测试

| 测试 | 守护的不变量 |
|---|---|
| `v6_speech_prob_higher_than_silence` | 语音 prob > 静音 ×3（防 context 拼接缺失 / 模型损坏） |
| `new_builtin_compute_silence` | 静音 prob ∈ [0,1] |
| `new_builtin_shares_session` | Session 缓存生效 |
| `builtin_cache_key_exists` | builtin:// cache key 正确 |

## 6. 验证

- `cargo test -p octopus-asr-local --lib vad` → **14 passed**
- `cargo build -p octopus-desktop` → 编译通过
- **e2e 实测通过**（2026-08-04）：录音识别正常，VAD 正确检测语音切段，不再出现 `no speech detected in buffer`
