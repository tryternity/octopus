# ort 跨平台 EP feature 条件化设计

> Date: 2026-06-20
> 状态：已实现（2026-06-20，commits 66a8a73 + 21fb2fb）。mac 单测+release 通过；GUI e2e 已通过；linux/win 交叉 check 受阻于目标工具链（见 §7.2）
> Worktree：`feature/ort-cross-platform`
> 关联：体积裁剪报告（2026-06-20，release profile 已落地 ac576de）、[[asr 硬件加速 segfault 修复]]

## 1. 背景

octopus 的 ort（ONNX Runtime）依赖在 `crates/asr/Cargo.toml` 无差别全开四个 feature：

```toml
ort = { version = "2.0.0-rc.12", features = ["download-binaries", "cuda", "coreml", "directml"] }
```

带来两个问题：

1. **体积冗余（§7.1 实测推翻此假设）**：原以为 mac 二进制会编入 cuda/directml 的 Rust EP 代码 + 触发多余预编译下载。**实测不成立**——config.rs 的 `#[cfg]` 早把 cuda/directml 的 Rust 引用排除（从未编进 mac 二进制），GPU 预编译库无 mac 版（从未下载）。mac 二进制维持 54M。本改动真正价值是 ② segfault defense-in-depth，非体积。
2. **segfault 根源未除**：全开 feature 曾导致 macOS 上跨平台误注册 CUDA/DirectML EP，其 init 失败路径（dlopen libcuda 等）直接 SIGSEGV 绕过 Rust 错误处理。**代码层已修复**（`config.rs:424-432` 按 `#[cfg(target_os)]` 只注册本平台 EP），但 **feature 仍全开**——根源未除，未来代码回归时仍可能复发。

目标 app 需同时支持 mac/win/linux 三平台打包。

## 2. 目标

- 三平台各自**只启用对应硬件加速 EP**：mac→coreml、linux→cuda、win→directml。
- ort feature 与代码层 `#[cfg]` **1:1 对齐**（设计意图；**非编译器硬约束**——ort 的 EP 类型无条件编译、只有 `register()` 内 FFI 块按 feature gate，故不一致不会编译失败，见 §5.4/§7.3）。
- 处理 Cargo feature unification 坑（确认在此结构下不构成问题）。
- 不引入构建脚本/CI 按平台传参的脆弱依赖。

## 3. 探索结论（关键）

1. **代码层（`config.rs:401-449` `apply_session_acceleration`）已按 `#[cfg(target_os)]` 分平台注册 EP**（设计时现状；本 spec Task 2 后 win 收敛为仅 DirectML）：mac=CoreML、linux=CUDA、win=DirectML+CUDA（改动前）。含 `asr_hardware_accelerated` 开关、qwen3-asr 跳 CoreML（动态算子不兼容）、EP 注册失败 fallback CPU。代码层早就是对的。
2. **ort 全 workspace 仅 `asr/Cargo.toml` 一处声明**（`dlp/main.rs:38` 的 `#[cfg]` 非 ort EP）→ **不存在 workspace 级 feature unification 问题**。
3. **target-specific dependency 的 feature 不跨 target 合并**：mac 编译时 linux 块的 `cuda` 不激活。所谓"并集"坑只在"同一 target 内多处声明同一包"时发生，octopus 仅 base 一处 + per-target 一处，合并结果正是期望（`download-binaries` + 该平台 EP）。
4. 结论：方案比预想简单安全——标准 target-specific dependency 即可，无需 build.rs 或自定义 feature 开关。

## 4. 方案选择

| 方案 | 形态 | 评价 |
|---|---|---|
| **A. target-specific dependency（采用）** | base `[dependencies]` 放 `download-binaries`，三个 `[target.'cfg']` 各放 EP feature | Cargo 惯例、最简洁、与代码 `#[cfg]` 1:1、零构建脚本依赖 |
| B. 自定义 `[features]` + `--features` 按平台传 | 定义 cuda/directml feature，构建按 target 传参 | 需 CI/脚本传参，易忘易错，不如 A 自动 |
| C. 拆 per-platform asr 子 crate | asr-mac / asr-linux / asr-win | 过度工程，YAGNI，否决 |

**采用 A。**

## 5. 设计

### 5.1 Feature 矩阵

| 平台 | base feature | EP feature | 代码层注册的 EP |
|---|---|---|---|
| macOS | download-binaries | coreml | CoreML |
| Linux | download-binaries | cuda | CUDA |
| Windows | download-binaries | directml | DirectML（**删 CUDA**） |

CPU EP 是 ort 内置，无需 feature，所有平台自动可用（EP 注册失败时 fallback CPU）。

### 5.2 `crates/asr/Cargo.toml` 形态

```toml
[dependencies]
ort = { version = "2.0.0-rc.12", features = ["download-binaries"] }

[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["coreml"] }

[target.'cfg(target_os = "linux")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["cuda"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["directml"] }
```

> **`default-features` 实测结论（2026-06-20）：去掉 false、保留默认集开启。** 关掉会缺 `tls-native`，`download-binaries` 编译即报缺 TLS feature。ort 默认集 = std/ndarray/tracing/download-binaries/tls-native/copy-dylibs/api-24，**本不含 cuda/directml/coreml**，故保留不影响目标。另：三个 `[target.'cfg']` 块须放 `[dependencies]` 表**末尾**（非 ort 行正下方）——TOML 表头切换活跃表，放中间会让后续依赖泄漏进 windows target。

### 5.3 代码层改动（`crates/asr/src/config.rs:428-432`）

win 块删 CUDA 注册。**实测更正**：`CUDAExecutionProvider` 类型**无条件存在**（并非「feature 没开就类型不存在」）——ort EP 类型始终编译，仅 `register()` 内 FFI 块按 feature gate（cuda off 时返回 `MissingFeature`、不碰 FFI）。故删除**非编译必需**，理由实为「避免注册注定失败的死 EP」：

```rust
// 改前
#[cfg(target_os = "windows")]
{
    providers.push(ort::ep::DirectMLExecutionProvider::default().build());
    providers.push(ort::ep::CUDAExecutionProvider::default().build());
}

// 改后
#[cfg(target_os = "windows")]
providers.push(ort::ep::DirectMLExecutionProvider::default().build());
```

同步更新上方注释（`Windows=DirectML+CUDA` → `Windows=DirectML`）。

### 5.4 一致性

- ort 仅 asr 一处声明，三平台各自 base+ep 合并，**无 workspace unification 风险**。
- 代码 `#[cfg]` 与 feature 矩阵 **1:1 对齐**（设计意图，**非编译器硬约束**）。ort EP 类型无条件编译、仅 `register()` FFI 块按 feature gate——故「win 有 directml 却引用 `CUDAExecutionProvider`」之类不一致**不会编译失败**，只在运行时 register 返回 `MissingFeature`→CPU fallback。真正的 feature-level 保护见 §7.3。

### 5.5 验证策略

- **mac 本地（已验证 2026-06-20）**：`cargo test -p octopus-asr` 45 passed/0 failed；`cargo build --release -p octopus-desktop` 通过；desktop e2e（CoreML 录音）**已通过**（CoreML 加速正常、无 segfault 回归）。
- **linux/win 交叉（受阻）**：`cargo check --target x86_64-unknown-linux-gnu` 卡在 `openssl-sys`（mac→linux 缺 openssl dev sysroot）、`--target x86_64-pc-windows-msvc` 卡在 `esaxx-rs` C++（缺 MSVC 工具链）——均为目标平台 C/C++ 工具链缺失，**非 ort、非本改动**。feature 矩阵正确性改由源码 gate 结构（§7.3）+ mac coreml 实证代理核验；运行正确性留用户在对应平台本地自测。
- CI 是否三平台 check：当前未设。

## 6. 关键决策

1. **win 仅 DirectML（删 CUDA）**：DirectML 覆盖所有 DX12 GPU（NVIDIA/AMD/Intel 核显通吃），实时语音转写够用；省 cuda 预编译库体积；三平台单 EP 矩阵一致。代价：NVIDIA 卡无法走 CUDA（推理更快），但 YAGNI，DirectML 对实时转录足够。需同步删代码层 win 的 CUDA 注册。
2. **`default-features`（实测后：不开 false，保留默认集）**：原想避免 ort 默认拉多余 feature。实测关掉会缺 `tls-native` 致 download-binaries 编译失败；而默认集本不含 cuda/directml/coreml，保留不影响目标。详见 §5.2。

## 7. 已知限制 / 风险（含 2026-06-20 实测结论）

- **7.1 mac 体积收益 = 0（实测推翻 §1.① 假设）**：删 cuda/directml feature 后 mac 二进制仍 54M（56,778,304 字节），零下降。原因：① config.rs 的 `#[cfg]` 早把 cuda/directml 的 Rust 引用排除，从未编进 mac 二进制；② cuda/directml 的 GPU 预编译库（libcuda/cudnn/DirectML.dll）无 mac 版，从未下载/链接。这两个 feature 在 mac 本就是 size no-op。**本改动价值不在 mac 体积，而在 §7.3 的 segfault defense-in-depth。**
- **7.2 linux/win 运行未实测 + 交叉 check 受阻**：环境仅 mac。交叉 `cargo check` 在 `openssl-sys`（linux）/`esaxx-rs`（win C++）受阻于目标工具链缺失，非 ort。运行正确性靠用户在对应平台本地 `cargo check` + 自测。
- **7.3 feature↔#[cfg] 非编译器硬约束（实测更正 §2/§5.4）**：ort EP 类型无条件编译，仅 `register()` 内 FFI 块按 feature gate。故「不一致」不会编译失败。但 feature 关闭仍提供**真·defense-in-depth**：cuda/directml feature off 时，即便有人退化 config.rs 的 cfg gate、在 mac 上注册 CUDA EP，`register()` 会直接返回 `MissingFeature`、**不走** FFI dlopen-libcuda（即 segfault 那条路径），从而不崩。这是本改动对 segfault 根源的二道防线（首道是 config.rs 的 cfg gate，已在 [[asr-hw-accel-release-segfault]] 修复）。
- **7.4 `default-features` 已定（实测后保留开启，非 false）**：见 §5.2/§6.2。
- **7.5 win CUDA 删除非编译必需（实测更正 §5.3）**：类型存在，留着也能编译。删除是运行时清洁（避免注册注定 MissingFeature 的死 EP），仍是正确改动。
- **7.6 CUDA 用户感知**：win NVIDIA 用户从 CUDA 回退到 DirectML，极端长音频批量转写可能略慢；实时录音转写影响可忽略。

## 8. 不做（YAGNI 边界）

- 不动 `asr_hardware_accelerated` 开关逻辑（现状保留）。
- 不动 qwen3-asr 跳 CoreML 逻辑（现状保留）。
- 不拆 per-platform crate（方案 C）。
- 不升级 ort 版本（仍 2.0.0-rc.12）。
- 不为 linux 额外支持 coreml、不为 mac 支持 cuda（跨平台无意义）。
