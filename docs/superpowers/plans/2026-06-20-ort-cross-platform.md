# ort 跨平台 EP feature 条件化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 ort 的硬件加速 EP feature 按平台条件化（mac=coreml、linux=cuda、win=directml），与代码层 `#[cfg]` 1:1 对齐，为 macOS 上曾发生的 cuda/directml EP segfault 提供feature-level 二道防线（首道是 config.rs 的 cfg gate）。

**Architecture:** Cargo target-specific dependency——base `[dependencies]` 放 `download-binaries`，三个 `[target.'cfg(target_os=...)']` 各放对应 EP feature；代码层 `config.rs` win 块删 CUDA 注册。

**Tech Stack:** Rust + Cargo target-specific dependencies + ort 2.0.0-rc.12 + `#[cfg(target_os)]` 条件编译

**设计 spec:** `docs/superpowers/specs/2026-06-20-ort-cross-platform-feature-design.md`

**Worktree:** `feature/ort-cross-platform`（`.claude/worktrees/ort-cross-platform`）。EnterWorktree 工具切入失败，session CWD 已手动落在 worktree 内；git 操作直接在本目录跑。

---

## 执行结果（2026-06-20，已落地）

commits：`66a8a73`（Task 1 Cargo.toml）、`21fb2fb`（Task 2 config.rs）。

**三项实测结论（部分修正了 spec/plan 的初始前提，详见 spec §7）：**

1. **mac 体积收益 = 0**：删 cuda/directml feature 后 mac 二进制仍 54M（56,778,304 字节），零下降。这两个 feature 在 mac 本就是 size no-op（Rust 引用早被 `#[cfg]` 排除；GPU 预编译库无 mac 版从未下载）。本改动价值不在体积。
2. **「feature↔#[cfg] 不一致则编译失败」不成立**：ort EP 类型无条件编译，仅 `register()` 内 FFI 块按 feature gate（feature off 时返回 `MissingFeature`、不碰 FFI）。故编译器**不**抓不一致。但 feature 关闭仍提供**真·defense-in-depth**——cuda/directml feature off 时，即便有人退化 config.rs 的 cfg gate，`register()` 也不走 FFI dlopen-libcuda（segfault 路径），从而不崩。
3. **`default-features=false` 实测去掉**（保留默认集开启）：关掉会缺 `tls-native`，`download-binaries` 编译即报缺 TLS feature；而默认集本不含 cuda/directml/coreml，保留不影响目标。

**验证状态：** mac 单测 45 passed/0 failed、release build 通过、**GUI e2e 已通过（2026-06-20，CoreML 加速正常、无 segfault 回归）**；linux/win 交叉 check 受阻于目标工具链（openssl-sys / esaxx-rs），非 ort，留用户在对应平台本地 `cargo check` 兜底。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/asr/Cargo.toml` | ort 依赖声明 | 改 target-specific（base + 3 平台块） |
| `crates/asr/src/config.rs` | `apply_session_acceleration` 的 EP 注册 | win 块删 CUDA + 更新注释 |

无新建文件。

## 测试策略

配置 + 条件编译改动，无传统单测新增。验证三层：

1. **现有测试不回归**：`cargo test -p octopus-asr` 全过。
2. **mac 编译+release**：`cargo check/build --release` 通过。
3. **mac e2e**：desktop 录音 + CoreML 加速正常、无 segfault 回归（用户本地）。

> feature 矩阵的 linux/win 正确性：交叉 `cargo check` 受阻于目标工具链（非 ort），改由源码 gate 结构核验（spec §7.3）+ 用户在对应平台本地自测。

---

## Task 1: Cargo.toml 改 target-specific ort ✅

**Files:**
- Modify: `crates/asr/Cargo.toml`（ort 声明 + 文件末尾追加 3 个 target 块）

- [x] **Step 1: 替换 ort 声明为 target-specific 形态**

base 行留在 `[dependencies]` 顶部，三个 `[target.'cfg']` 块追加到 `[dependencies]` 表**末尾**（flate2 之后）——TOML 表头切换活跃表，放 ort 行正下方会让 ndarray/anyhow 等后续依赖泄漏进 windows target。最终形态（实测去掉 `default-features=false`，见 Step 3）：

```toml
# ort EP feature 按平台条件化（spec 2026-06-20）：与 config.rs apply_session_acceleration
# 的 #[cfg(target_os)] 1:1 对齐（设计意图，非编译器硬约束——ort EP 类型无条件编译、仅 register()
# FFI 块按 feature gate，故不一致不会编译失败；真正的 feature-level 保护见 spec §7.3）。
# base 放跨平台必需的 download-binaries，各平台只启用对应 EP。
# 不用 default-features=false：ort 默认集 = std/ndarray/tracing/download-binaries/tls-native/
# copy-dylibs/api-24，关掉会缺 TLS（download-binaries 编译即报缺 TLS feature）。关键在于
# 默认集本就不含 cuda/directml/coreml——保留默认集不影响「各平台只编入本平台 EP」的目标。
ort = { version = "2.0.0-rc.12", features = ["download-binaries"] }
```

文件末尾（flate2 之后）追加：

```toml
[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["coreml"] }

[target.'cfg(target_os = "linux")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["cuda"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["directml"] }
```

- [x] **Step 2: mac 本地验证编译**

Run: `cargo check -p octopus-asr`
结果：初版带 `default-features=false` 时**报错**——`download-binaries` 要求 TLS feature（关掉默认集丢了 `tls-native`）。去掉 false 后通过（16.3s）。

- [x] **Step 3: 实测 `default-features = false` 结论**

**结论：去掉 `default-features = false`，保留默认集开启。** ort 默认集 = `["std","ndarray","tracing","download-binaries","tls-native","copy-dylibs","api-24"]`，关掉缺 `tls-native` 致 download-binaries 编译失败。关键：默认集**本不含** cuda/directml/coreml，故保留不影响「各平台只编入本平台 EP」目标。

- [x] **Step 4: commit**（`66a8a73`）

```
build(asr): ort EP feature 按平台条件化（mac=coreml/linux=cuda/win=directml）
```
（含 default-features 实测结论 + target 块放末尾的 TOML 正确性说明；Cargo.lock 顺带 webpki-root-certs 1.0.7→1.0.8 patch bump）

---

## Task 2: config.rs win 块删 CUDA 注册 ✅

**Files:**
- Modify: `crates/asr/src/config.rs`（`apply_session_acceleration` 的 win cfg 块 + 上方注释）

- [x] **Step 1: 改 win cfg 块（删 CUDA）+ 更新注释**

```rust
    // macOS=CoreML、Linux=CUDA、Windows=DirectML（Cargo feature 同步按平台条件化，见 Cargo.toml）。
    let mut providers = Vec::new();
    #[cfg(target_os = "macos")]
    providers.push(ort::ep::CoreMLExecutionProvider::default().build());
    #[cfg(target_os = "linux")]
    providers.push(ort::ep::CUDAExecutionProvider::default().build());
    #[cfg(target_os = "windows")]
    providers.push(ort::ep::DirectMLExecutionProvider::default().build());
```

> **实测更正原前提**：原 plan/spec 称「win 无 cuda feature → `CUDAExecutionProvider` 类型不存在会编不过」。实测 ort EP 类型**无条件编译**（仅 `register()` 内 FFI 块按 feature gate），故留着也能编译（运行时 register 返回 `MissingFeature`→CPU fallback）。删除**非编译必需**，但仍是正确改动——避免注册一个注定失败的死 EP。详见 spec §5.3/§7.5。

- [x] **Step 2: mac 本地验证（win 块不参与 mac 编译，应仍过）**

Run: `cargo check -p octopus-asr`
结果：通过（1.37s）。

- [x] **Step 3: commit**（`21fb2fb`）

```
refactor(asr): win EP 收敛为仅 DirectML（删 CUDA 注册）
```

---

## Task 3: mac 本地完整验证 ✅

**Files:** 无改动，仅验证。

- [x] **Step 1: asr 单测不回归**

Run: `cargo test -p octopus-asr`
结果：**45 passed / 0 failed / 6 ignored**（无回归）。

- [x] **Step 2: desktop release 编译**

Run: `cargo build --release -p octopus-desktop`
结果：通过（5m07s，strip+lto+codegen-units 预期慢）。

- [x] **Step 3: desktop e2e（CoreML 加速正常、无 segfault 回归）—— 已通过（2026-06-20 用户本地）**

启动 desktop app → 触发录音快捷键 → 确认：转写出文字、日志含 `Successfully registered EPs!`、无 SIGSEGV。**结果：通过**——CoreML 加速正常工作、无 segfault 回归（本改动根治的点）。

- [x] **Step 4: 无代码改动不 commit；Step 1-3 全过**

---

## Task 4: linux/windows 交叉编译验证 ⚠️（受目标工具链阻塞，非 ort）

**Files:** 无改动，仅验证。

- [x] **Step 1: 安装交叉编译 target**

Run: `rustup target add x86_64-unknown-linux-gnu x86_64-pc-windows-msvc`
结果：两个 target 安装成功。

- [x] **Step 2: linux 交叉 check —— 受阻**

Run: `cargo check -p octopus-asr --target x86_64-unknown-linux-gnu`
结果：**卡在 `openssl-sys` build script**——mac→linux 缺 openssl dev sysroot / pkg-config 交叉配置。非 ort、非本改动（openssl-sys 来自 ort 默认集 `tls-native`）。

- [x] **Step 3: windows 交叉 check —— 受阻**

Run: `cargo check -p octopus-asr --target x86_64-pc-windows-msvc`
结果：**卡在 `esaxx-rs` C++ build script**（jieba-rs 的 C++ 依赖）——mac 无 MSVC C++ 工具链，`clang++` 找不到 `<cstdint>`。非 ort、非本改动。

- [x] **Step 4: 降级处理（按 plan 原预案）**

两端均为目标平台 C/C++ 工具链缺失（非 ort-sys 下载问题）。feature 矩阵正确性改由：
- **源码 gate 结构核验**（spec §7.3）：ort EP 类型无条件编译、仅 `register()` FFI 块按 feature gate；
- **mac coreml 实证代理**：mac（有 coreml feature）编译通过，证明「feature present + 类型引用 → 编通过」模式成立，directml/cuda 同构。
- linux/win 运行正确性留用户在对应平台本地 `cargo check` + 自测（spec §7.2 已声明环境限制）。

**未**为绕过而改回全开 feature——那会破坏本改动目的。

- [x] **Step 5: 验证 task，无 commit**

---

## Task 5: mac 体积对比 ✅（结果：0 下降）

**Files:** 无改动，仅度量。

- [x] **Step 1: rebuild release 并对比**

Run: `cargo build --release -p octopus-desktop`（已在 Task 3 Step 2 产出）→ `stat` 二进制。
结果：**54M（56,778,304 字节），与基线（profile 优化后的 54M）相同，零下降**。

原因（spec §7.1）：cuda/directml feature 在 mac 是 size no-op——① 其 Rust 类型早被 config.rs `#[cfg]` 排除、从未编进 mac 二进制；② 其 GPU 预编译库（libcuda/cudnn/DirectML.dll）无 mac 版、从未下载/链接。本改动价值在 segfault defense-in-depth（§7.3），非体积。

- [x] **Step 2: 度量 task，无 commit；数据已记录**

---

## 已知限制（来自 spec §7）

- linux/win **运行未实测** + 交叉 check 受目标工具链阻塞（openssl-sys / esaxx-rs），运行正确性靠用户在对应平台自测。
- mac **体积收益 = 0**（cuda/directml 在 mac 是 size no-op）。
- feature↔#[cfg] **非编译器硬约束**；真正的保护是 `register()` 的 feature gate（defense-in-depth）。
- `default-features` 保留开启（实测后定）。
- win CUDA 删除**非编译必需**（类型存在），但运行时清洁，仍正确。
- win NVIDIA 用户从 CUDA 回退到 DirectML，极端长音频批量转写可能略慢；实时录音转写影响可忽略。
