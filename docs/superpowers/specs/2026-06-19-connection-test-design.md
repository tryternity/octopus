# 设置页连接测试按钮设计

> Date: 2026-06-19
> 状态：已实现（commits `819777d` + `3f96a31` + `e2cd7a8`）

## 1. 背景

设置页「语音识别引擎」和「文本润色模型」两个 select 切到远程模型后，用户没有直观方式确认配置（endpoint + API Key）是否有效——只能开始录音、等报错才知道。新增两个连接测试按钮，让用户在保存配置前先验证连通性。

## 2. 目标

- ASR 引擎 select 右侧加一个测试按钮：
  - **本地模型** → 灰掉（`disabled`，`pointer-events:none`，title「本地模型无需测试」）
  - **远程模型**（provider=aliyun）→ 可点，3s WS 握手连通性检测
  - select 切换时按 `is_local` 动态刷新按钮 disabled 状态
- 润色模型 select 右侧加一个测试按钮：始终可点，发一个 `max_tokens=1` 的极简 chat 请求（10s 超时）
- 三态视觉反馈：默认（灰边框）/ 成功（绿 #22c55e）/ 失败（红 #ef4444）；点击中 `loading`（半透明 + 禁用）
- 不消耗大量 API 额度（LLM 仅 1 token；ASR 仅握手不发协议帧）

## 3. 接口

### 3.1 新增 Tauri 命令

文件：`crates/desktop/src/settings_commands.rs`

```rust
#[tauri::command]
pub fn test_llm_connection(spec: String) -> Result<String, String>;
//   入参：spec = polish_llm 配置值（3-part spec 或裸名）
//   返回：Ok("连接成功") / Err("<错误信息>")
//   实现：load_llm_model(spec) → 独立线程跑 octopus_llm::test_connection

#[tauri::command]
pub fn test_asr_connection(bare_name: String) -> Result<String, String>;
//   入参：bare_name = ASR 引擎裸名（前端 select 的 value）
//   返回：Ok("连接成功") / Err("<错误信息>")
//   实现：list_engines().find(name) → is_local 则 Err("本地模型无需测试")
//         否则取 DB endpoint+key → 独立线程建 tokio runtime
//         → tokio::time::timeout(3s, connect_async(req))
```

注册：`crates/desktop/src/main.rs::run` 的 `invoke_handler` 列表追加两个命令。

### 3.2 LLM 测试实现

文件：`crates/llm/src/client.rs`

```rust
pub fn test_connection(config: &CompatibleLlmConfig) -> Result<()>;
```

- 复用 `ChatRequest` 结构，messages=[{"user","Hi"}]，`max_tokens=1`，`temperature=0.0`
- 按 `needs_disable_thinking()` 走与 `polish` 一致的 thinking 关闭逻辑（DeepSeek 用 `thinking.kind="disabled"`，其他用 `enable_thinking=false`）
- `reqwest::blocking::Client`，10s 超时
- 失败路径：网络/构建错误 → `anyhow::context`；非 2xx → `bail!("LLM API 返回错误 {}: {}", status, body)`

`crates/llm/src/lib.rs` 导出：`pub use client::{polish, test_connection};`

### 3.3 ASR 测试实现

内联在 `test_asr_connection` 内：

- `parse_model_spec(&bare_name).model_name()` 取裸名 → 查 `cfg.asr.aliyun[model_name]`
- `secret_key` 空 → Err 提示
- `#[cfg(feature = "dashscope")]` 分支：独立线程 → `tokio::runtime::Runtime::new()` → `rt.block_on` 跑 `tokio::time::timeout(3s, connect_async(req))`，req 经 `IntoClientRequest` + 追加 `Authorization: bearer <key>` header
- `#[cfg(not(feature = "dashscope"))]` → Err「远程 ASR 连接测试需要 dashscope feature」

**关键：仅验证 WS 握手成功，不发任何协议帧（run-task / session.update 都不发）**——避免消耗 DashScope 推理额度。握手成功即代表 endpoint + key 有效。

## 4. 前端 UI

文件：`crates/desktop/dist/settings/index.html`

### 4.1 DOM 结构

每个 select 包一层 `.select-with-test` flex 容器，select + 32×32 `.test-btn`：

```html
<div class="select-with-test">
  <select id="asr-engine-select" onchange="setVal('asr_engine', this.value); updateAsrTestBtn(this.value)">
    ...options...
  </select>
  <button class="test-btn disabled" id="asr-test-btn" onclick="testAsrConnection()" title="本地模型无需测试">
    <svg>...check.svg path...</svg>
  </button>
</div>
```

### 4.2 CSS（`.test-btn`）

- 默认：白底 + `var(--border)` 1px 边框 + 6px 圆角 + hover 边框/图标变 primary
- 三态：`.ok`（绿边框 + 绿图标）/ `.fail`（红边框 + 红图标）/ `.loading`（半透明 + `pointer-events:none`）
- `.disabled`：`opacity:0.3` + `pointer-events:none`（ASR 本地模型用）

### 4.3 JS 逻辑

- **LLM 测试**（`testLlmConnection`）：取 polish-llm-select 裸名 → **先 `set_config('polish_llm', value)` 持久化**（确保后端从 DB 读到最新 spec）→ `invoke('test_llm_connection', {spec: bareName})` → 切 ok/fail + `showToast`
- **ASR 测试**（`testAsrConnection`）：取 asr-engine-select 裸名 → `disabled` class 直接 return → `invoke('test_asr_connection', {bareName})` → 切 ok/fail + `showToast`
- **按钮状态联动**（`updateAsrTestBtn(bareName)`）：从缓存的 `asrEnginesData`（`renderSettings` 时缓存 `resp.asr_engines`）查 `is_local` → 本地加 `disabled` + title「本地模型无需测试」；远程移除 `disabled` + title「测试连接」。同时清掉历史 ok/fail 残留态。

## 5. 关键决策

1. **不抽独立引擎类、不发协议帧**：ASR 测试仅握手（connect_async），不进 `run-task` / `session.update`。理由：握手成功 ⇔ endpoint+key 有效，足够回答「能不能用」的问题，不消耗推理额度。
2. **独立线程跑阻塞请求**：LLM 用 `reqwest::blocking`，ASR 用独立 tokio runtime——避免 Tauri 命令超时（默认 800ms warning）和污染主 runtime。
3. **LLM 测试前先 `set_config` 持久化**：`test_llm_connection` 后端按 spec 从 DB 加载配置，必须确保 DB 中 `polish_llm` 是用户刚选中的值（与 set_config 内部 `build_polish_llm_spec` 一致的裸名）。
4. **ASR 测试不持久化**：`test_asr_connection` 接收 `bare_name` 直接查 DB endpoint——若用户改了 select 但还没触发 `setVal`，会测旧值；但用户从 select 切换到点按钮中间一般有 setVal 触发，可接受。
5. **图标源**：`crates/desktop/dist/result/icons/check.svg`（FontAwesome check，640×640 viewBox）——前端内联 SVG path（避免运行时加载），独立 SVG 文件保留作资源备份。

## 6. 已知限制

- ASR 测试只验 WS 握手，不验协议帧正确性（如 model_name 拼写错只能等真录音报错）
- LLM 测试发的是真实 chat 请求（即使 `max_tokens=1`），极小额度消耗
- 测试期间用户可重复点击——靠 `loading` class 的 `pointer-events:none` 拦截，但若回调丢失（极罕见）按钮会卡 loading
