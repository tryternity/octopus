# setup_all 拆分 spec（run() 拆分第三步）

> **Status: ✅ 已实现**（2026-07-29，分支 `daily_bugfix_0729`，commit 待提交）

## 背景

`setup.rs::AppSetup::setup_all` 587 行（第二步提取的 setup 闭包内容），内部有 ~12 段注释分节的独立逻辑块。按方案 A（结构体字段）拆成 12 个方法。

## 实施记录

### 结构体字段提升（从 4 → 2）

**实施偏差**：spec 原方案列了 4 个字段，但按「跨段共享变量才提升为字段」的原则追溯依赖链后，实际只有 **2** 个真正跨段共享——其余 2 个自包含于方法内部，保留为局部变量更干净（避免永不 take 的死字段）。

依赖链追溯结果：

| 变量 | 创建点 | 消费点 | 判定 |
|---|---|---|---|
| `clipboard_handle` | init_clipboard | init_input（watcher/worker 复用） | ✅ 字段 |
| `engine_manager` | init_engine | init_coordinator（build_local_engine / DispatchEngine） | ✅ 字段 |
| `runtime_config` | init_coordinator L481 | init_coordinator L517（Coordinator::new） | ❌ 局部（同方法内创建+消费） |
| `vault_session` | init_vault | `set_global_session` move 消费，Coordinator::new **不接收** | ❌ 局部（move 后不再跨段用） |

最终结构体：

```rust
pub(crate) struct AppSetup<'a> {
    app: &'a tauri::App,
    config: &'a AppConfig,
    /// 跨段共享：init_clipboard 创建，init_input watcher/worker 复用。
    clipboard_handle: Option<Arc<octopus_clipboard::ClipboardHandle>>,
    /// 跨段共享：init_engine 创建，init_coordinator build_local_engine / DispatchEngine 复用。
    engine_manager: Option<Arc<octopus_asr_local::engine::AsrEngineManager>>,
}
```

消费端用 `self.clipboard_handle.clone().expect(...)`（保留语义化 panic message），非 `.take().unwrap()`——clone 即可，handle 在后续 Tauri State 还要继续用。

### 12 个方法

| 方法 | 原~行数 | 职责 | 填充/消费字段 |
|---|---|---|---|
| init_clipboard | 30 | onboarding + clipboard_handle + 方言 + 热词 + 图片迁移 | 填充 clipboard_handle |
| init_cleanup | 14 | clipboard 自动清理 + 录屏孤儿清理 | — |
| init_scheduler | 54 | 通用调度器（定时清理 + vault 同步 + 索引刷新） | — |
| init_watchers | 107 | app/prompt 文件监听 + 索引校准 + bookmark 扫描 | — |
| init_input | 65 | focus_tracker + AX watcher + clipboard 队列 worker | 消费 clipboard_handle |
| create_windows | 26 | download/action_bar/overlay 窗口 + 录屏快捷键 | — |
| init_engine | 98 | engine_manager + preheat + SystemStatusSampler | 填充 engine_manager |
| init_vault | 15 | vault_session + picker_url_cache（局部变量） | — |
| init_coordinator | 90 | runtime_config + Coordinator + RecordSession | 消费 engine_manager |
| init_tray | 20 | i18n + tray + locale listener | — |
| create_result_window | 3 | result_window 创建 | — |
| register_shortcuts | 17 | edit + polish 全局快捷键 | — |

### 执行顺序（与原代码一致）

**实施偏差**：spec 表格原顺序是 vault → engine，但原代码 `vault_session` 创建在 `engine_manager` 之后（L383 engine vs L495 vault），所以正确顺序是 **engine → vault → coordinator**。已按原代码顺序保持不变。

```rust
fn setup_all(&mut self) -> Result<()> {
    self.init_clipboard()?;
    self.init_cleanup();
    self.init_scheduler();
    self.init_watchers();
    self.init_input();
    self.create_windows();
    self.init_engine()?;      // engine 在 vault 前（原代码 L383 < L495）
    self.init_vault();
    self.init_coordinator();
    self.init_tray();
    self.create_result_window();
    self.register_shortcuts();
    Ok(())
}
```

### 错误传播调整

- `init_clipboard` / `init_engine` 保留 `?`（clipboard handle 创建 + engine 初始化可能失败）
- 其余 10 个方法无 `Result` 返回（原代码中它们就是 fire-and-forget / warn 不阻断）
- 注意：原 `init_coordinator` 的 `?` 已去掉——其内部所有操作都是 `log::warn/error` 不 propagate（Coordinator::new / AudioRecorder 失败都是 graceful fallback）

## 验证结果

- ✅ `cargo build -p octopus-desktop --features embedded` — 0 error 0 warning
- ✅ `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error 0 warning
- ✅ `cargo test -p octopus-desktop` — **441 passed, 0 failed, 1 ignored**
- ⏳ 手测启动验证（用户侧）

## 不变量
- 启动行为完全不变（纯代码搬家）
- 段的执行顺序不变（scheduler → watchers → input → ... → shortcuts）
- 共享变量通过 Option 字段 + clone 在消费段获取

## 风险
- 低。纯搬家，cargo build + cargo test（441）+ 手测启动验证
- &self → &mut self（字段填充），但 setup_all 是唯一调用方
