# ort 跨平台 EP feature 条件化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 ort 的硬件加速 EP feature 按平台条件化（mac=coreml、linux=cuda、win=directml），与代码层 `#[cfg]` 1:1 对齐，消除 mac 二进制里的 cuda/directml 体积冗余与 segfault 根源。

**Architecture:** Cargo target-specific dependency——base `[dependencies]` 放 `download-binaries`，三个 `[target.'cfg(target_os=...)']` 各放对应 EP feature；代码层 `config.rs` win 块删 CUDA 注册。feature 矩阵与 `#[cfg]` 互为硬验证（不一致则该平台编译失败，编译器即测试）。

**Tech Stack:** Rust + Cargo target-specific dependencies + ort 2.0.0-rc.12 + `#[cfg(target_os)]` 条件编译

**设计 spec:** `docs/superpowers/specs/2026-06-20-ort-cross-platform-feature-design.md`

**Worktree:** `feature/ort-cross-platform`（`.claude/worktrees/ort-cross-platform`）。EnterWorktree 工具切入失败，session CWD 已手动落在 worktree 内；git 操作直接在本目录跑。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/asr/Cargo.toml` | ort 依赖声明 | 改 target-specific（base + 3 平台块） |
| `crates/asr/src/config.rs` | `apply_session_acceleration` 的 EP 注册 | win 块删 CUDA + 更新注释 |

无新建文件。

## 测试策略

配置 + 条件编译改动，无传统单测新增。验证三层：

1. **现有测试不回归**：`cargo test -p octopus-asr`（含 `config.rs` 的 `tests` mod）全过，确保 `apply_session_acceleration` 逻辑未坏。
2. **编译即验证**：三平台 `cargo check`（mac 本地 + linux/win 交叉）验证 feature 矩阵正确——feature 与 `#[cfg]` 不一致时编译器直接报错，无需手写断言。
3. **mac e2e**：desktop 录音 + CoreML 加速正常、无 segfault 回归。

---

## Task 1: Cargo.toml 改 target-specific ort

**Files:**
- Modify: `crates/asr/Cargo.toml:7-12`

- [ ] **Step 1: 替换 ort 声明为 target-specific 形态**

把现有的单块全开声明：

```toml
ort = { version = "2.0.0-rc.12", features = [
    "download-binaries",
    "cuda",
    "coreml",
    "directml",
] }
```

替换为（base 跨平台 + 三平台各一 EP）：

```toml
# ort EP feature 按平台条件化（spec 2026-06-20）：与 config.rs apply_session_acceleration
# 的 #[cfg(target_os)] 1:1 对齐。base 放跨平台必需的 download-binaries，各平台只启用对应 EP。
# 硬约束：feature 矩阵必须与 #[cfg] 一致，否则对应平台编译失败（编译器即验证）。
ort = { version = "2.0.0-rc.12", default-features = false, features = ["download-binaries"] }

[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.12", default-features = false, features = ["coreml"] }

[target.'cfg(target_os = "linux")'.dependencies]
ort = { version = "2.0.0-rc.12", default-features = false, features = ["cuda"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "2.0.0-rc.12", default-features = false, features = ["directml"] }
```

- [ ] **Step 2: mac 本地验证编译**

Run: `cargo check -p octopus-asr`
Expected: 编译通过（mac target 激活 base `download-binaries` + `coreml`）。

- [ ] **Step 3: 实测 `default-features = false` 是否误关必需默认项**

若 Step 2 报错提示缺默认 feature（典型如 ort 的 `ndarray` 集成或 `std` 相关类型找不到），把四处声明里的 `default-features = false, ` 全部去掉，改为只追加 feature，例如：

```toml
ort = { version = "2.0.0-rc.12", features = ["download-binaries"] }

[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["coreml"] }

[target.'cfg(target_os = "linux")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["cuda"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "2.0.0-rc.12", features = ["directml"] }
```

重跑 `cargo check -p octopus-asr` 直到通过。通过后记录最终是否保留 `default-features = false`（写进 commit message）。

- [ ] **Step 4: commit**

```bash
git add crates/asr/Cargo.toml
git commit -m "build(asr): ort EP feature 按平台条件化（mac=coreml/linux=cuda/win=directml）

Cargo target-specific dependency，与 config.rs #[cfg] 1:1 对齐。
消除 mac 二进制 cuda/directml 体积冗余 + segfault 根源。
default-features=<记录 Step 3 实测结果>。"
```

---

## Task 2: config.rs win 块删 CUDA 注册

**Files:**
- Modify: `crates/asr/src/config.rs:419-432`

- [ ] **Step 1: 改 win cfg 块（删 CUDA）+ 更新注释**

把现有：

```rust
    // 按平台注册对应 EP——原实现跨平台全注册（CUDA/DirectML/CoreML 一起），在 macOS 上
    // 会去 init Linux/Windows 专用的 CUDA/DirectML EP，其失败路径（dlopen libcuda 等）
    // 可能直接 segfault（SIGSEGV 绕过 Rust 错误处理，下面 match 抓不到）。
    // macOS=CoreML、Linux=CUDA、Windows=DirectML+CUDA。
    let mut providers = Vec::new();
    #[cfg(target_os = "macos")]
    providers.push(ort::ep::CoreMLExecutionProvider::default().build());
    #[cfg(target_os = "linux")]
    providers.push(ort::ep::CUDAExecutionProvider::default().build());
    #[cfg(target_os = "windows")]
    {
        providers.push(ort::ep::DirectMLExecutionProvider::default().build());
        providers.push(ort::ep::CUDAExecutionProvider::default().build());
    }
```

改为（win 块收敛为单行 DirectML + 注释末行同步）：

```rust
    // 按平台注册对应 EP——原实现跨平台全注册（CUDA/DirectML/CoreML 一起），在 macOS 上
    // 会去 init Linux/Windows 专用的 CUDA/DirectML EP，其失败路径（dlopen libcuda 等）
    // 可能直接 segfault（SIGSEGV 绕过 Rust 错误处理，下面 match 抓不到）。
    // macOS=CoreML、Linux=CUDA、Windows=DirectML（Cargo feature 同步按平台条件化，见 Cargo.toml）。
    let mut providers = Vec::new();
    #[cfg(target_os = "macos")]
    providers.push(ort::ep::CoreMLExecutionProvider::default().build());
    #[cfg(target_os = "linux")]
    providers.push(ort::ep::CUDAExecutionProvider::default().build());
    #[cfg(target_os = "windows")]
    providers.push(ort::ep::DirectMLExecutionProvider::default().build());
```

> 为什么删 CUDA：win 的 ort feature 现在只开 `directml`（Task 1），`CUDAExecutionProvider` 类型在 win 编译时不存在，留着会编不过。DirectML 覆盖所有 DX12 GPU，实时语音转写够用（详见 spec §6.1）。

- [ ] **Step 2: mac 本地验证（win 块不参与 mac 编译，应仍过）**

Run: `cargo check -p octopus-asr`
Expected: 编译通过。

- [ ] **Step 3: commit**

```bash
git add crates/asr/src/config.rs
git commit -m "refactor(asr): win EP 收敛为仅 DirectML（删 CUDA 注册）

与 Cargo.toml win=directml feature 对齐（feature↔#[cfg] 一致性硬约束）。
DirectML 通吃 DX12 GPU，实时转写够用；NVIDIA 卡不再走 CUDA（YAGNI）。"
```

---

## Task 3: mac 本地完整验证

**Files:** 无改动，仅验证。

- [ ] **Step 1: asr 单测不回归**

Run: `cargo test -p octopus-asr`
Expected: 全过（基线 42 passed, 6 ignored——以当前实际为准），`apply_session_acceleration` 相关逻辑无回归。

- [ ] **Step 2: desktop release 编译**

Run: `cargo build --release -p octopus-desktop`
Expected: 通过（含 2026-06-20 落地的 strip+lto profile，首编慢属正常）。

- [ ] **Step 3: desktop e2e（CoreML 加速正常、无 segfault 回归）**

启动 desktop app → 触发录音快捷键 → 确认：
- 转写正常出文字
- 日志含 `Successfully registered EPs!`（CoreML 注册成功）
- **无 SIGSEGV / 崩溃**（这是本改动要根治的回归点）

> 若 `asr_hardware_accelerated` 关闭则不会注册 EP——确认 config.yaml 该项开启再测。

- [ ] **Step 4:（无代码改动，不 commit；若 Step 1-3 全过，本 task 视为完成）**

---

## Task 4: linux/windows 交叉编译验证（feature 矩阵正确性）

**Files:** 无改动，仅验证。

- [ ] **Step 1: 安装交叉编译 target**

Run: `rustup target add x86_64-unknown-linux-gnu x86_64-pc-windows-msvc`
Expected: 两个 target 安装成功（已装则跳过）。

- [ ] **Step 2: linux 交叉 check**

Run: `cargo check -p octopus-asr --target x86_64-unknown-linux-gnu`
Expected: 编译通过——验证 linux 激活 `cuda` feature + 代码 linux `#[cfg]` 块（`CUDAExecutionProvider`）类型存在、一致。

- [ ] **Step 3: windows 交叉 check**

Run: `cargo check -p octopus-asr --target x86_64-pc-windows-msvc`
Expected: 编译通过——验证 win 激活 `directml` feature + 代码 win `#[cfg]` 块（仅 `DirectMLExecutionProvider`，已无 CUDA）类型存在、一致。

- [ ] **Step 4: 若 ort-sys 交叉下载预编译库受阻的降级处理**

`cargo check` 会触发 ort-sys 的 build script 下载对应 target 预编译库。若网络/平台原因下载失败导致 check 报错（错误信息含 `ort-sys` / download），则：
- 记录跳过原因（写进本 task 完成报告）
- 该平台的 feature↔#[cfg] 一致性改由**用户在对应平台本地 `cargo check`** 兜底验证（环境限制，spec §7 已声明）
- **不要**为绕过下载而改回全开 feature——那会破坏本改动的目的

- [ ] **Step 5:（验证 task，无 commit）**

---

## Task 5（可选）: mac 体积对比

**Files:** 无改动，仅度量。

- [ ] **Step 1: rebuild release 并对比**

Run: `cargo build --release -p octopus-desktop && ls -lh target/release/octopus-desktop`
Expected: 记录体积。基线（cuda/directml 全开 + profile 优化）= 54M；本改动后 mac 不再编入 cuda/directml 的 Rust EP 代码，预期下降（具体幅度以实测为准，记录进完成报告）。

- [ ] **Step 2:（度量 task，无 commit；仅记录数据）**

---

## 已知限制（来自 spec §7）

- linux/win **运行未实测**：交叉 `cargo check` 只验证编译通过，EP 实际推理正确性靠用户在对应平台自测。
- `default-features` 实测结论由 Task 1 Step 3 确定。
- win NVIDIA 用户从 CUDA 回退到 DirectML，极端长音频批量转写可能略慢；实时录音转写影响可忽略。
