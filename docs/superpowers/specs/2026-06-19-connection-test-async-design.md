# 连接测试命令 async 重构设计

> Date: 2026-06-19
> 状态：设计中
> 关联：[2026-06-19-connection-test-design.md](./2026-06-19-connection-test-design.md)（连接测试功能本身，已实现）

## 1. 背景

连接测试命令（`test_llm_connection` / `test_asr_connection`，由 settings-ui 分支引入）当前实现为同步 `#[tauri::command] fn`：

- `test_llm_connection`（`settings_commands.rs:263`）：`std::thread::spawn(move || test_connection(&llm_cfg))` + `handle.join()`。注释称「避免 Tauri 命令超时」，但 `join()` 仍阻塞命令线程直至阻塞请求返回——spawn 只是把阻塞挪到子线程再等回，徒增线程创建/切换开销，命令线程仍被占住。
- `test_asr_connection`（`settings_commands.rs:283`）：`std::thread::spawn` + `tokio::runtime::Runtime::new()` + `block_on`。每次测试新建一个独立 tokio runtime，与 Tauri 内置 `tauri::async_runtime` 并存，开销且语义混乱（nested runtime 隐患）。

Tauri 2 的命令原生支持 `async fn`，且 `tauri::async_runtime` 已是项目在用的 tokio runtime（`coordinator.rs:905` 直接 `tauri::async_runtime::handle()`）。改 async 后命令跑在该 runtime 上，无需手动 spawn / new-runtime。

## 2. 目标

- 两个命令改 `async fn`，跑在 `tauri::async_runtime` 上。
- 删除 `std::thread::spawn + join`（LLM）与 `Runtime::new() + block_on`（ASR）。
- 前端 `invoke` 契约不变（命令名、入参、`Result<String, String>` 返回、错误文案）。

## 3. 非目标

- 不改连接测试业务逻辑（请求内容、超时阈值、错误文案）。
- 不改错误返回类型（保持 `Result<String, String>`，前端依赖字符串文案 showToast）。
- 不改 `main.rs` 的 `generate_handler!` 注册（async command 注册方式与 sync 相同，Tauri 自动适配）。
- 不改前端 `index.html`。

## 4. 方案

### 4.1 test_llm_connection

```rust
#[tauri::command]
pub async fn test_llm_connection(spec: String) -> Result<String, String> {
    if spec.is_empty() {
        return Err("未选择润色模型".into());
    }
    let llm_cfg = octopus_infra::db::load_llm_model(&spec)
        .map_err(|e| format!("从 DB 加载 LLM 配置失败: {}", e))?
        .ok_or_else(|| format!("DB 中未找到 LLM 模型 '{}'", spec))?;

    // reqwest::blocking 客户端跑在 spawn_blocking 线程池，不占用 async runtime worker
    tauri::async_runtime::spawn_blocking(move || octopus_llm::test_connection(&llm_cfg))
        .await
        .map_err(|_| "测试线程异常终止".to_string())?  // JoinError
        .map(|_| "连接成功".to_string())                  // test_connection: Result<()>
}
```

说明：`spawn_blocking` 返回 `JoinHandle<Result<()>>`，`.await` 得 `Result<Result<()>, JoinError>`：外层 `map_err` 处理线程 panic/取消，内层 `map` 处理 `test_connection` 成功。

### 4.2 test_asr_connection

前置校验（`is_local`、`entry`、`secret_key` 空）逻辑不变。WS 测试段改为直接 await：

```rust
#[tauri::command]
pub async fn test_asr_connection(bare_name: String) -> Result<String, String> {
    // ... 前置校验同现状（list_engines / is_local / entry / secret_key 空）...
    #[cfg(feature = "dashscope")]
    {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = endpoint.into_client_request()
            .map_err(|e| format!("WS 端点无效: {}", e))?;
        req.headers_mut().insert(
            "Authorization",
            format!("bearer {}", key).parse().unwrap(),
        );
        // 直接在 tauri::async_runtime 上 await，删除 Runtime::new + block_on
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
    { Err("远程 ASR 连接测试需要 dashscope feature".into()) }
}
```

说明：async command 在 `tauri::async_runtime`（tokio）上下文执行，`connect_async` 对 tokio runtime 的要求天然满足，删除 `Runtime::new` 后无 nested runtime 问题。

### 4.3 契约不变性

前端 `crates/desktop/dist/settings/index.html` 的 `testLlmConnection` / `testAsrConnection` 调 `invoke('test_llm_connection', { spec })` / `invoke('test_asr_connection', { bareName })`，返回 `Promise<Result<string,string>>`。Tauri 对 sync / async command 在前端侧表现完全一致（自动 wrap 为 Promise）。无需改前端。

## 5. 文件清单

| 文件 | 改动 |
|---|---|
| `crates/desktop/src/settings_commands.rs` | 2 个 fn 改 `async fn`；LLM 用 `spawn_blocking`，ASR 删 `Runtime::new` 直接 await；纯逻辑单测保持 |
| `crates/desktop/src/main.rs` | 不动（`generate_handler!` 注册不变） |
| 前端 `index.html` | 不动 |

## 6. 风险

- **低**。`reqwest::blocking` 跑在 `spawn_blocking` 线程池，不污染 async runtime；`connect_async` 在 tauri runtime 上，删 nested runtime 反而更安全。
- 单测覆盖纯逻辑（spec 解析、`is_local` 判定、`secret_key` 空检查）；WS 连通不便单测，沿用现有手动验证（设置窗口点测试按钮）。

## 7. 验证

- `cargo check -p octopus-desktop --features dashscope`
- 手动：设置窗口点「测试连接」，LLM + ASR（aliyun 远程）各验一次成功/失败文案与重构前一致。
