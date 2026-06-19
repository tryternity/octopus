# 连接测试 async 重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 按任务实施。步骤用 `- [ ]` 跟踪。

**Goal:** `test_llm_connection` / `test_asr_connection` 改 `async fn`，跑在 `tauri::async_runtime`，删除手动 `thread::spawn + join` 与 `Runtime::new()`，前端 invoke 契约不变。

**Architecture:** 两个 `#[tauri::command]` 改 `async`。LLM（`reqwest::blocking`）包 `tauri::async_runtime::spawn_blocking`；ASR（`tokio-tungstenite` WS）直接 `.await`。`generate_handler!` 注册方式不变，前端 `invoke` 契约不变。

**Tech Stack:** Tauri 2 async command, `tauri::async_runtime`, `tokio-tungstenite`, `reqwest::blocking`。

**Spec:** `docs/superpowers/specs/2026-06-19-connection-test-async-design.md`

> **状态：已实现**（commits `b2b67b3` + `6bd791a`，merge main；GUI 已验证）。Task 1 实施时发现 `test_connection` 返回 `anyhow::Error`，闭包内补 `.map_err(|e| format!("{}", e))` 转 `String`（plan 代码已同步，commit `af809c8`）。

---

## File Structure

- `crates/desktop/src/settings_commands.rs` — 仅此一个文件，改 2 个 fn。纯逻辑单测保持不变。

---

### Task 1: test_llm_connection 改 async

**Files:** Modify `crates/desktop/src/settings_commands.rs:260-278`

- [x] **Step 1: 改签名 + 实现**

将 `pub fn test_llm_connection` 整体替换为：

```rust
/// 测试 LLM 连接是否可用（发一个 max_tokens=1 的极简请求）。
/// spec 为 polish_llm 配置值（3-part spec 或裸名），从 DB 加载配置后测试连通性。
#[tauri::command]
pub async fn test_llm_connection(spec: String) -> Result<String, String> {
    if spec.is_empty() {
        return Err("未选择润色模型".into());
    }
    let llm_cfg = octopus_infra::db::load_llm_model(&spec)
        .map_err(|e| format!("从 DB 加载 LLM 配置失败: {}", e))?
        .ok_or_else(|| format!("DB 中未找到 LLM 模型 '{}'", spec))?;

    // reqwest::blocking 客户端跑在 spawn_blocking 线程池，不占用 async runtime worker。
    // test_connection 返回 Result<(), anyhow::Error>：闭包内先 map_err 转 String，
    // 使 spawn_blocking 返回 JoinHandle<Result<(), String>>，.await 后链式匹配 Result<String, String>。
    tauri::async_runtime::spawn_blocking(move || {
        octopus_llm::test_connection(&llm_cfg).map_err(|e| format!("{}", e))
    })
        .await
        .map_err(|_| "测试线程异常终止".to_string())?
        .map(|_| "连接成功".to_string())
}
```

说明：`spawn_blocking` 返回 `JoinHandle<Result<()>>`，`.await` 得 `Result<Result<()>, JoinError>`——外层 `map_err` 处理线程 panic/取消，内层 `map` 处理 `test_connection` 成功。

- [x] **Step 2: 编译验证**

Run: `cargo check -p octopus-desktop --features dashscope`
Expected: PASS（`main.rs` 的 `generate_handler!` 注册不变，async command 自动支持）

- [x] **Step 3: commit**

```bash
git add crates/desktop/src/settings_commands.rs
git commit -m "refactor(desktop): test_llm_connection 改 async + spawn_blocking"
```

---

### Task 2: test_asr_connection 改 async

**Files:** Modify `crates/desktop/src/settings_commands.rs:280-339`

- [x] **Step 1: 改签名 + 删 Runtime::new，WS 直接 await**

将 `pub fn test_asr_connection` 整体替换为（前置校验逻辑不变，仅签名 + WS 测试段改）：

```rust
/// 测试 ASR 远程引擎连接是否可用。
/// 本地模型返回 Err 提示无需连接测试；远程模型（provider=aliyun）检查 secret_key + WS 连通性。
#[tauri::command]
pub async fn test_asr_connection(bare_name: String) -> Result<String, String> {
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    let engine = engines.iter().find(|e| e.name == bare_name)
        .ok_or_else(|| format!("ASR 引擎 '{}' 不存在", bare_name))?;

    if engine.is_local {
        return Err("本地模型无需连接测试".into());
    }

    // 远程引擎：从 DB 取配置（source = WS endpoint, secret_key = API Key）
    let asr_cfg = octopus_asr::config::load_config().map_err(|e| e.to_string())?;
    let model_name = octopus_infra::db::parse_model_spec(&bare_name).model_name().to_string();
    let entry = asr_cfg.asr.aliyun.as_ref()
        .and_then(|m| m.get(model_name.as_str()))
        .ok_or_else(|| format!("远程 ASR 模型 '{}' 未在 DB 配置", bare_name))?;

    if entry.secret_key.is_empty() {
        return Err(format!("ASR 模型 '{}' 的 secret_key 为空", bare_name));
    }

    #[cfg(feature = "dashscope")]
    {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = entry.source.clone().into_client_request()
            .map_err(|e| format!("WS 端点无效: {}", e))?;
        req.headers_mut().insert(
            "Authorization",
            format!("bearer {}", entry.secret_key).parse().unwrap(),
        );
        // 直接在 tauri::async_runtime 上 await，不再 thread::spawn + Runtime::new + block_on
        match tokio::time::timeout(
            std::time::Duration::from_secs(3),
            tokio_tungstenite::connect_async(req),
        ).await {
            Ok(Ok(_)) => Ok("连接成功".into()),
            Ok(Err(e)) => Err(format!("WS 连接失败: {}", e)),
            Err(_) => Err("WS 连接超时（3s）".into()),
        }
    }
    #[cfg(not(feature = "dashscope"))]
    {
        Err("远程 ASR 连接测试需要 dashscope feature".into())
    }
}
```

- [x] **Step 2: 编译验证**

Run: `cargo check -p octopus-desktop --features dashscope`
Expected: PASS（`connect_async` 在 tauri runtime 上下文，删 nested runtime 后无冲突）

- [x] **Step 3: commit**

```bash
git add crates/desktop/src/settings_commands.rs
git commit -m "refactor(desktop): test_asr_connection 改 async，删 Runtime::new"
```

---

### Task 3: 回归验证

- [x] **Step 1: 现有单测通过**

Run: `cargo test -p octopus-desktop`
Expected: PASS（纯逻辑单测——spec 解析、`is_local` 判定、`secret_key` 空检查——不受 async 改造影响）

- [x] **Step 2: 手动验证契约不变（需 GUI 环境）**

- 设置窗口选远程 LLM → 点测试 → 成功/失败文案与重构前一致
- 设置窗口选 aliyun ASR → 点测试 → 成功/失败文案一致
- 本地 ASR → 按钮灰 + 提示「本地模型无需连接测试」

- [x] **Step 3: workspace 整体编译**

Run: `cargo check --workspace --all-targets`
Expected: PASS，零 warning 回归

---

## Self-Review

- **Spec 覆盖**：§4.1 LLM async（Task 1）、§4.2 ASR async（Task 2）、§4.3 契约不变（Task 3 手动）✓
- **Placeholder 扫描**：无 TBD/TODO；两个 fn 给完整代码 ✓
- **类型一致**：`spawn_blocking` → `JoinHandle<Result<()>>` → `.await` → `Result<Result<()>, JoinError>` → `map_err` + `map` 链正确；ASR `connect_async` 返回 `Result<(WSStream, Response), Error>`，`timeout` 包一层 → 三臂 match 覆盖全 ✓
