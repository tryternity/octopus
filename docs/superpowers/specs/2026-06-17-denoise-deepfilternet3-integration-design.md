# 环境降噪（DeepFilterNet3 原生整合）设计

> 本文是 `2026-06-16-denoise-deepfilternet-design.md` 的续作。上一版因第三方逐帧 ONNX 导出
> （`penta2himajin/dfn3.onnx`）模型层缺陷（压语音至 ~10%）而弃用 DF3、改用 RNNoise。本版
> 用**官方原生 libDF + tract** 重新整合 DF3，经 spike 验证可行，作为 `denoise_mode=2` 与
> RNNoise（mode=1）并存。

## 0. spike 验证结论（2026-06-17）

在新 worktree 对官方源（`Rikorose/DeepFilterNet`）`v0.5.6` tag + tract `^0.19.4` 跑通完整
逐帧 spike（`libDF/examples/verify_gain.rs`，资产 `assets/clean_freesound_33711.wav` 与
`noisy_snr0.wav`）。> 注：spike 起初在 fork `tryternity/DeepFilterNet` 上进行，后续发现该 fork
> 无任何 tag，正式实施改用上游官方源（tag v0.5.6 = commit `978576aa`，与 fork 本地同一
> commit 等价），见 §3.2 修正说明。

| 指标 | 结果 | 判据 | 结论 |
|---|---|---|---|
| 干净语音 gain | **0.958** | 官方应 0.8–1.0 / dfn3 缺陷 0.10 | ✅ 不压语音 |
| 带噪 gain | 0.604 | 应 < 干净 | ✅ 压 ~40% 噪声、保留语音 |
| RTF | 0.015–0.036 | <1.0 即可实时 | ✅ 比实时快 28–66 倍 |

**崩溃根因坐实**：此前失败的 `libDF HEAD`（0.5.7-pre）依赖 tract `^0.21.4`，而 tract 0.21.4 在
native 有 codegen bug（`duplicate name /convt3/Conv.bias` + Conv kernel pack 后权重 NaN），连
官方 `deep-filter` bin 也崩。`v0.5.6` 的 tract `^0.19.4`（解析到 0.19.16）无此 bug。唯一补丁是
`time 0.3.28 → 0.3.44`（rustc 1.96 下 time 0.3.28 的 E0282 类型推断 bug，与 tract 无关）。

**VST3 参考佐证**：`DeepFilterNet3-VST3`（native cdylib 插件，macOS 生产可用）正是
`df = { git="...DeepFilterNet.git", tag="v0.5.6", features=["tract","default-model","transforms"] }`，
证明该组合在 native 可用。

---

## 1. 背景与目标

octopus 采集层已用 RNNoise（`nnnoiseless`）做实时环境降噪（`denoise.rs`）。DF3 是 48kHz 全频带
语音增强，质量优于 RNNoise，但此前因模型缺陷被搁置。spike 已证明官方原生路径可用。

**目标**：将 DF3 作为可选降噪后端整合，与 RNNoise 并存，由配置 `denoise_mode` 切换：

- `0` = 关闭降噪（直通）
- `1` = RNNoise（现状，默认）
- `2` = DeepFilterNet3

**非目标**：替换 RNNoise；改动采集层 audio pipeline 结构；改变 `DenoiseProcessor` 对外接口。

## 2. 范围

### 2.1 在范围内
- `DenoiseProcessor` 重构为 mode 分发器 + trait 后端（对外接口不变）。
- 新增 `Df3Backend`（包装 libDF `DfTract`）。
- 配置 `denoise_mode` 0/1/2 + 向后兼容旧 `denoise_enabled`。
- git 依赖 `deep_filter v0.5.6`（fork tag）+ time patch。
- 测试：RNNoise 回归 + DF3 gain/噪声抑制断言。

### 2.2 不在范围内
- DF3 的低延迟模型（`default-model-ll`）——后续可选。
- DF3 参数（attenuation limit / mix）暴露给用户——YAGNI，先用默认。
- 降噪后处理的可视化/调节 UI。
- 替换 ort——DF3 用 tract（流式 GRU 状态所需），与 ASR 的 ort 无关。

## 3. 模型与依赖选型

### 3.1 为什么是官方 libDF + tract（v0.5.6）

| 路径 | 状态 | 结论 |
|---|---|---|
| 第三方 ONNX `dfn3.onnx` + ort | 模型压语音 gain≈0.10 | ✗ 已弃用（上一版） |
| libDF HEAD(0.5.7-pre) + tract 0.21.4 | native codegen 崩（权重 NaN） | ✗ spike 失败 |
| **libDF v0.5.6 + tract 0.19.4** | **spike gain=0.958，RTF=0.015** | **✓ 采用** |

DF3 的流式 GRU 需要跨帧保持隐状态。tract 的 `PulsedModel` + `SimpleState` 原生支持（`DfTract::process`
每次喂一帧 hop，内部维护状态）。ort 无法等价复刻（需每帧重置或重新导出有状态 ONNX——即 dfn3 失败路）。
故 tract 非冗余，是 DF3 流式的必需。

### 3.2 依赖声明

```toml
# crates/asr/Cargo.toml
# 引用 octopus fork `tryternity/DeepFilterNet` tag v0.5.6（= commit `978576aa`，与上游官方
# `Rikorose/DeepFilterNet` v0.5.6 等价）。自控仓库避免上游删库/移 tag；精确 commit 由 Cargo.lock 锁定。
# 演进：初版用上游官方 Rikorose（fork 当时无 tag）；2026-06-17 在 fork 打同名 tag v0.5.6 后改回 fork。
df = { git = "https://github.com/tryternity/DeepFilterNet.git", tag = "v0.5.6",
       package = "deep_filter", default-features = false,
       features = ["tract", "default-model", "transforms"] }
```

- `default-features = false`：关闭 vorbis/flac（octopus 不解码 ogg/flac）。
- `default-model`：编译期内嵌 `DeepFilterNet3_onnx.tar.gz`（~7.9MB），无需运行时外部模型文件。
- `transforms`：提供 `resample`（octopus 采集已是 48k，实际不调用，但 libDF trait 约束需要）。

### 3.3 time patch

tract 0.19 间接依赖 `time`，默认解析到 `0.3.28`，在 rustc 1.96 下 E0282 编译失败。须确保 octopus
`Cargo.lock` 锁 `time ≥ 0.3.35`（规避 E0282 的最低版本）。

**实施阶段实际情况（2026-06-17）**：workspace 已有 `tauri → plist` 依赖链要求 `time ^0.3.47`，
Cargo.lock 解析到 `0.3.49`，**远高于 0.3.35 阈值**，故 DF3 引入后全程 `cargo check` 无任何 time
E0282 错误，**无需手动 `cargo update -p time`**。

**兜底（仅新克隆环境）**：若未来某环境 tauri/plist 链变动导致 time 解析到 `<0.3.35`，才需手动钉版本：

```bash
cargo update -p time --precise 0.3.36
```

若仅靠 lock 钉版本在 CI/新 clone 不可靠，则在 workspace 根 `Cargo.toml` 加
`time = "0.3.36"` 直接依赖约束固化（不实际使用，仅抬高最小版本）。

## 4. 架构

### 4.1 方案选择：trait 抽象（方案 A）

`DenoiseProcessor` 从"具体 RNNoise 实现"重构为"mode 分发器 + 可插拔后端"。对外接口
（`new` / `reset` / `process_samples` / `flush`）与 `Default` **完全不变**，audio.rs 仅把
`denoise_enabled` 读法换成 `denoise_mode`。

对比备选：enum 后端（process 内 match，重复分支）、双 Processor（audio.rs 按 mode 选，改动大）。
trait 方案改动最小、最易扩展。

### 4.2 数据流（不变）

```
cpal 回调 → samples(原生sr) → down_sampler → 48k s48k
  → DenoiseProcessor::process_samples(s48k)   ← mode 分发到 backend
  → enhanced_48k → resampler(48k→16k) → ASR
```

DF3/RNNoise 在边界同构：都是 hop=480 逐帧。`DfTract::process` 内部透明维护 GRU 状态 +
lookahead（延迟 `fft_size-hop + lookahead*hop = 960-480+2·480 = 1440` 样本 ≈ 30ms，由流式状态
与现有 `in_buf`/`out_buf` 累积 + `flush` 尾部补零天然吸收，pipeline 无感）。

## 5. 组件

### 5.1 `FrameDenoise` trait（新增）

```rust
/// 单帧（FRAME_SIZE=480，48k，i16 PCM 等价值域）降噪后端抽象。
/// 仅用原生 slice，不暴露 ndarray —— 隔离 libDF(0.15) 与 asr(0.17)。
trait FrameDenoise: Send + Sync {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]);
    /// 清状态（会话边界调用）。各 backend 自行决定轻量清零 vs 重建。
    fn reset(&mut self);
}
```

### 5.2 `RnnoiseBackend`（重构自现有实现）

包装 `nnnoiseless::DenoiseState<'static>`，impl `FrameDenoise`（逻辑即现有
`self.denoise.process_frame(out, pcm)`）。`reset` 重建 `DenoiseState`。

### 5.3 `Df3Backend`（新增）

```rust
use df::tract::DfTract;

pub struct Df3Backend(DfTract);

impl Df3Backend {
    pub fn new() -> Result<Self> {
        // DfTract::default() 加载内嵌 DeepFilterNet3（7.9MB + tract init）
        Ok(Self(DfTract::default()))
    }
}

impl FrameDenoise for Df3Backend {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]) {
        // 构造 libDF 的 ArrayView2 [1,480] / ArrayViewMut2 [1,480]
        // 调 self.0.process(noisy_view, enh_view_mut)
        // enh_view_mut → out（libDF 内部 ndarray 0.15，边界转换）
    }
}
```

### 5.4 `DenoiseProcessor` 重构

```rust
pub struct DenoiseProcessor {
    mode: DenoiseMode,                        // 决定 reset 时重建哪个 backend
    backend: Option<Box<dyn FrameDenoise>>,  // None = 直通(mode=0 或加载失败降级)
    in_buf: Vec<f32>,                         // 48k [-1,1] 累积输入
    out_buf: Vec<f32>,                        // 48k [-1,1] 已降噪待输出
}
```

`process_samples`：累积/分帧/PCM_SCALE 逻辑原样保留，核心改为
`if let Some(b) = self.backend.as_mut() { b.process_frame(&pcm, &mut out_frame) } else { out_frame = pcm /* 直通 */ }`。

`flush`：尾部补零逻辑不变（DF3/RNNoise 都按 FRAME_SIZE 补齐）。

`reset`：清 `in_buf`/`out_buf`；调 `backend.reset()`（trait 方法，各 backend 自实现）。
DF3 reset：实施时优先查 libDF 是否提供轻量状态重置（不重载权重）；若无，`Df3Backend::reset`
重建 `DfTract`（成本 = 重载 7.9MB，仅在会话边界 `start()` 调用可接受——VAD 段间不调 denoise
reset，与现有 RNNoise 语义一致，见 denoise.rs:35-36）。

## 6. Send/Sync 安全

`SharedAudioState` 经 `unsafe impl Send/Sync`（audio.rs:302-303）跨 cpal 回调/coordinator，
故 `DenoiseProcessor` 必须 `Send + Sync`（编译期断言 audio.rs:305-312）。

`DfTract` 含 `Arc<dyn RealToComplex<f32>>`（无 `+ Send`）→ `DfTract: !Send` → `Df3Backend: !Send`。
照搬 VST3（`plugin/src/lib.rs:9-11`）：

```rust
// 安全性论证（同 SharedAudioState）：
// - DenoiseProcessor 在 Mutex<Option<..>> 内（audio.rs:26），coordinator 单线程串行 lock+process
//   （audio.rs:94 注释：全在 coordinator 单线程串行调用，无跨线程并发访问）；
// - 实际不存在跨线程并发，unsafe impl 仅满足类型约束，不引入数据竞争。
unsafe impl Send for Df3Backend {}
unsafe impl Sync for Df3Backend {}
```

`RnnoiseBackend`（`Box<DenoiseState<'static>>`）天然 `Send + Sync`，无需 unsafe。

## 7. ndarray 版本隔离

- libDF（deep_filter）依赖 ndarray `0.15`；asr 现有 ndarray `0.17`（ort/whisper 等用）。
- Cargo 允许同 workspace 内 ndarray 0.15 与 0.17 共存（不同 major）。
- **隔离点**：`FrameDenoise` trait 方法只用 `&[f32]` / `&mut [f32]`，绝不暴露 ndarray 类型。
- `Df3Backend::process_frame` 内部用 libDF 的 ndarray 0.15 构造 `ArrayView2` 喂 `DfTract::process`，
  再从 `ArrayViewMut2` 取回 `&mut [f32]`。asr 的 0.17 类型完全不触及。

## 8. 配置

`AppConfig` / `DesktopConfig` 加 `denoise_mode: u8`，serde 默认 `1`（RNNoise，保持当前行为）。
向后兼容旧 `denoise_enabled: bool`：

- `denoise_mode` 存在 → 用它（0/1/2）。
- 缺失但 `denoise_enabled: true` → 映射为 `1`；`false` → `0`。
- 两者皆缺 → 默认 `1`。

audio.rs:98 `let denoise_on = cfg.denoise_enabled;` → 改读 `cfg.denoise_mode`，按 mode 构造 backend。
audio.rs:211-213 `DenoiseProcessor::new()` → `DenoiseProcessor::new(mode)`。

> **实施修正（2026-06-17 合并后）**：上述「向后兼容旧 `denoise_enabled`」逻辑**最终未保留**。
> 合并时发现 main 已独立引入 `denoise_mode: u8`（接工具栏 `set_denoise_mode` 命令 + 持久化），
> 与本设计的 `Option<u8>` + `effective_denoise_mode()` 向后兼容方案冲突。经决策**以 main 的
> `denoise_mode: u8`（固定默认 `1`）为唯一真相**，删除：
> - feature 的 `Option<u8>` 字段与 `effective_denoise_mode()` 方法；
> - 旧 `denoise_enabled: bool` 字段本身（彻底移除，不再保留作回退）。
>
> 现状：audio.rs 直接读 `cfg.denoise_mode`；`default_denoise_mode() = 1`；旧 config.yaml 里残留的
> `denoise_enabled` 被 serde 静默忽略（`AppConfig` 无 `deny_unknown_fields`），不影响解析。

## 9. 懒加载与降级

**懒加载**（mode=2）：

- `DenoiseProcessor::new(mode=2)`：backend 先留 `None` 占位，**不立即加载 DfTract**。
- 首次 `process_samples` 时才 `Df3Backend::new()`（加载 7.9MB + tract init）。
- mode=0 → backend 永远 None（直通）；mode=1 → 构造 `RnnoiseBackend`（同现状）。

**降级**（沿用 audio.rs:88-89 现有语义）：DF3 加载失败 / 单帧推理失败 → warn 日志 + backend 置 None
→ 直通，绝不 panic、绝不阻断录音。start() 在非实时路径（audio.rs:211），首帧加载延迟可接受。

### 模型加载日志（DF3 特有）

tract 加载 DF3 模型时会刷出极大量 DEBUG 日志（`tract_core::optim` 的 `applying patch`
数百行、`tract_hir::infer` 的 `Refined` / `Can't infer shape` 等），且 `df::tract` 自身也打
`Info`/`Debug`（`Init encoder` / `Start init ERB decoder` / `ERB decoder input:` 等），严重
污染 octopus 启动日志。`crates/desktop/src/main.rs` 的 `tauri_plugin_log::Builder`（全局
`level(Debug)`）对这些 target 一律 `level_for(Warn)`：`tract_core` / `tract_hir` /
`tract_onnx` / `tract_linalg` / `df::tract`。

> **2026-06-17 修订**：初版曾有意保留 `df::tract` 的 `Info` 作加载进度信号，实测其 `Info`/`Debug`
> 仍刷屏（`ERB decoder input:` 等），改为一并压到 `Warn`。RNNoise 无 tract 依赖，不受此策略影响。

## 10. 测试策略

**RNNoise 回归**（mode=1）：现有 `denoise.rs` 测试（`processor_basic_roundtrip` /
`length_invariant_within_one_frame` / `streaming_incremental_equals_batch` /
`diag_*`）保持全绿，验证 trait 重构未破坏 RNNoise。

**DF3 新增**（mode=2）：

- 长度守恒：输入 N → process+flush 输出与 N 差 < FRAME_SIZE（同 RNNoise 断言）。
- 干净语音 gain ≥ 0.5（反 dfn3 压语音回归；spike 实测 0.958）。
- 噪声抑制：纯白噪声 → out_rms < in_rms（同 `diag_pure_noise_suppressed` 思路）。
- 用 spike 已验证资产（`assets/clean_freesound_33711.wav` gain≈0.96、`noisy_snr0.wav` gain≈0.60）
  作断言基准。DF3 加载耗资源，DF3 测试加 `#[ignore]` 或独立 feature gate，避免拖慢常规 `cargo test`。

**⚠ DF3 测试输入必须用真实语音**（Task 4 实施发现，2026-06-17）：

DF3 的「干净语音 gain」断言**不能用合成稳态谐波**（如现有 `synth_speech` 的简单正弦叠加），**必须用真实
语音 wav**（如 `/tmp/voice48k.wav` TTS 输出或真实录音）。原因：DF3 训练于真实语音的时频动态，会把恒幅
稳态谐波（持续不变的单一频率/简单谐波叠加）**正确识别为非语音信号**（类啸叫/feedback）并压制——这不是
缺陷，而是 DF3 的设计目标（啸叫抑制）。实测对比：

| 输入 | gain | 判定 |
|---|---|---|
| 合成稳态谐波（`synth_speech`） | **≈0.005** | DF3 当稳态噪声压掉（比 dfn3 缺陷 0.10 还低！） |
| 真实语音 `/tmp/voice48k.wav` | **≈0.999** | 正常保留（spike 真实音频 0.958） |

合成谐波对 DF3 是**固有代理失真**（proxy distortion），**不是「DF3 压语音」回归**。用合成谐波测 DF3 会得到
假阳性失败。RNNoise 用频带能量特征（不依赖时频动态建模），合成谐波测试对它有效（gain≥0.5），故 RNNoise
测试可继续用 `synth_speech`。

**实践**：DF3 gain 断言的输入源用真实 wav 文件路径（如 `/tmp/voice48k.wav`，测试中 `hound::WavReader`
读取）；若文件不存在则 `#[ignore]` 跳过（避免 CI 缺资产失败）。

**Send 守护**：audio.rs:312 编译期 `_assert_send_sync::<DenoiseProcessor>()` 继续生效——
验证 `Df3Backend` 的 unsafe impl 没破坏 `DenoiseProcessor: Send + Sync`。

## 11. 验收标准

- [x] `cargo check --workspace --all-targets` 通过（ndarray 0.15/0.17 共存，time patch 生效）。
- [x] mode=1：所有现有 `denoise.rs` 测试全绿（RNNoise 行为不变）。
- [x] mode=2：DF3 测试通过（gain/噪声抑制/长度守恒）。
- [x] mode=0：直通，输出 = 输入。
- [x] 配置：`denoise_mode: 2` 加载 DF3；`denoise_mode: 1`（缺省）RNNoise；`denoise_mode: 0` 直通（旧 `denoise_enabled` 已删除，详见 §8 实施修正）。
- [x] 手动 e2e：备份 `~/.octopus/` 后，`denoise_mode: 2` 录音 → ASR 不退化（DF3 不压语音）。
- [x] Send 断言编译通过。

## 12. 关键文件

- `crates/asr/Cargo.toml`：加 `df` git 依赖（v0.5.6）+ 可选 time 约束。
- `crates/asr/src/denoise.rs`：`FrameDenoise` trait + `RnnoiseBackend` + `Df3Backend` + `DenoiseProcessor` 重构。
- `crates/desktop/src/audio.rs`：`denoise_enabled` → `denoise_mode`（:98 读、:211 构造）。
- `crates/infra/src/config.rs`：`denoise_mode: u8` 字段 + `default_denoise_mode()`（旧 `denoise_enabled` 已删除，详见 §8 实施修正）。
- workspace `Cargo.toml` / `Cargo.lock`：time patch。

## 13. 历史与关联

- 前作：[`2026-06-16-denoise-deepfilternet-design.md`](./2026-06-16-archived-design.md)
  （dfn3.onnx 弃用记录、RNNoise 现状）。
- spike 证据：`DeepFilterNet/libDF/examples/verify_gain.rs`（fork worktree）。
- 参考：`DeepFilterNet3-VST3/plugin/src/lib.rs`（Send 解法、v0.5.6 依赖范本）。
