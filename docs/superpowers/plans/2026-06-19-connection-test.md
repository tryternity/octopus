# 设置页连接测试按钮 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ⚠️ 命令实现为 `async fn`——见 [2026-06-19-connection-test-async.md](./2026-06-19-connection-test-async.md)（LLM `spawn_blocking` / ASR 直接 `await connect_async`）。本 plan Task 1（`test_connection` 函数）+ Task 3（前端 UI）仍有效；Task 2 命令实现已迁至 async plan，仅保留注册步骤。

**Goal:** 在设置页「语音识别引擎」和「文本润色模型」两个 select 旁加连接测试按钮，远程模型可点（WS 握手 / chat max_tokens=1），本地模型灰掉；三态视觉反馈。

**Architecture:**
- 后端新增 2 个 Tauri 命令（`crates/desktop/src/settings_commands.rs`）：`test_llm_connection(spec)` + `test_asr_connection(bare_name)`，均为 `async fn`（LLM `spawn_blocking` 包 `reqwest::blocking`，ASR 直接 `await connect_async`）——命令实现详见 [2026-06-19-connection-test-async.md](./2026-06-19-connection-test-async.md)
- LLM 测试逻辑抽到 `crates/llm/src/client.rs::test_connection`（复用 `ChatRequest`，`max_tokens=1`）
- ASR 测试内联在命令实现里（`#[cfg(feature="dashscope")]` 包围，仅握手不发协议帧）
- 前端 `crates/desktop/dist/settings/index.html`：每个 select 包 flex 容器 + `.test-btn`，三态 CSS + JS 联动

**Tech Stack:** Rust + Tauri 2 + reqwest blocking + tokio-tungstenite + vanilla HTML/CSS/JS

**设计 spec:** `docs/superpowers/specs/2026-06-19-connection-test-design.md`

> **状态（2026-06-19）：已实施**（commits `819777d` LLM 按钮 + `3f96a31` ASR 按钮 + `e2cd7a8` check.svg 图标）。下方 checkbox 标记实际完成进度。

---

## 文件结构

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/llm/src/client.rs` | `test_connection()` 实现 | **新增函数** |
| `crates/llm/src/lib.rs` | 公开导出 | **修改 re-export** |
| `crates/desktop/src/settings_commands.rs` | 2 个 Tauri 命令 | **新增命令** |
| `crates/desktop/src/main.rs` | `invoke_handler` 注册 | **追加 2 项** |
| `crates/desktop/dist/settings/index.html` | UI（DOM + CSS + JS） | **修改** |
| `crates/desktop/dist/result/icons/check.svg` | FontAwesome check 图标源 | **新增资源** |

## 测试策略

无单测（UI + 网络命令，YAGNI）。每个 task 手动验证：
- Task 1：`cargo build -p octopus-llm` 编译通过
- Task 2：`cargo build -p octopus-desktop --features dashscope` 编译通过
- Task 3：启动应用 → 设置页 → ASR 选本地 → 按钮灰 / 选远程 → 点击 → 绿/红切换；LLM 选任意 → 点击 → 绿/红切换

---

## Task 1: LLM `test_connection` 函数

- [x] `crates/llm/src/client.rs` 新增 `pub fn test_connection(config: &CompatibleLlmConfig) -> Result<()>`
  - 复用 `ChatRequest` 结构 + `needs_disable_thinking()` 逻辑
  - messages=[{"role":"user","content":"Hi"}]，`max_tokens=1`，`temperature=0.0`
  - `reqwest::blocking::Client::builder().timeout(10s).build()`
  - 失败：`anyhow::context` 网络错误 / `bail!` 非 2xx + body
- [x] `crates/llm/src/lib.rs` re-export：`pub use client::{polish, test_connection};`
- [x] `cargo build -p octopus-llm` 通过

## Task 2: 两个 Tauri 命令注册

> **命令实现已重构为 `async fn`**——见 [2026-06-19-connection-test-async.md](./2026-06-19-connection-test-async.md) Task 1/2（LLM `spawn_blocking`、ASR 直接 `await connect_async`，删 `thread::spawn` + `Runtime::new`）。下方仅保留注册步骤（async/sync 注册方式一致）。

- [x] `crates/desktop/src/main.rs` 的 `invoke_handler![...]` 追加 `test_llm_connection` + `test_asr_connection`（async command 注册方式与 sync 相同，Tauri 自动适配）
- [x] `cargo build --release -p octopus-desktop --features "embedded dashscope"` 通过

## Task 3: 前端 UI（DOM + CSS + JS）

- [x] **资源**：`crates/desktop/dist/result/icons/check.svg` 新增（FontAwesome check 640×640 viewBox path）
- [x] **CSS**（`<style>` 内）：新增 `.test-btn`（32×32 圆角，hover 边框/图标变 primary）、`.test-btn.ok`（绿 #22c55e）、`.test-btn.fail`（红 #ef4444）、`.test-btn.loading`（半透明 + `pointer-events:none`）、`.test-btn.disabled`（`opacity:0.3` + `pointer-events:none`）、`.select-with-test`（flex 容器）
- [x] **JS 常量**：`const checkIconSvg = '<svg>...check.svg path...</svg>'`（内联，避免运行时加载）；`let asrEnginesData = []`（缓存引擎列表供 `updateAsrTestBtn` 查 `is_local`）
- [x] **renderSettings 改动**：
  - 缓存 `asrEnginesData = resp.asr_engines`
  - 求当前选中 ASR 引擎的 `is_local`（`currentAsrLocal`）
  - ASR select 包 `.select-with-test` + `<button class="test-btn{disabled}" id="asr-test-btn" onclick="testAsrConnection()">`，select 加 `onchange="...updateAsrTestBtn(this.value)"`
  - Polish LLM select 同样包 `.select-with-test` + `<button class="test-btn" id="llm-test-btn" onclick="testLlmConnection()">`（无 disabled 初始态）
- [x] **JS 函数**：
  - `testLlmConnection()`：取 select value → 先 `invoke('set_config', {key:'polish_llm', value:bareName})` 持久化 → `invoke('test_llm_connection', {spec:bareName})` → 切 ok/fail + showToast
  - `testAsrConnection()`：取 select value → `disabled` class 直接 return → `invoke('test_asr_connection', {bareName})` → 切 ok/fail + showToast
  - `updateAsrTestBtn(bareName)`：从 `asrEnginesData` 查 `is_local` → 切 `disabled` class + title
  - 三个函数都 `window.xxx = xxx` 显式挂全局（Tauri webview inline event handler 限制）
- [x] 启动应用 e2e：本地 ASR 灰 / 远程 ASR 可点 / LLM 可点 / 成功绿失败红 / loading 半透明

---

## 已知后续工作

- ASR 测试目前只验握手——未来可考虑发一个空 PCM 帧跑完整协议初始化（消耗 1 次 DashScope 调用，但能验 model_name 拼写）
- 抽 `check.svg` 的 path 为前端共享常量（当前 HTML 内联 + 独立 SVG 文件并存）
