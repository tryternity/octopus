# 跨平台兼容性复查与修复

**日期**：2026-07-05
**范围**：针对一份外部「macOS/Windows/Linux 跨平台兼容性评估报告」逐项复查，修复真实问题，反驳不实指控。

## 背景

收到一份涵盖 5 大维度（GUI 窗口、截图性能、原生库依赖、系统交互、文件系统）的跨平台评估报告，共提出 11 个问题点。本 spec 记录逐项复查结论——**不轻信 bug 报告**，以代码实际状态为准。

## 复查结论汇总

| # | 报告指控 | 结论 | 处理 |
|---|---------|------|------|
| 1.1 | 非 macOS 点击穿透 poller 为空 | **属实** | 已修复：统一为跨平台 poller |
| 1.2 | activation.rs macOS 独占致编译失败 | **不实** | `set_activation_policy` 是 Tauri 跨平台 API；所有 `ns_window()` 已 `#[cfg(macos)]` 门控 |
| 1.3 | 多屏高 DPI 缩放定位错位 | **已处理** | poller 用物理坐标 + `scale_factor` 换算；截图全程物理像素 |
| 2.1 | 非 macOS 截图热循环 PNG 编解码往返 | **属实** | 已修复：新增 `crop_region_rgba` |
| 3.1 | ONNX Runtime DLL/SO 分发 | **架构问题** | 非代码缺陷，属打包配置（`tauri.conf.json` resources），不在本次修复范围 |
| 3.2 | ASR 硬件加速 segfault 风险 | **已缓解** | 平台门控已做（`#[cfg]` 按 OS 注册 EP）；EP 注册失败 catch 回退 CPU。segfault 属 ort C++ 上游限制 |
| 3.3 | DF3/OCR SIMD 指令集 SIGILL | **部分有效** | 已改善：DF3 失败降级到 RNNoise（原为直通）。实际用 tract 纯 Rust（内部 SIMD 检测），SIGILL 风险极低 |
| 4.1 | Linux Wayland 剪贴板后台监听失效 | **属实但无解** | Wayland 协议层安全限制，非代码缺陷。文档标注，建议 XWayland |
| 4.2 | Windows 剪贴板文件路径格式 | **不实** | 已正确 `cmd /C start "" path`；Rust stdlib 自动加引号；`decode_file_uri` 仅对 `file://` 解码 |
| 5.1 | dlp `0o755` 权限致 Windows 编译失败 | **不实** | 代码已 `#[cfg(unix)]` 门控；`.exe` 后缀已平台化处理 |
| 5.1b | dlp 硬编码斜杠 | **不实** | 已用 `PathBuf::join` + 平台化 URL/扩展名 |

## 已实施的修复

### 修复 1：统一跨平台点击穿透 poller（#1.1）

**问题**：`start_click_through_poller` 在 `#[cfg(not(target_os = "macos"))]` 下是空函数，精简态结果窗的透明区在 Windows/Linux 拦截后方应用点击。

**根因**：历史遗留——poller 体仅用跨平台 Tauri API（`cursor_position` / `outer_position` / `scale_factor` / `is_visible`），却被错误地包在 `#[cfg(target_os = "macos")]` 内。平台差异只在 `set_result_ignores_mouse`（macOS 直调 NSWindow vs 非 macOS 用 `set_ignore_cursor_events`），而后者已有双分支。

**修复**：移除 poller 的 `#[cfg]` 门控，统一为单函数。文档标注 Wayland 限制。

**已知限制**：Linux **Wayland** 禁止后台读全局光标，tao 恒返回 `(0,0)` → 轮询判定光标恒在小条外，整窗穿透。这是 Wayland 协议层安全限制，无代码层解法（改用 XWayland 可恢复 X11 行为）。

### 修复 2：截图热路径移除 PNG 编解码往返（#2.1）

**问题**：非 macOS 滚动截帧在 30ms 热循环中对每帧执行 `capture_all_monitors` → `crop_region`（裁剪 + PNG 编码）→ `load_from_memory`（PNG 解码）→ `to_rgba8`。4K/多屏下 CPU 瞬间跑满。

**修复**：`crates/capx/src/capture.rs` 新增 `crop_region_rgba()` 直接返回 `RgbaImage`（零 PNG 编解码）。`screenshot_commands.rs` 两处热路径（首帧 + 循环帧）改用新函数。原 `crop_region()`（返回 PNG bytes）保留——一次性截图（行 517、621）仍需 PNG 输出。

### 修复 3：DF3 加载失败降级到 RNNoise（#3.3）

**问题**：DF3 模型加载失败时降级到 pass-through（无降噪），用户体验差。

**修复**：`denoise.rs` 的 `process_samples` 懒加载分支，DF3 `Err` 时构造 `RnnoiseBackend` 而非 `None`。仅 RNNoise 也 OOM 才最终直通。

**注**：实际 SIGILL 风险极低——octopus 用 tract 纯 Rust 后端（非 C libDF），tract 内部做运行时 SIMD 特征检测并优雅回退。`catch_unwind` 兜底 Rust panic。

## 未实施（理由充分）

- **#3.1 ONNX Runtime 打包**：属 `tauri.conf.json` 打包配置 + CI/CD 问题，非代码缺陷。当前 macOS 分发已含 `ort` 静态链接策略，Windows/Linux 分发待实际移植时处理。
- **#3.2 EP 注册前置 dlopen 探测**：ort 的 C++ EP 注册失败可能 segfault（绕过 Rust catch），但当前已按平台隔离注册（macOS 仅 CoreML、Linux 仅 CUDA、Windows 仅 DirectML），把跨平台全注册的 segfault 路径消除。进一步 dlopen 预探测是 ort 上游应做的事，应用层 hack `libcuda.so` 探测会增加维护负担且无法覆盖所有失败模式。
- **#4.1 Wayland 剪贴板**：协议层限制，代码无解。

## 验证

- `cargo build -p octopus-capx` ✅
- `cargo build -p octopus-asr-local` ✅
- `cargo build -p octopus-desktop --features embedded` ✅
- `cargo test -p octopus-asr-local --lib denoise` ✅（6 passed, 6 ignored[需真实模型], 0 failed）
- `cargo test -p octopus-capx` ✅（19 passed, 0 failed）
