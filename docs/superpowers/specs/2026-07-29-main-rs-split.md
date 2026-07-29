# main.rs run() 拆分 spec（第一步：bootstrap 提取）

> **Status: 🔨 实施中**（2026-07-29，分支 `daily_bugfix_0729`）

## 背景

`main.rs::run()` 1266 行，是全工程最长函数。按 brainstorming 决策，**一步步拆**：先提取初始化逻辑（第一步），setup 闭包后续再评估。

## 第一步范围

把 run() 开头的**初始化逻辑**（L125-258，~133 行）提取到 `bootstrap.rs`：

- panic hook 设置
- config 加载（`load_config`，失败用 default）
- DB 初始化（`ensure_db`）
- 模型路径软链（`create_model_symlinks`）
- builtin 模型 is_available 同步（`sync_builtin_models_availability`）
- 4 域激活引擎预热（`load_active_engine`）
- 搜索引擎初始化（`init_search_engine`）
- 引擎模式校验
- 润色配置校验（三档模式 + LLM 配置）
- 润色 prompt 加载（DB → 文件 → `set_system_prompt`）

## 设计

### 新建 `crates/desktop/src/bootstrap.rs`

```rust
/// 应用启动初始化（panic hook → config → DB → 模型 → 引擎预热 → 润色配置 → prompt）。
/// 返回加载的 AppConfig 供后续 builder/setup 使用。
pub(crate) fn bootstrap() -> octopus_infra::config::AppConfig { ... }
```

### main.rs run() 改为

```rust
pub fn run() {
    let config = bootstrap::bootstrap();
    let mut app = tauri::Builder::default()
        .plugin(...)
        .invoke_handler(generate_handler![...])  // 命令列表不动
        .setup(move |app| { ... })               // setup 闭包不动
        .run(...);
}
```

### main.rs 加 mod 声明

```rust
mod bootstrap;
```

## 不变量
- 启动行为完全不变（纯代码搬家，无逻辑变更）
- setup 闭包不动（741 行，后续步骤评估）
- 命令列表不动（Tauri generate_handler! 宏限制）
- bootstrap 返回 `AppConfig`（setup 闭包需要 config）

## 风险
- 极低：纯提取，`cargo build` + `cargo test`（441 测试）+ 手动冒烟验证
- bootstrap 内调用的函数（ensure_db / load_config 等）都是跨 crate pub，不受模块移动影响
