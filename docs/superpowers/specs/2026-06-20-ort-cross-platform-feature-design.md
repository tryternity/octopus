# ort 跨平台 EP feature 条件化设计

> Date: 2026-06-20
> 状态：设计中（待实现）
> Worktree：`feature/ort-cross-platform`
> 关联：体积裁剪报告（2026-06-20，release profile 已落地 ac576de）、[[asr 硬件加速 segfault 修复]]

## 1. 背景

octopus 的 ort（ONNX Runtime）依赖在 `crates/asr/Cargo.toml` 无差别全开四个 feature：

```toml
ort = { version = "2.0.0-rc.12", features = ["download-binaries", "cuda", "coreml", "directml"] }
```

带来两个问题：

1. **体积冗余**：mac 二进制编入 cuda/directml 的 Rust EP 代码 + 触发 ort-sys 下载多余预编译成分（release profile 优化后 desktop 仍 54M，cuda/directml 占其中可观比例）。
2. **segfault 根源未除**：全开 feature 曾导致 macOS 上跨平台误注册 CUDA/DirectML EP，其 init 失败路径（dlopen libcuda 等）直接 SIGSEGV 绕过 Rust 错误处理。**代码层已修复**（`config.rs:424-432` 按 `#[cfg(target_os)]` 只注册本平台 EP），但 **feature 仍全开**——根源未除，未来代码回归时仍可能复发。

目标 app 需同时支持 mac/win/linux 三平台打包。

## 2. 目标

- 三平台各自**只启用对应硬件加速 EP**：mac→coreml、linux→cuda、win→directml。
- ort feature 与代码层 `#[cfg]` **1:1 对齐**（硬约束：不一致则编译失败）。
- 处理 Cargo feature unification 坑（确认在此结构下不构成问题）。
- 不引入构建脚本/CI 按平台传参的脆弱依赖。

## 3. 探索结论（关键）

1. **代码层（`config.rs:401-449` `apply_session_acceleration`）已按 `#[cfg(target_os)]` 分平台注册 EP**：mac=CoreML、linux=CUDA、win=DirectML+CUDA。含 `asr_hardware_accelerated` 开关、qwen3-asr 跳 CoreML（动态算子不兼容）、EP 注册失败 fallback CPU。代码层早就是对的。
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
ort = { version = "2.0.0-rc.12", default-features = false, features = ["download-binaries"] }

[target.'cfg(target_os = "macos")'.dependencies]
ort = { version = "2.0.0-rc.12", default-features = false, features = ["coreml"] }

[target.'cfg(target_os = "linux")'.dependencies]
ort = { version = "2.0.0-rc.12", default-features = false, features = ["cuda"] }

[target.'cfg(target_os = "windows")'.dependencies]
ort = { version = "2.0.0-rc.12", default-features = false, features = ["directml"] }
```

> **`default-features = false` 待实测**：ort 2.0-rc 的默认 feature 集未核实，若关闭后报缺默认项（如 std / ndarray 集成），则去掉 `default-features = false`、仅追加 feature。

### 5.3 代码层改动（`crates/asr/src/config.rs:428-432`）

win 块删 CUDA 注册（feature 未开 `cuda`，`CUDAExecutionProvider` 类型不存在会编不过）：

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
- 代码 `#[cfg]` 与 feature 矩阵 **1:1 对齐**（硬约束：任一平台两者不一致则该平台编译失败，编译器即验证）。

### 5.5 验证策略

- **mac 本地**：`cargo build/test -p octopus-asr` + desktop e2e（CoreML 正常工作、无回归）。
- **linux/win**：`cargo check --target <交叉 target>` 验证 feature 矩阵"编得过"（核心正确性）；**运行验证留用户自测**（环境无 linux/win GPU）。
- CI 是否三平台 check，留 plan 阶段确认（octopus 当前 CI 状态待查）。

## 6. 关键决策

1. **win 仅 DirectML（删 CUDA）**：DirectML 覆盖所有 DX12 GPU（NVIDIA/AMD/Intel 核显通吃），实时语音转写够用；省 cuda 预编译库体积；三平台单 EP 矩阵一致。代价：NVIDIA 卡无法走 CUDA（推理更快），但 YAGNI，DirectML 对实时转录足够。需同步删代码层 win 的 CUDA 注册。
2. **`default-features = false`**：意图避免 ort 默认拉多余 feature（减小体积/避免误触发其他 EP 下载），但是否误关必需项需实测；若报缺默认则去掉该标志，仅追加 feature。

## 7. 已知限制 / 风险

- **linux/win 运行未实测**：环境仅 mac，交叉 `cargo check` 只验证编译通过，不验证 EP 实际推理正确性。运行正确性靠用户在对应平台自测。
- **`default-features` 不确定**：ort 2.0-rc 默认 feature 集未核实，需实测。
- **体积收益未量化**：mac 删 cuda/directml feature 后二进制具体减多少，需 rebuild 对比（profile 优化已到 54M，再降幅度待测）。
- **CUDA 用户感知**：win NVIDIA 用户从 CUDA 回退到 DirectML，极端长音频批量转写可能略慢；实时录音转写影响可忽略。

## 8. 不做（YAGNI 边界）

- 不动 `asr_hardware_accelerated` 开关逻辑（现状保留）。
- 不动 qwen3-asr 跳 CoreML 逻辑（现状保留）。
- 不拆 per-platform crate（方案 C）。
- 不升级 ort 版本（仍 2.0.0-rc.12）。
- 不为 linux 额外支持 coreml、不为 mac 支持 cuda（跨平台无意义）。
