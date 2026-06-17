# DeepFilterNet3 原生降噪整合 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 DeepFilterNet3（官方 libDF v0.5.6 + tract 0.19）作为 `denoise_mode=2` 整合进采集层，与现有 RNNoise（mode=1）并存，`DenoiseProcessor` 对外接口不变。

**Architecture:** `DenoiseProcessor` 重构为 trait 后端分发器——新增 `FrameDenoise` trait（`[-1,1]` 契约），`RnnoiseBackend` 包装现有 nnnoiseless，`Df3Backend` 包装 libDF `DfTract`（`unsafe impl Send/Sync` 照 VST3）。ndarray 0.15（libDF）与 0.17（asr）靠 trait 的 slice 边界隔离。spike 已验证 v0.5.6+tract 0.19 在 native gain=0.958（不压语音）、RTF=0.015。

**Tech Stack:** Rust、libDF(deep_filter v0.5.6)、tract 0.19、ndarray 0.15/0.17 共存、nnnoiseless、serde(config)。

参考 spec：`docs/superpowers/specs/2026-06-17-denoise-deepfilternet3-integration-design.md`。

---

## File Structure

| 文件 | 责任 | 操作 |
|---|---|---|
| `crates/asr/Cargo.toml` | 加 `df` git 依赖 + `ndarray_015` rename | 改 |
| `crates/asr/src/denoise.rs` | `FrameDenoise` trait + `DenoiseMode` + `RnnoiseBackend` + `Df3Backend` + `DenoiseProcessor` 重构 + 测试 | 改 |
| `crates/infra/src/config.rs` | `denoise_mode: Option<u8>` + `effective_denoise_mode()` + 测试 | 改 |
| `crates/desktop/src/audio.rs` | `denoise_enabled` → `effective_denoise_mode()`（:98 读、:211 构造） | 改 |
| `docs/configuration.md`、`docs/architecture.md` | denoise_mode 说明 | 改 |

**隔离边界**：`FrameDenoise::process_frame(pcm: &[f32;480], out: &mut [f32;480])` 只用原生 slice，绝不暴露 ndarray——asr 的 0.17 与 libDF 的 0.15 在此隔离。`Df3Backend` 内部用 `ndarray_015` 构造 `ArrayView2` 喂 `DfTract::process`。

**测试策略**：现有 `denoise.rs` 测试保持全绿（mode=Rnnoise 回归）；DF3 功能测试加 `#[ignore]`（需加载 7.9MB 模型，慢，手动 `cargo test -- --ignored`）；Send 正确性由 `audio.rs:312` 编译期断言守护。

---

## Task 1: 加 libDF 依赖 + ndarray 0.15 隔离 + time patch

**Files:**
- Modify: `crates/asr/Cargo.toml`
- 注：`Cargo.lock` 由 cargo update 生成但**仓库不跟踪**（既有策略），不进 commit。

**背景**：tract 0.19 间接拉 `time`，默认解析到 0.3.28，在 rustc 1.96 下 E0282 编译失败。必须先升 time 再 check。ndarray 0.15 与现有 0.17 共存靠 package rename。

- [x] **Step 1: 改 `crates/asr/Cargo.toml`，加 df 与 ndarray_015**

在 `nnnoiseless = { version = "0.5", default-features = false }`（约 :25）下方插入：

```toml
# DeepFilterNet3 原生降噪（libDF v0.5.6 + tract 0.19，spec 2026-06-17）。
# ndarray 0.15（libDF 版本）与上方 0.17（ort/asr）共存：rename 隔离，Df3Backend 边界转换。
ndarray_015 = { package = "ndarray", version = "0.15", default-features = false }
# df URL：原设计写 fork tryternity，实测该 fork 无 tag（git ls-remote --tags 空），
# 改用上游官方 Rikorose/DeepFilterNet tag v0.5.6（commit 978576aa，与 fork 同 commit 等价）。
df = { git = "https://github.com/Rikorose/DeepFilterNet.git", tag = "v0.5.6", package = "deep_filter", default-features = false, features = ["tract", "default-model", "transforms"] }
```

- [x] **Step 2: time 版本检查（通常无需手动 patch）**

tract 0.19 间接拉 `time`，rustc 1.96 下若解析到 `0.3.28` 会 E0282 失败。但 octopus workspace 已有
`tauri → plist` 链要求 `time ^0.3.47`（Cargo.lock 解析到 `0.3.49`），**已远高于规避 E0282 的 0.3.35
阈值**，故引入 df 后通常无需任何手动 time patch。

先检查（仓库根）:
```bash
grep -A1 '^name = "time"' Cargo.lock
```
Expected: `version = "0.3.47"` 或更高（≥0.3.35 即可）。

**仅当**解析到 `<0.3.35` 时（如未来 tauri/plist 链变动）才需手动钉版本:
```bash
cargo update -p time --precise 0.3.36
```
Expected: `Updating time v0.3.x -> v0.3.36`（或 "Already up to date" 若已 ≥0.3.36）。若无输出报错
`Package does not feature`，先 `cargo fetch` 再重试。

- [x] **Step 3: 验证 asr 编译（首次拉 df + 编译 tract，约 1–3 分钟）**

Run:
```bash
cargo check -p octopus-asr
```
Expected: `Finished` 无错误。
诊断：
- 若报 `time ... E0282 type annotations needed` → time 未升级，回 Step 2。
- 若报 ndarray 版本冲突 → 确认 `ndarray_015` 用了 `package = "ndarray"` rename。
- 若报 df git 拉取失败 → 确认网络/上游 tag `v0.5.6` 存在（`git ls-remote --tags https://github.com/Rikorose/DeepFilterNet.git v0.5.6`）。注意：**用上游 Rikorose 不是 fork tryternity**（后者无 tag）。

- [x] **Step 4: 提交**

> 注：仓库不跟踪 `Cargo.lock`（既有策略），故只 add `Cargo.toml`。

```bash
git add crates/asr/Cargo.toml
git commit -m "build(asr): 加 libDF v0.5.6 依赖 + ndarray_0.15 隔离 + time patch"
```

---

## Task 2: config 加 denoise_mode + effective_denoise_mode()

> ⚠️ **实施修正（2026-06-17 合并后）**：本 Task 原设计 `denoise_mode: Option<u8>` + `effective_denoise_mode()`（向后兼容旧 `denoise_enabled`）。DF3 分支合并时与 main 工具栏的 `u8` 版本语义冲突（`git merge-tree` 未报文本冲突、却留下重复字段），经决策**统一为 main 的 `denoise_mode: u8`**——`Option<u8>` 字段与 `effective_denoise_mode()` **未保留**，旧 `denoise_enabled` 字段彻底删除。下方 Step 1–5 记录的是原设计步骤（历史执行轨迹）；最终落盘代码见 `crates/infra/src/config.rs`（`denoise_mode: u8` + `default_denoise_mode()=1`）。完整决策动机详见 spec §8「实施修正（2026-06-17 合并后）」。

**Files:**
- Modify: `crates/infra/src/config.rs:143-145`（字段）、`config.rs`（impl 块加方法）、`config.rs:230`（Default）、`config.rs:324-332`（测试）

**背景**：`denoise_enabled: bool`（默认 true）在 `infra/src/config.rs:144`。新增 `denoise_mode: Option<u8>`（None=未配置）。`effective_denoise_mode()`：mode 显式优先，否则 `denoise_enabled` 映射（true→1, false→0），实现旧配置向后兼容。

- [x] **Step 1: 写失败测试（先于实现）**

在 `crates/infra/src/config.rs` 测试模块（`denoise_enabled_override_from_yaml` 测试附近，约 :330）后追加：

```rust
    #[test]
    fn denoise_mode_explicit_wins() {
        let cfg: AppConfig =
            serde_yaml::from_str("denoise_mode: 2\ndenoise_enabled: false\n").unwrap();
        assert_eq!(cfg.effective_denoise_mode(), 2);
    }

    #[test]
    fn denoise_mode_absent_falls_back_to_enabled() {
        let cfg: AppConfig = serde_yaml::from_str("denoise_enabled: true\n").unwrap();
        assert_eq!(cfg.effective_denoise_mode(), 1);
        let cfg: AppConfig = serde_yaml::from_str("denoise_enabled: false\n").unwrap();
        assert_eq!(cfg.effective_denoise_mode(), 0);
    }

    #[test]
    fn denoise_mode_absent_defaults_to_rnnoise() {
        let cfg: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(cfg.effective_denoise_mode(), 1);
    }
```

- [x] **Step 2: 跑测试确认失败**

Run:
```bash
cargo test -p octopus-infra denoise_mode 2>&1 | tail -15
```
Expected: 编译失败 `no field or method effective_denoise_mode` / `no field denoise_mode`。

- [x] **Step 3: 加字段 + 默认函数**

改 `crates/infra/src/config.rs:143-145`，在 `denoise_enabled` 字段**之后**插入 `denoise_mode`：

```rust
    /// 是否启用 RNNoise 环境降噪（录音送 ASR 前降噪）
    #[serde(default = "default_denoise_enabled")]
    pub denoise_enabled: bool,

    /// 环境降噪模式：0=关闭，1=RNNoise（默认），2=DeepFilterNet3。
    /// None=未配置 → 回退看 denoise_enabled（向后兼容旧配置）。
    #[serde(default)]
    pub denoise_mode: Option<u8>,
```

- [x] **Step 4: 加 effective_denoise_mode() 方法 + Default 初始化**

先定位 AppConfig 的 impl 块与 Default：
```bash
grep -n "impl AppConfig\|impl Default for AppConfig\|denoise_enabled: default_denoise_enabled" crates/infra/src/config.rs
```

在 `impl AppConfig { ... }` 块内（若无 impl 块则新增 `impl AppConfig { ... }`）加方法：

```rust
    /// 解析最终降噪模式（denoise_mode 显式优先，否则 denoise_enabled 映射）。
    /// 0=关闭，1=RNNoise，2=DeepFilterNet3。
    pub fn effective_denoise_mode(&self) -> u8 {
        if let Some(m) = self.denoise_mode {
            return m;
        }
        if self.denoise_enabled {
            1
        } else {
            0
        }
    }
```

在 `impl Default for AppConfig` 的构造体里 `denoise_enabled: default_denoise_enabled(),`（约 :230）下方加：

```rust
            denoise_mode: None,
```

- [x] **Step 5: 跑测试确认通过**

Run:
```bash
cargo test -p octopus-infra denoise 2>&1 | tail -15
```
Expected: `denoise_mode_explicit_wins ... ok`、`denoise_mode_absent_falls_back_to_enabled ... ok`、`denoise_mode_absent_defaults_to_rnnoise ... ok`、现有 `denoise_enabled_*` 仍 ok。

- [x] **Step 6: 提交**

```bash
git add crates/infra/src/config.rs
git commit -m "feat(config): denoise_mode 0/1/2 + effective_denoise_mode 向后兼容"
```

---

## Task 3: FrameDenoise trait + RnnoiseBackend + Df3Backend + DenoiseProcessor 重构

**Files:**
- Modify: `crates/asr/src/denoise.rs`（整体重构，含两后端 + 分发器，保持现有测试绿）

**背景**：`DenoiseProcessor` 从直接持有 nnnoiseless 改为 trait 后端分发。trait 用 `[-1,1]` 契约，PCM_SCALE 下沉到 `RnnoiseBackend`。`Df3Backend`（依赖 Task 1 的 df）一并定义，使 `DenoiseProcessor` 的 mode=Df3 分支可编译。现有测试调 `DenoiseProcessor::new()` → 改 `new(DenoiseMode::Rnnoise)`。

**注意**：本 task 一次写完 trait + 两后端 + processor，否则 `DenoiseProcessor` 引用 `Df3Backend` 无法编译。

- [x] **Step 1: 重写 `crates/asr/src/denoise.rs` 的非测试部分**

替换文件顶部模块文档到 `impl Default for DenoiseProcessor` 之前（含模块文档 + 常量 + 枚举 + trait + 两后端 + DenoiseProcessor 结构 + impl + Default）。新内容：

```rust
//! 环境降噪：可插拔后端（RNNoise / DeepFilterNet3），由 denoise_mode 选择。
//!
//! ## 后端
//! - `RnnoiseBackend`（mode=1）：nnnoiseless（Xiph RNNoise 纯 Rust 移植），内置默认模型。
//! - `Df3Backend`（mode=2）：libDF v0.5.6 + tract 0.19，DeepFilterNet3，48kHz 全频带。
//! - mode=0：无后端（直通）。
//!
//! ## 契约
//! `FrameDenoise::process_frame` 用 `[-1, 1]` 归一化单声道（与 octopus pipeline 一致）。
//! 各后端内部按模型需求转换（RNNoise 转 i16 PCM 等价；DF3 直接喂 [-1,1]）。
//! 帧大小 FRAME_SIZE=480（10ms @48kHz），与 octopus HOP 一致。
//!
//! ## 历史
//! 曾用第三方 dfn3.onnx（压语音 gain≈0.10），已弃用。见
//! `docs/superpowers/specs/2026-06-17-denoise-deepfilternet3-integration-design.md`。

use anyhow::Result;

/// 帧大小（480 样本 = 10ms @48kHz）。
const FRAME_SIZE: usize = 480;

/// 降噪模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenoiseMode {
    Off = 0,
    Rnnoise = 1,
    Df3 = 2,
}

impl DenoiseMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Off,
            1 => Self::Rnnoise,
            _ => Self::Df3,
        }
    }
}

/// 单帧（FRAME_SIZE，48k，[-1,1]）降噪后端抽象。
///
/// 仅用原生 slice，不暴露 ndarray——隔离 libDF(ndarray 0.15) 与 asr(ndarray 0.17)。
/// `Send + Sync`：`DenoiseProcessor` 经 `Mutex` 在 SharedAudioState 跨线程（audio.rs:305 断言）。
trait FrameDenoise: Send + Sync {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]);
    /// 清状态（会话边界）。各后端自行决定轻量清零 vs 重建。
    fn reset(&mut self);
}

// ── RNNoise 后端 ──

/// nnnoiseless 内部以 i16 PCM 等价值域运算；边界 [-1,1] ↔ PCM 转换在此。
const PCM_SCALE: f32 = 32768.0;

struct RnnoiseBackend {
    denoise: Box<nnnoiseless::DenoiseState<'static>>,
}

impl RnnoiseBackend {
    fn new() -> Self {
        Self {
            denoise: nnnoiseless::DenoiseState::new(),
        }
    }
}

impl FrameDenoise for RnnoiseBackend {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]) {
        let pcm_scaled: [f32; FRAME_SIZE] = std::array::from_fn(|i| pcm[i] * PCM_SCALE);
        self.denoise.process_frame(out, &pcm_scaled);
        // nnnoiseless 输出沿用输入值域（i16 PCM 等价），转回 [-1,1]
        for s in out.iter_mut() {
            *s /= PCM_SCALE;
        }
    }
    fn reset(&mut self) {
        self.denoise = nnnoiseless::DenoiseState::new();
    }
}

// ── DeepFilterNet3 后端（libDF v0.5.6 + tract 0.19）──

use df::tract::DfTract;

/// DeepFilterNet3 降噪后端。包装 libDF `DfTract`（48kHz 全频带，内嵌 DeepFilterNet3 模型）。
///
/// `DfTract: !Send`（含 `Arc<dyn RealToComplex>` 无 Send bound）。此处 unsafe impl 仅满足
/// `DenoiseProcessor: Send`（audio.rs:312 断言）的类型约束——实际由 coordinator 单线程串行
/// 访问（audio.rs:94），无跨线程并发。同 VST3 plugin/src/lib.rs:9-11。
pub struct Df3Backend(DfTract);

impl Df3Backend {
    /// 加载内嵌 DeepFilterNet3 模型。失败返回 Err（供懒加载降级，绝不 panic）。
    pub fn new() -> Result<Self> {
        let model = std::panic::catch_unwind(std::panic::AssertUnwindSafe(DfTract::default))
            .map_err(|e| anyhow::anyhow!("DF3 模型加载失败（panic）: {:?}", e))?;
        Ok(Self(model))
    }
}

// 安全性：coordinator 单线程串行访问（audio.rs:94），Mutex 保护，无跨线程并发。
unsafe impl Send for Df3Backend {}
unsafe impl Sync for Df3Backend {}

impl FrameDenoise for Df3Backend {
    fn process_frame(&mut self, pcm: &[f32; FRAME_SIZE], out: &mut [f32; FRAME_SIZE]) {
        // DfTract::process 接 ndarray 0.15 的 ArrayView2/ArrayViewMut2 [ch=1, hop]。
        // 用 ndarray_015（与 libDF 同一 crate 实例）构造；契约 [-1,1]（DfTract 期望归一化）。
        use ndarray_015::{ArrayView2, ArrayViewMut2};
        let noisy = match ArrayView2::from_shape((1, FRAME_SIZE), pcm.as_slice()) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("DF3 frame shape 错误，直通：{:?}", e);
                out.copy_from_slice(pcm);
                return;
            }
        };
        let mut enh = match ArrayViewMut2::from_shape((1, FRAME_SIZE), out.as_mut_slice()) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("DF3 enh shape 错误，直通：{:?}", e);
                return;
            }
        };
        if let Err(e) = self.0.process(noisy, enh.view_mut()) {
            log::warn!("DF3 process 失败，本帧直通：{:?}", e);
        }
    }
    fn reset(&mut self) {
        // DfTract 无轻量状态重置；重建 = 重载模型（仅会话边界调用）。
        match Self::new() {
            Ok(b) => *self = b,
            Err(e) => log::warn!("DF3 reset 重载失败：{:?}", e),
        }
    }
}

// ── DenoiseProcessor（mode 分发器）──

/// 流式降噪处理器。对外接口与旧 RNNoise-only 实现一致（new/reset/process_samples/flush）。
pub struct DenoiseProcessor {
    mode: DenoiseMode,
    backend: Option<Box<dyn FrameDenoise>>, // None = 直通(mode=0 或加载失败降级)
    in_buf: Vec<f32>,  // 48k [-1,1] 累积输入
    out_buf: Vec<f32>, // 48k [-1,1] 已降噪待输出
    df_pending: bool,  // DF3 懒加载：mode=Df3 但尚未首次 process
}

impl DenoiseProcessor {
    /// 按 mode 创建降噪器。mode=Df3 时延迟到首次 process_samples 加载（避免 new 热路径开销）。
    pub fn new(mode: DenoiseMode) -> Result<Self> {
        let mut p = Self {
            mode,
            backend: None,
            in_buf: Vec::with_capacity(FRAME_SIZE),
            out_buf: Vec::new(),
            df_pending: false,
        };
        match mode {
            DenoiseMode::Off => {}
            DenoiseMode::Rnnoise => {
                p.backend = Some(Box::new(RnnoiseBackend::new()));
            }
            DenoiseMode::Df3 => {
                p.df_pending = true; // 懒加载
            }
        }
        Ok(p)
    }

    /// 全状态清零（重建后端）。DF3 reset 重载模型——仅会话边界调用。
    pub fn reset(&mut self) {
        self.in_buf.clear();
        self.out_buf.clear();
        match self.mode {
            DenoiseMode::Off => self.backend = None,
            DenoiseMode::Rnnoise => self.backend = Some(Box::new(RnnoiseBackend::new())),
            DenoiseMode::Df3 => {
                self.backend = match Df3Backend::new() {
                    Ok(b) => Some(Box::new(b)),
                    Err(e) => {
                        log::warn!("DF3 reset 重建失败，降级直通：{:?}", e);
                        None
                    }
                };
                self.df_pending = false;
            }
        }
    }

    /// 增量处理 48k [-1,1] 样本：累积到 FRAME_SIZE，逐帧降噪，返回已降噪样本。
    pub fn process_samples(&mut self, samples: &[f32]) -> Vec<f32> {
        if self.df_pending {
            self.backend = match Df3Backend::new() {
                Ok(b) => Some(Box::new(b)),
                Err(e) => {
                    log::warn!("DF3 模型加载失败，降级直通（不阻断录音）：{:?}", e);
                    None
                }
            };
            self.df_pending = false;
        }
        self.in_buf.extend_from_slice(samples);
        let mut out_frame = [0.0f32; FRAME_SIZE];
        while self.in_buf.len() >= FRAME_SIZE {
            let frame: Vec<f32> = self.in_buf.drain(..FRAME_SIZE).collect();
            let pcm: [f32; FRAME_SIZE] = std::array::from_fn(|i| frame[i]);
            if let Some(b) = self.backend.as_mut() {
                b.process_frame(&pcm, &mut out_frame);
                for &s in &out_frame {
                    self.out_buf.push(s);
                }
            } else {
                for &s in &pcm {
                    self.out_buf.push(s); // 直通
                }
            }
        }
        std::mem::take(&mut self.out_buf)
    }

    /// 尾部 flush：零填残差到 FRAME_SIZE，处理一帧排出尾部。
    pub fn flush(&mut self) -> Vec<f32> {
        if !self.in_buf.is_empty() {
            self.in_buf.resize(FRAME_SIZE, 0.0);
            let pcm: [f32; FRAME_SIZE] = std::array::from_fn(|i| self.in_buf[i]);
            let mut out_frame = [0.0f32; FRAME_SIZE];
            if let Some(b) = self.backend.as_mut() {
                b.process_frame(&pcm, &mut out_frame);
                for &s in &out_frame {
                    self.out_buf.push(s);
                }
            } else {
                for &s in &pcm {
                    self.out_buf.push(s);
                }
            }
            self.in_buf.clear();
        }
        std::mem::take(&mut self.out_buf)
    }
}

impl Default for DenoiseProcessor {
    fn default() -> Self {
        Self::new(DenoiseMode::Rnnoise).expect("RNNoise new 仅在 OOM 失败")
    }
}
```

- [x] **Step 2: 改现有测试的 `new()` 调用为 `new(DenoiseMode::Rnnoise)`**

在 `crates/asr/src/denoise.rs` 测试模块，把所有 `DenoiseProcessor::new().unwrap()` 改为 `DenoiseProcessor::new(DenoiseMode::Rnnoise).unwrap()`。涉及：`processor_basic_roundtrip`、`length_invariant_within_one_frame`、`streaming_incremental_equals_batch`、`diag_pure_noise_suppressed`、`diag_clean_speech_preserved`、`diag_silence_output`、`diag_denoise_tts_wav`、`diag_real_speech_noisy_denoise_effect`。

- [x] **Step 3: 跑测试确认 RNNoise 回归全绿**

Run:
```bash
cargo test -p octopus-asr --lib denoise 2>&1 | tail -25
```
Expected: 所有非 `#[ignore]` 测试 ok。
诊断：若 `streaming_incremental_equals_batch` 失败（max_diff≠0）→ 检查 `RnnoiseBackend::process_frame` 的 PCM_SCALE 双向转换（`*PCM_SCALE` 喂、`/PCM_SCALE` 收）。

- [x] **Step 4: 验证 Send 断言（含 Df3Backend 的 unsafe impl）**

Run:
```bash
cargo check -p octopus-desktop 2>&1 | tail -10
```
Expected: `Finished`（`audio.rs:312` 的 `_assert_send_sync::<DenoiseProcessor>()` 通过——RnnoiseBackend 天然 Send，Df3Backend 经 unsafe impl 满足）。

- [x] **Step 5: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "refactor(asr): FrameDenoise trait + RnnoiseBackend + Df3Backend + DenoiseProcessor 重构"
```

---

## Task 4: Df3Backend 行为测试（验证不压语音 / 噪声抑制）

**Files:**
- Modify: `crates/asr/src/denoise.rs`（测试模块追加 DF3 测试）

**背景**：Df3Backend 已在 Task 3 实现。本 task 验证其行为：长度守恒、不压语音（gain≥0.5，反 dfn3 回归）、噪声抑制。DF3 测试需加载 7.9MB 模型，加 `#[ignore]` 手动跑。

> **⚠ DF3 测试输入必须用真实语音**（Task 4 实施发现）：DF3 训练于真实语音时频动态，把恒幅稳态谐波
> （如 `synth_speech` 简单正弦叠加）正确识别为非语音（类啸叫/feedback）并压制——实测合成谐波
> gain≈0.005（比 dfn3 缺陷 0.10 还低！），真实语音 gain≈0.999（spike 真实音频 0.958）。故「不压语音」
> gain 断言**不能用 `synth_speech`**，必须用真实 wav（如 `/tmp/voice48k.wav` TTS 或真实录音，
> `hound::WavReader` 读取；文件不存在则 `#[ignore]` 跳过）。合成谐波对 DF3 是固有代理失真，非「压语音」
> 回归。RNNoise 用频带能量特征，合成谐波测试对它有效（gain≥0.5），故 RNNoise 测试可继续用 `synth_speech`。

- [x] **Step 1: 写 DF3 测试**

在 `crates/asr/src/denoise.rs` 测试模块末尾追加（均 `#[ignore]`，复用现有 `white_noise`/`rms` helper；
**真实语音输入用 `read_wav_48k` helper 读 `/tmp/voice48k.wav`**——见上方背景说明，不能用 `synth_speech`）：

先加真实语音读取 helper（若测试模块尚无）：
```rust
    /// 读取 /tmp/voice48k.wav（48k mono i16）→ [-1,1] f32。
    fn read_wav_48k() -> Vec<f32> {
        let mut reader = hound::WavReader::open("/tmp/voice48k.wav").expect("/tmp/voice48k.wav");
        reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect()
    }
```

生成 `/tmp/voice48k.wav`（macOS，48k mono i16）：
```bash
say -o /tmp/voice.aiff "这是一段用于降噪测试的真实中文语音，包含正常语速与停顿。" \
  && ffmpeg -y -i /tmp/voice.aiff -ar 48000 -ac 1 -sample_fmt s16 /tmp/voice48k.wav
```

然后追加测试：

```rust
    // ── DF3 后端测试（需加载 7.9MB 模型，慢，手动 cargo test -- --ignored）──

    /// DF3 加载 + 长度守恒（同 RNNoise 断言）。
    #[test]
    #[ignore]
    fn df3_length_invariant() {
        for &n in &[480usize, 481, 960, 4800] {
            let mut p = DenoiseProcessor::new(DenoiseMode::Df3).unwrap();
            let input: Vec<f32> = (0..n)
                .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.3)
                .collect();
            let mut out = p.process_samples(&input);
            out.extend(p.flush());
            let diff = (out.len() as i64 - n as i64).abs();
            assert!(diff < FRAME_SIZE as i64, "n={n} out={} diff={diff}", out.len());
        }
    }

    /// DF3 不压语音：**必须用真实语音 `/tmp/voice48k.wav`**（非 synth_speech）。
    /// 真实语音 gain 应 ≥0.5（spike 实测 0.96，实施实测 0.999）。
    /// 原因：DF3 把 synth_speech 的稳态谐波当非语音（类啸叫）压制（gain≈0.005），是代理失真非缺陷。
    #[test]
    #[ignore] // 需 /tmp/voice48k.wav
    fn df3_clean_speech_preserved() {
        let input = read_wav_48k();
        let n = input.len();
        let mut p = DenoiseProcessor::new(DenoiseMode::Df3).unwrap();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        let lo = FRAME_SIZE * 2;
        let hi = n - FRAME_SIZE * 2;
        let in_rms = rms(&input, lo, hi);
        let out_rms = rms(&out, lo, hi);
        let gain = out_rms / in_rms.max(1e-12);
        eprintln!("DIAG df3_clean: gain={:.3}（真实语音，应 ≥0.5；dfn3 缺陷≈0.10）", gain);
        assert!(gain >= 0.5, "DF3 压语音：gain={:.3}", gain);
    }

    /// DF3 抑制噪声：纯白噪声 out_rms < in_rms。
    #[test]
    #[ignore]
    fn df3_noise_suppressed() {
        let n = 48000 * 3;
        let input = white_noise(n, 0.1);
        let mut p = DenoiseProcessor::new(DenoiseMode::Df3).unwrap();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        let lo = FRAME_SIZE * 100;
        let hi = n - FRAME_SIZE * 2;
        let in_rms = rms(&input, lo, hi);
        let out_rms = rms(&out, lo, hi);
        eprintln!("DIAG df3_noise: in_rms={:.4} out_rms={:.4}", in_rms, out_rms);
        assert!(out_rms < in_rms, "DF3 未抑制噪声：out={:.4} in={:.4}", out_rms, in_rms);
    }

    /// 诊断：合成谐波被 DF3 压制的对照（**仅打印 gain，不断言**）。
    /// 用以记录「合成稳态谐波 → DF3 gain≈0.005」这一代理失真现象，警示勿用合成语音测 DF3。
    #[test]
    #[ignore]
    fn df3_synth_speech_gain_diag() {
        let n = 48000 * 2;
        let input = synth_speech(n);
        let mut p = DenoiseProcessor::new(DenoiseMode::Df3).unwrap();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        let lo = FRAME_SIZE * 2;
        let hi = n - FRAME_SIZE * 2;
        let gain = rms(&out, lo, hi) / rms(&input, lo, hi).max(1e-12);
        eprintln!("DIAG df3_synth: gain={:.3}（合成稳态谐波；DF3 应压低 ~0.005，非缺陷）", gain);
    }
```

- [x] **Step 2: 跑 DF3 测试（手动，加载模型慢）**

Run:
```bash
cargo test -p octopus-asr --lib denoise -- --ignored 2>&1 | tail -25
```
Expected: `df3_length_invariant ... ok`、`df3_clean_speech_preserved ... ok`（真实语音 gain≥0.5，
实测≈0.999）、`df3_noise_suppressed ... ok`、`df3_synth_speech_gain_diag` 仅打印（gain≈0.005）。
诊断：
- 若 `df3_clean_speech_preserved` 报 `/tmp/voice48k.wav` 不存在 → 先用 `say + ffmpeg` 生成（见 Step 1）。
- 若 `df3_clean_speech_preserved` gain<0.5 且输入是真实语音 → 真异常，检查 Df3Backend 实现或模型版本。
  （若误用 `synth_speech` 得 gain≈0.005，是代理失真非缺陷——改用真实 wav。）
- 若 `DfTract::default` panic 未 catch → 确认 `AssertUnwindSafe(DfTract::default)` 包裹正确。
- 若 ndarray 类型不匹配 → 确认 `ndarray_015` 与 libDF 同版本（`grep -A1 'name = "ndarray"' Cargo.lock` 应只有 0.15.x 与 0.17.x 各一）。

- [x] **Step 3: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "test(asr): Df3Backend 行为测试（长度守恒 / 不压语音 / 噪声抑制）"
```

---

## Task 5: audio.rs 接入 denoise_mode

**Files:**
- Modify: `crates/desktop/src/audio.rs:96-98`（读 mode）、`audio.rs:208-220`（构造）、注释 :88

**背景**：audio.rs 经 `octopus_asr::config::load_app_config_cached()`（返回 `&AppConfig`）读 `effective_denoise_mode()`。mode=0 直通（不走 down_sampler/denoise），mode=1/2 走 denoise 路径（DenoiseProcessor 内部按 mode 选后端）。

- [x] **Step 1: 改 process_pipeline 读 mode（audio.rs:96-98）**

把：
```rust
        let cfg = octopus_asr::config::load_app_config_cached();
        let denoise_on = cfg.denoise_enabled;
```
改为：
```rust
        let cfg = octopus_asr::config::load_app_config_cached();
        let denoise_on = cfg.effective_denoise_mode() != 0;
```

- [x] **Step 2: 改 start() 构造传 mode（audio.rs:208-220）**

把：
```rust
        let cfg = octopus_asr::config::load_app_config_cached();
        {
            let mut g = self.denoise.lock().unwrap();
            if cfg.denoise_enabled {
                match octopus_asr::denoise::DenoiseProcessor::new() {
```
改为：
```rust
        let cfg = octopus_asr::config::load_app_config_cached();
        let mode = octopus_asr::denoise::DenoiseMode::from_u8(cfg.effective_denoise_mode());
        {
            let mut g = self.denoise.lock().unwrap();
            if mode != octopus_asr::denoise::DenoiseMode::Off {
                match octopus_asr::denoise::DenoiseProcessor::new(mode) {
```

- [x] **Step 3: 改日志文案（区分 mode）**

把该 match 块内的：
```rust
                        info!("RNNoise 环境降噪已启用（nnnoiseless，48k）");
```
改为：
```rust
                        info!("环境降噪已启用（mode={:?}，48k）", mode);
```
以及降级 warn 文案 `RNNoise 降噪初始化失败` 改为 `环境降噪初始化失败`。

- [x] **Step 4: 更新注释（audio.rs:88）**

把：
```rust
    /// 降级（spec §9）：denoise_enabled=false / 模型缺失 / 实例未就绪 → 走直通（原生→16k），
```
改为：
```rust
    /// 降级（spec §9）：denoise_mode=0 / 模型缺失 / 实例未就绪 → 走直通（原生→16k），
```

- [x] **Step 5: 编译验证**

Run:
```bash
cargo check --workspace --all-targets 2>&1 | tail -10
```
Expected: `Finished` 无错误。

- [x] **Step 6: 提交**

```bash
git add crates/desktop/src/audio.rs
git commit -m "feat(desktop): audio 接入 denoise_mode（effective_denoise_mode 分发）"
```

---

## Task 6: 文档同步

**Files:**
- Modify: `docs/configuration.md`、`docs/architecture.md`

**背景**：CLAUDE.md 强制——需求/接口变更同步文档。

- [x] **Step 1: docs/configuration.md 加 denoise_mode 说明**

grep 定位：
```bash
grep -n "denoise" docs/configuration.md
```
把对应字段说明改为（若无则新增）：
```markdown
- `denoise_mode`（可选，默认看 `denoise_enabled`）：环境降噪模式
  - `0`：关闭（直通）
  - `1`：RNNoise（nnnoiseless，默认）
  - `2`：DeepFilterNet3（libDF v0.5.6，48kHz 全频带，质量最佳，~7.9MB 模型）
  - 未配置时回退旧 `denoise_enabled`（true→1，false→0）以向后兼容。
```

- [x] **Step 2: docs/architecture.md 更新降噪段**

grep 定位：
```bash
grep -n "降噪\|denoise\|RNNoise" docs/architecture.md
```
更新为说明：降噪为可插拔后端（`FrameDenoise` trait），`denoise_mode` 0/1/2 选择；DF3 用 libDF v0.5.6 + tract 0.19（git 依赖），ndarray 0.15 与 asr 0.17 靠 slice 边界隔离；Df3Backend 经 unsafe impl Send（单线程串行访问）。

- [x] **Step 3: 提交**

```bash
git add docs/configuration.md docs/architecture.md
git commit -m "docs: 同步 denoise_mode 0/1/2 与 DF3 整合说明"
```

---

## 验收（手动 e2e）

备份 `~/.octopus/` 后：

```bash
# mode=2 加载 DF3、不压语音
# 编辑 ~/.octopus/config.yaml 设 denoise_mode: 2
cargo run -p octopus-desktop  # 录音 → ASR 结果不应退化（DF3 gain≈0.96）
# mode=1 RNNoise（现状）
# 设 denoise_mode: 1（或删 denoise_mode，留 denoise_enabled: true）
# mode=0 直通
# 设 denoise_mode: 0
```

```bash
cargo test -p octopus-asr --lib denoise -- --ignored  # DF3 单元测试（手动，慢）
cargo test -p octopus-asr --lib denoise               # RNNoise 回归
cargo test -p octopus-infra denoise                   # config 测试
```
