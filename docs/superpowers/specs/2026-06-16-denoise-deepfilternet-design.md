# 环境降噪（DeepFilterNet3）设计

> 日期：2026-06-16
> 状态：已设计，待实施计划
> 关联：独立于 `config-infra-and-engine-truth` plan（见 §8 落点注记）

## 1. 背景与目标

octopus 麦克风录音链路当前**无任何噪声消除处理**（已核查：`audio.rs` 仅做多声道下混、格式转换、重采样；`filter_speech`/VAD 是切静音段而非降噪；`normalize_whisper_features` 是 mel 特征域归一化，不在波形上降噪）。环境噪声完全靠 ASR 模型自身鲁棒性硬扛。

**目标**：在语音识别前增加一层基于 ONNX 小模型（DeepFilterNet3）的环境降噪（Noise Suppression, NS），降低稳态/非稳态背景噪声对识别的干扰。

**边界声明**：NS 对「正在播放的音乐」抑制有限（音乐是有结构的信号，非稳态噪声模型会部分压制但无法彻底消除）。降环境噪声（空调/键盘/风扇/背景人声）是其强项。

## 2. 范围

### 2.1 在范围内
- DeepFilterNet3（`dfn3.onnx`）流式环境降噪
- 流式（`drain_samples` 周期取）与非流式（`stop` 整段）两条路径统一受益
- 跨平台：macOS / Windows / Linux

### 2.2 不在范围内（明确排除）
- **回声消除（AEC）**：放弃。octopus 自身不播放任何音频（已核查全仓无 `output_stream`/`playback`/`tts`/`speak`），AEC 所需的「回放参考信号」无法从应用内部获取；系统级音频回环（macOS CoreAudio Tap/ScreenCaptureKit、BlackHole 虚拟声卡）代价过大且侵入用户音频路由。背景音乐干扰由 NS 部分承担。
- **多降噪模型切换**：DF3 是**唯一固定模型**，不进 DB `models` 表，不走 `AsrEngineManager`/`resolve_active_engine`。
- **数据库配置管理**：DF3 不入数据库（与 ASR 引擎模型的管理路径完全不同）。
- **AGC/归一化/高通滤波**：不做（YAGNI）。

## 3. 模型选型

### 3.1 为什么是 `penta2himajin/deepfilternet3-onnx/dfn3.onnx`

候选模型核查（HF 缓存实测 IO 契约）：

| 来源 | 结构 | 是否含 GRU 状态入参 | 流式 |
|---|---|---|---|
| bitsydarel / tonythethompson | 3 文件（enc + erb_dec + df_dec） | 否（`S` 序列维，一次喂多帧） | ❌ 离线展开版 |
| **penta2himajin** | 单文件 `dfn3.onnx`（8.5MB） | 是（`enc_h`/`erb_h`/`df_h`，每帧 S=1） | ✅ 真正的流式有状态版 |

三文件版是展开计算图（无状态、需整段喂入），实时延迟不可接受。**`dfn3.onnx` 是唯一带 GRU 隐状态、支持逐帧实时推理的版本**，故选之。

### 3.2 IO 契约（每帧 hop=480 样本 @48kHz = 10ms）

```
入:
  spec      [1,1,1,481,2]   当前帧 STFT 复数频谱（实部+虚部），n_fft=960 → 481 bins
  feat_erb  [1,1,1,32]      32 个 ERB 频带能量特征
  feat_spec [1,1,1,96,2]    前 96 个 bin 的复数频谱特征
  enc_h     [1,1,256]       encoder GRU 状态（初始 0）
  erb_h     [2,1,256]       erb decoder GRU 状态（2 层，初始 0）
  df_h      [2,1,256]       df decoder GRU 状态（2 层，初始 0）
出:
  enhanced_spec [1,1,1,481,2]   增强后的复数频谱（coefs/mask 已在图内应用）
  new_enc_h / new_erb_h / new_df_h   更新后的状态（下一帧入参）
```

模型直接输出增强频谱，**无需手写滤波系数应用**，后处理仅需 STFT→推理→iSTFT。

## 4. 架构

### 4.1 集成位置：采集层（`SharedAudioState` 内），coordinator 无感

`SharedAudioState` 本就承担「把麦克风原始流转成 ASR 可用的 16k 流」之责，NS 是该职责的自然延伸；且它已持有有状态资源（`AudioResampler`/`Stream`），再加一个 `DenoiseProcessor` 模式一致。VAD/ASR 拿到的仍是干净 16k，**流式/非流式两条路径统一受益，无需改 coordinator**。

### 4.2 数据流

```
cpal 回调（设备原生 SR：mac/win/linux 各异）
   │  多声道下混 → samples buffer（不变）
   ▼
drain_samples() / stop()   ← coordinator 线程调用
   │
   ├─ raw(原生SR) →[重采样 48k]→ DenoiseProcessor(48k) →[重采样 16k]→ out   （denoise_enabled）
   │                            │
   │        每 480 样本(10ms)：STFT(hann, n_fft=960) → feat_erb / feat_spec
   │                            │  dfn3.onnx(spec + 3 组 GRU 状态) → enhanced_spec
   │                            │  iSTFT + overlap-add → 480 增强样本
   │                            └─ GRU 状态跨帧保持（录音会话内）
   │
   └─ raw(原生SR) →[重采样 16k]→ out                                          （denoise 关闭，原逻辑）
```

### 4.3 采样率桥接

- DeepFilterNet 是 **48kHz**，octopus ASR 是 **16kHz**。NS 层工作在 48k 域，前后各一次重采样。
- 「重采样 48k」以 **cpal 报告的设备 SR 为准动态判断**（不写死平台假设）：`if rate==48000 { 直通 } else { 升/降到 48k }`。mac/win/linux 默认输入 SR 各异（48k/44.1k 等），统一桥接到 48k。
- STFT 参数（n_fft=960 / hop=480 / 481 bins / 32 ERB / 96 df）是**模型契约，硬绑 48kHz**——任何平台都必须先重采样到 48k 进 NS，频带映射才正确。这是跨平台一致的硬约束。
- `DenoiseProcessor` 内部维护 48k 输入缓冲 + OLA 输出缓冲（跨次 `drain_samples` 保留残帧），与现有 `AudioResampler` 的增量 + flush 模式同构。

## 5. 组件

### 5.1 新模块 `crates/asr/src/denoise.rs`

逻辑集中于此；`desktop/src/audio.rs` 只持有 `Option<DenoiseProcessor>` 并调用，保持薄。

```rust
pub struct DenoiseProcessor {
    session: ort::session::Session,       // dfn3.onnx
    // GRU 隐状态（持久，跨帧传递）
    enc_h: Array3<f32>,   // [1,1,256]
    erb_h: Array3<f32>,   // [2,1,256]
    df_h:  Array3<f32>,   // [2,1,256]
    // 流式增量状态
    in_buf:   Vec<f32>,   // 48k 输入累积，每满 480 触发一帧
    out_buf:  Vec<f32>,   // 已增强样本待 drain
    ola_prev: Vec<f32>,   // 上一帧 iSTFT（overlap-add 用）
    // DSP 常量（构造时算一次）
    window:    Vec<f32>,            // 分析/合成窗（hann/sqrt-hann，对齐 DF）
    erb_bounds: Vec<(usize,usize)>, // 481 bin → 32 ERB 带边界
    fft: rustfft::FftPlanner<f32>,  // n_fft=960
}

impl DenoiseProcessor {
    pub fn new(model_path: &Path) -> Result<Self>;   // 加载 session + 算窗/ERB 表 + 状态归零
    pub fn process_samples(&mut self, s48k: &[f32]) -> Vec<f32>;  // 增量：in_buf 累积，逐帧 STFT→feat→run→iSTFT+OLA
    pub fn flush(&mut self) -> Vec<f32>;              // 尾部零填，吐残留（同 AudioResampler::flush 模式）
    pub fn reset(&mut self);                          // GRU + 缓冲清零
}
```

### 5.2 每帧处理流水（`process_samples` 内）

```
in_buf 凑满 480 → 取 [上帧尾 480 .. 上帧尾+960] = 960 样本
  → × window → rustfft(960) → spec[481] 复数
  → feat_erb[32]    = 按 erb_bounds 对 |spec|² 分带求和
  → feat_spec[96,2] = spec 前 96 bin 的 (re, im)
  → ort run(spec, feat_erb, feat_spec, enc_h, erb_h, df_h)
       → enhanced_spec[481,2], new_enc_h, new_erb_h, new_df_h
  → rustfft 逆变换(enhanced_spec) → × window → OLA(减上帧重叠) → 480 增强样本入 out_buf
  → in_buf 弹出 480（保留余数供下次）
```

### 5.3 STFT：复用现有 `rustfft`

`crates/asr/Cargo.toml` 已依赖 `rustfft = "6"`，**零新依赖**（不引入 realfft）。窗类型与 OLA 增益系数实施时对齐 `deepfilter-rt`（https://github.com/shimondoodkin/deepfilter-rt）参考代码（见 §13 实施前提）。

## 6. 状态管理（呼应上次 VAD 状态污染教训，但 NS 语义相反）

| | filter_vad（已修） | DenoiseProcessor |
|---|---|---|
| 状态本质 | 「当前是否语音段」——每段语义独立 | 「噪声环境稳态估计」——连续物理过程 |
| 段间 | **每段 reset**（独立语义） | **保持**（reset 会丢噪声估计，段首几帧降噪失效=温启问题） |
| 会话边界 | start 时新建实例 | `start()` 调 `reset()`（新噪声环境起点） |

- **录音会话内**：GRU 状态跨 `drain_samples` 周期、跨 VAD 分段**连续保持**（与 filter_vad 的每段 reset **故意相反**，因噪声估计不应被分段打断）。
- **会话边界**：`SharedAudioState::start()` 调 `denoise.reset()`，与现有「`start` 重置 resampler」同模式。
- `Send/Sync`：`ort::Session` 本身 `Send+Sync`，加入 `SharedAudioState` 不破坏其既有 unsafe impl 的不变量（仍只 coordinator 单线程访问）。

## 7. 跨平台

- **ort EP 矩阵已是三平台**：`Cargo.toml` 的 `ort = { features = ["download-binaries","cuda","coreml","directml"] }` 在运行时按平台自动选最优 EP（mac→CoreML、win→DirectML、linux→CUDA/CPU）。DF3 推理三平台都走最优路径。
- **cpal 跨平台采集**：CoreAudio / WASAPI / ALSA-Pulse。多声道下混已在 `audio.rs` 回调内完成（均值），NS 拿到 mono，跨平台一致。
- **验证前提**（写进实施计划）：三平台采集实测（WASAPI shared mode / ALSA 默认设备的 SR 报告与下混）；性能下限（弱 CPU：低档 win 笔记本 / linux ARM，单帧推理 <10ms 实时预算，兜底用 `ort::with_intra_threads` 可配线程数）。

## 8. 配置

- 仅加一个配置项 `denoise_enabled: bool`（默认 `true`）。
- 模型固定走 HF cache，**不**暴露为配置项（DF 只一个模型，无切换需求）。
- DF3 **不进数据库**，不参与 `models` 表 / `AsrEngineManager` / `resolve_active_engine` 体系。
- 新增 `crates/asr/src/config.rs::find_df3()`：从 `~/.cache/huggingface/hub/models--penta2himajin--deepfilternet3-onnx/snapshots/*/dfn3.onnx` 定位（glob snapshots 子目录，复刻现有 HF cache `find_*` 模式）。**缺失时错误信息固定**：
  ```
  DeepFilterNet3 模型缺失，请先下载：hf download penta2himajin/deepfilternet3-onnx
  ```
- `denoise_enabled` 字段加到 `infra::AppConfig`（配置 schema 已下沉 infra），默认 `true`（在 `Default` impl 中设置）。

## 9. 错误降级

「前处理是增强，失败降级直通，不阻断识别」——与现有 `mic missing→silent` / `DB init failed→storage disabled` 同哲学。

| 故障 | 行为 |
|---|---|
| `find_df3()` 缺失 / onnx 解析失败 | `DenoiseProcessor::new` 返回 Err → `SharedAudioState` 持 `None` → drain/stop 走原逻辑（直接 16k 重采样），日志 `warn`（含下载提示），**录音不阻断** |
| 单帧推理失败（罕见） | 该帧 bypass（输出未降噪原样本），日志 `warn`，GRU 状态保持，继续下一帧，**不 panic** |
| `denoise_enabled=false` | 不建 session，零开销直通 |

## 10. 测试策略

- **DSP 正确性**：STFT→iSTFT（不经模型）OLA 重建误差，干净信号重建 SNR > 40dB；`feat_erb` 分带能量对已知频谱的数值正确性。
- **样本守恒**：带噪 wav 经 `process_samples`→`flush`，输出总长 == 输入长（OLA 不丢不增）。
- **流式一致性**（强制项，呼应 paraformer 边界 bug）：同一信号「分 N 次增量 `process_samples`」与「一次性」输出应逐样本相等——验证无状态漂移、无边界丢帧。
- **状态语义**：连续两帧 GRU 状态更新；`reset()` 后归零。
- **跨平台 CI**：mac 默认跑（win/linux 视 CI）；模型从 HF cache 拉。
- 不强求「降噪后识别准确率提升」端到端指标（难量化），但实施后手动对比脏/净样本听感与识别结果。

## 11. 模型分发

HF cache 模式（与 ASR 引擎模型同源）。用户 `hf download penta2himajin/deepfilternet3-onnx` 下载到 `~/.cache/huggingface/hub/`，`find_df3()` 读取。零 bundle 体积，三平台一致。

## 12. 验收标准

1. `denoise_enabled=true` 时，带噪录音经处理后环境噪声听感明显降低，识别结果改善（手动验证）。
2. `denoise_enabled=false` 时，行为与现状完全一致（零回归）。
3. 模型缺失时，应用正常启动、录音正常工作（仅日志告警 + 下载提示），不崩溃。
4. 流式增量与一次性处理输出逐样本相等。
5. mac/win/linux 三平台单帧推理 <10ms。

## 13. 实施前提（需在实施首步确认）

1. **STFT 窗类型与 OLA 增益**：对齐 `deepfilter-rt` 参考实现（hann / sqrt-hann / COLA 增益系数），确保重建无损。
2. **ERB 边界表**：32 个 ERB 带对 481 bin 的边界常量，取自 DeepFilterNet 原始 `df` crate / `deepfilter-rt`。
3. **三平台采集实测**：cpal WASAPI(shared)/ALSA(默认设备) 的 SR 报告与下混行为，确认与 mac 一致。
4. **性能基准**：单帧推理耗时三平台实测，确认实时性。

## 关键文件

- `crates/asr/src/denoise.rs`（新建：`DenoiseProcessor` + STFT/feat/OLA）
- `crates/asr/src/config.rs`（新增 `find_df3()`）
- `crates/desktop/src/audio.rs`（`SharedAudioState` 持 `Option<DenoiseProcessor>`，drain/stop 接入，start 调 reset）
- `crates/infra/src/config.rs`（加 `denoise_enabled` 字段，默认 true）
- `crates/infra/src/consts.rs`（可选：DF3 HF repo 名常量）
