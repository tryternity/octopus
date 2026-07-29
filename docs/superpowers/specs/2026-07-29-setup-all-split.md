# setup_all 拆分 spec（run() 拆分第三步）

> **Status: 🔨 实施中**（2026-07-29，分支 `daily_bugfix_0729`）

## 背景

`setup.rs::AppSetup::setup_all` 587 行（第二步提取的 setup 闭包内容），内部有 ~12 段注释分节的独立逻辑块。按方案 A（结构体字段）拆成 12 个方法。

## 方案

### 结构体字段提升

跨段共享的 4 个变量提升为 `AppSetup` 的 `Option` 字段：

```rust
pub(crate) struct AppSetup<'a> {
    app: &'a tauri::App,
    config: &'a AppConfig,
    clipboard_handle: Option<Arc<ClipboardHandle>>,
    engine_manager: Option<Arc<AsrEngineManager>>,
    runtime_config: Option<SharedRuntimeConfig>,
    vault_session: Option<SharedVaultSession>,
}
```

### 12 个方法

| 方法 | 原~行数 | 职责 | 填充/消费字段 |
|---|---|---|---|
| init_clipboard | 30 | onboarding + clipboard_handle + 方言 + 热词 + 图片迁移 | 填充 clipboard_handle |
| init_cleanup | 14 | clipboard 自动清理 + 录屏孤儿清理 | — |
| init_scheduler | 54 | 通用调度器（定时清理 + vault 同步 + 索引刷新） | — |
| init_watchers | 107 | app/prompt 文件监听 + 索引校准 + bookmark 扫描 | — |
| init_input | 65 | focus_tracker + AX watcher + clipboard 队列 worker | — |
| create_windows | 26 | download/action_bar/overlay 窗口 + 录屏快捷键 | — |
| init_vault | 15 | vault_session + picker_url_cache | 填充 vault_session |
| init_engine | 98 | engine_manager + preheat + SystemStatusSampler | 填充 engine_manager |
| init_coordinator | 90 | runtime_config + Coordinator + RecordSession | 填充 runtime_config，消费 vault_session + engine_manager |
| init_tray | 20 | i18n + tray + locale listener | — |
| create_result_window | 3 | result_window 创建 | — |
| register_shortcuts | 17 | edit + polish 全局快捷键 | — |

### setup_all 变为 ~15 行串联

```rust
fn setup_all(&mut self) -> Result<()> {
    self.init_clipboard()?;
    self.init_cleanup();
    self.init_scheduler();
    self.init_watchers();
    self.init_input();
    self.create_windows();
    self.init_vault();
    self.init_engine();
    self.init_coordinator()?;
    self.init_tray();
    self.create_result_window();
    self.register_shortcuts();
    Ok(())
}
```

## 不变量
- 启动行为完全不变（纯代码搬家）
- 段的执行顺序不变（scheduler → watchers → input → ... → shortcuts）
- 共享变量通过 Option 字段 + unwrap/take 在消费段获取

## 风险
- 低。纯搬家，cargo build + cargo test（441）+ 手测启动验证
- &self → &mut self（字段填充），但 setup_all 是唯一调用方
