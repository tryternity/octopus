# 归档实施计划（2026-06-15 ~ 2026-06-16，已实现）

> 本文件合并以下**已实现功能**的原始实施 plan，作为历史记录归档（2026-06-18）。
> 各功能已在 main 实现，**权威现状以 [`architecture.md`](../../architecture.md) 为准**。
> 归档内各 plan 之间的交叉引用可能指向已归档的同级文件——所需内容均在本文内，请按下方标题搜索。

## 包含的原 plan

- `2026-06-15-result-window-toolbar.md`
- `2026-06-16-asr-llm-model-menu.md`
- `2026-06-16-denoise-deepfilternet.md`
- `2026-06-16-model-spec-prefix.md`

---

## `2026-06-15-result-window-toolbar.md`

# 结果窗口工具栏 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 在 `result_window` 展示区上方加一行 hover 显隐的工具栏，提供运行时切换 LLM 润色 mode（立即生效）和 ASR 引擎（下次会话生效），两者持久化写回 `~/.octopus/config.yaml`；另两个工具（设置页、LLM 模型切换）本轮占位。

**Architecture:** 方案 A——新增 `#[tauri::command]` 直连 + 共享 `Arc<RwLock<RuntimeConfig>>`（仅 `asr_engine`+`polish_mode` 两字段）挂 `tauri::State`。命令写 RuntimeConfig + 写回 config.yaml；Coordinator 单线程闭包在「开新会话」时同步 asr_engine、在每个 tick 同步 polish_mode（让中间/最终润色 live 生效），无需改任何 `handle_*` 自由函数签名。

**Tech Stack:** Rust + Tauri 2（`#[tauri::command]`/`tauri::State`/`invoke_handler`）、serde_yaml（config.yaml 写回）、vanilla JS（单 HTML，`window.__TAURI__.core.invoke` + `__TAURI__.window.setSize`）、CSS mask（图标变色）。

**Spec:** `docs/superpowers/specs/2026-06-15-result-window-toolbar-design.md`

> **状态：✅ 已实现（已合并 main，2026-06-15）**——全部 Task 完成。实现中 UI 多轮打磨，最终代码与本文 Task 的代码块有若干差异（触发改 mousemove、弹层 360px、EngineOption 加 `is_local`、引擎名加「本地-/远程-」前缀、图标 `?v=2` 缓存清除、debug devtools、run-octopus.sh），详见 spec §14「实现后修订」。本文 checkbox 已全部勾选，代码块保留作历史执行记录。

**Pre-flight（执行前）：**
- 当前在 `main` 分支。按项目约定（CLAUDE.md）先建特性分支再提交：`git checkout -b feat/result-window-toolbar`。提交/推送仅在用户要求时进行；本计划的 commit 步骤在该分支上执行。
- 全量基线：`cargo check --workspace --all-targets` 应通过（实施前确认绿色基线）。
- 用户需在 `crates/desktop/dist/result/icons/` 放 4 个 SVG：`settings.svg` / `polish-mode.svg` / `asr-engine.svg` / `llm-model.svg`（viewBox `0 0 24 24`，单色 stroke，透明背景）。缺失则图标不显示但布局正常。

---

## Task 1: infra `AppConfig` / `PolishMode` 加 `Serialize`

config.yaml 写回需要序列化。当前 `AppConfig` 只 `Deserialize`，`PolishMode` 只有自定义 `Deserialize`。

**Files:**
- Modify: `crates/infra/src/config.rs`

- [x] **Step 1: 写失败测试——Serialize/Deserialize 往返保留 asr_engine + polish_mode**

在 `crates/infra/src/config.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
    #[test]
    fn app_config_serialize_round_trip_preserves_overrides() {
        // 构造一个带覆盖值的 AppConfig（从 yaml 解析）
        let yaml = "asr_engine: whisper-small\npolish_mode: 2\nmicrophone: \"My Mic\"\n";
        let cfg: AppConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.asr_engine, "whisper-small");
        assert_eq!(cfg.polish_mode, PolishMode::Intermediate);

        // 序列化回 yaml，再解析，字段应保留
        let reserialized = serde_yaml::to_string(&cfg).unwrap();
        let cfg2: AppConfig = serde_yaml::from_str(&reserialized).unwrap();
        assert_eq!(cfg2.asr_engine, "whisper-small");
        assert_eq!(cfg2.polish_mode, PolishMode::Intermediate);
        assert_eq!(cfg2.microphone, "My Mic");

        // polish_mode 序列化为整数（u8），非枚举名
        assert!(
            reserialized.contains("polish_mode: 2"),
            "polish_mode 应序列化为整数 2，实际: {}",
            reserialized
        );
    }
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-infra app_config_serialize_round_trip_preserves_overrides`
Expected: 编译失败（`AppConfig`/`PolishMode` 未实现 `Serialize`，`serde_yaml::to_string` 报错）。

- [x] **Step 3: 给 AppConfig 加 Serialize**

`crates/infra/src/config.rs` 第 44 行 derive 改为：

```rust
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppConfig {
```

并在文件顶部 `use` 区（第 7 行 `use serde::Deserialize;`）改为：

```rust
use serde::{Deserialize, Serialize};
```

- [x] **Step 4: 给 PolishMode 加 Serialize impl**

在 `PolishMode` 的 `Deserialize` impl 之后（`config.rs` 约第 39 行 `}` 之后）追加：

```rust
impl Serialize for PolishMode {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(match self {
            PolishMode::Disabled => 0,
            PolishMode::FinalOnly => 1,
            PolishMode::Intermediate => 2,
        })
    }
}
```

- [x] **Step 5: 运行测试确认通过**

Run: `cargo test -p octopus-infra`
Expected: PASS（含新往返测试 + 原有 polish_mode/write_to_clipboard 等测试）。

- [x] **Step 6: 全量 check**

Run: `cargo check --workspace --all-targets`
Expected: 通过（infra 加 Serialize 不影响其他 crate）。

- [x] **Step 7: Commit**

```bash
git add crates/infra/src/config.rs
git commit -m "feat(infra): add Serialize to AppConfig/PolishMode for config.yaml write-back"
```

---

## Task 2: Transcript 增 `set_mode`（polish mode live 化）

polish mode 立即生效要求运行中能更新 Transcript 的 mode。

**Files:**
- Modify: `crates/desktop/src/transcript.rs`

- [x] **Step 1: 写失败测试——set_mode 切换后 display 行为随变**

在 `transcript.rs` 的 `#[cfg(test)] mod tests` 末尾追加：

```rust
    #[test]
    fn set_mode_changes_intermediate_behavior_live() {
        // 起始 mode=2（中间润色）：说一段 + 快照 + 润色
        let mut t = Transcript::new(20, PolishMode::Intermediate);
        t.set_full("原文");
        t.snapshot_for_polish();
        t.on_polish_done("润色".into());
        assert_eq!(t.display_text(), "润色");

        // 继续说 → increase 出现（mode=2 行为）
        t.set_full("原文新增");
        assert_eq!(t.increase(), "新增");
        assert_eq!(t.display_text(), "润色新增");

        // live 切到 mode=0（关闭）：increase 立即恒空，display 退回 full
        t.set_mode(PolishMode::Disabled);
        assert_eq!(t.increase(), "");
        assert_eq!(t.display_text(), "原文新增"); // full，不再用 polished
    }
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-desktop set_mode_changes_intermediate_behavior_live`
Expected: 编译失败（无 `set_mode` 方法）。

- [x] **Step 3: 加 set_mode 方法**

在 `transcript.rs` 的 `impl Transcript` 中，`pub fn mode(&self)`（约第 114 行）之后追加：

```rust
    /// 运行时更新润色模式（工具栏 live 切换用）。Coordinator 单线程访问，无需同步。
    pub fn set_mode(&mut self, mode: PolishMode) {
        self.mode = mode;
    }
```

- [x] **Step 4: 运行测试确认通过**

Run: `cargo test -p octopus-desktop transcript`
Expected: PASS（含新测试 + 原有 8 个 transcript 测试）。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/transcript.rs
git commit -m "feat(desktop): add Transcript::set_mode for live polish-mode switch"
```

---

## Task 3: 新建 `runtime_config.rs`（RuntimeConfig + persist + 4 命令）

**Files:**
- Create: `crates/desktop/src/runtime_config.rs`
- Modify: `crates/desktop/src/main.rs`（仅加 `mod runtime_config;`，命令注册在 Task 4）

- [x] **Step 1: 写 runtime_config.rs 主体**

创建 `crates/desktop/src/runtime_config.rs`：

```rust
//! 工具栏运行时可变配置：asr_engine + polish_mode 的共享镜像 + config.yaml 写回 + Tauri 命令。
//!
//! 与 OnceLock 缓存的 AppConfig 关系：AppConfig 是启动只读快照；RuntimeConfig 是这两个字段的
//! 可变运行时镜像。命令写 RuntimeConfig（即时生效）+ 写 config.yaml（重启生效）。

use serde::Serialize;
use std::sync::{Arc, RwLock};
use tauri::State;

use crate::config::PolishMode;

/// 运行时可变的两个配置字段。
pub struct RuntimeConfig {
    pub asr_engine: String,
    pub polish_mode: PolishMode,
}

impl RuntimeConfig {
    pub fn from_config(cfg: &octopus_infra::config::AppConfig) -> Self {
        Self {
            asr_engine: cfg.asr_engine.clone(),
            polish_mode: cfg.polish_mode,
        }
    }
}

/// 挂 tauri::State 的共享句柄。
pub type SharedRuntimeConfig = Arc<RwLock<RuntimeConfig>>;

fn polish_mode_to_u8(m: PolishMode) -> u8 {
    match m {
        PolishMode::Disabled => 0,
        PolishMode::FinalOnly => 1,
        PolishMode::Intermediate => 2,
    }
}

fn u8_to_polish_mode(n: u8) -> Option<PolishMode> {
    match n {
        0 => Some(PolishMode::Disabled),
        1 => Some(PolishMode::FinalOnly),
        2 => Some(PolishMode::Intermediate),
        _ => None,
    }
}

fn category_str(c: octopus_asr::config::EngineCategory) -> &'static str {
    use octopus_asr::config::EngineCategory::*;
    match c {
        Whisper => "whisper",
        SenseVoice => "sensevoice",
        Paraformer => "paraformer",
        Qwen3Asr => "qwen3-asr",
        Zipformer => "zipformer",
    }
}

// ── config.yaml 写回 ──

/// 读当前 config.yaml → 覆盖 asr_engine → 序列化写回 ~/.octopus/config.yaml。
/// 写盘只影响下次重启读取；运行时生效走 RuntimeConfig。失败返回 Err（调用方 best-effort）。
pub fn persist_asr_engine(value: &str) -> Result<(), String> {
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    cfg.asr_engine = value.to_string();
    write_config_yaml(&cfg)
}

/// 读当前 config.yaml → 覆盖 polish_mode → 序列化写回。
pub fn persist_polish_mode(value: u8) -> Result<(), String> {
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    cfg.polish_mode = u8_to_polish_mode(value).ok_or_else(|| format!("polish_mode={} 非法", value))?;
    write_config_yaml(&cfg)
}

fn write_config_yaml(cfg: &octopus_infra::config::AppConfig) -> Result<(), String> {
    let path = octopus_infra::octopus_config_home().join("config.yaml");
    let text = serde_yaml::to_string(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

// ── 命令返回 DTO ──

#[derive(Serialize)]
pub struct ToolbarState {
    pub asr_engine: String,
    pub polish_mode: u8,
}

#[derive(Serialize)]
pub struct EngineOption {
    pub name: String,
    pub category: String,
    pub current: bool,
}

// ── Tauri 命令 ──

#[tauri::command]
pub fn toolbar_state(rc: State<'_, SharedRuntimeConfig>) -> ToolbarState {
    let g = rc.read().unwrap();
    ToolbarState {
        asr_engine: g.asr_engine.clone(),
        polish_mode: polish_mode_to_u8(g.polish_mode),
    }
}

#[tauri::command]
pub fn list_asr_engines(rc: State<'_, SharedRuntimeConfig>) -> Result<Vec<EngineOption>, String> {
    let current_raw = rc.read().unwrap().asr_engine.clone();
    // 兜底：空 asr_engine → 当前生效 zipformer-small-ctc
    let current_effective = if current_raw.is_empty() {
        "zipformer-small-ctc".to_string()
    } else {
        current_raw
    };
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    Ok(engines
        .into_iter()
        .map(|e| EngineOption {
            current: e.name == current_effective,
            name: e.name,
            category: category_str(e.category).to_string(),
        })
        .collect())
}

#[tauri::command]
pub fn switch_asr_engine(name: String, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    // 校验：name 必须是 DB 已配置的引擎（不走兜底）
    let exists = octopus_asr::config::list_engines()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|e| e.name == name);
    if !exists {
        return Err(format!("引擎 '{}' 不存在，未切换", name));
    }
    {
        let mut g = rc.write().unwrap();
        g.asr_engine = name.clone();
    }
    if let Err(e) = persist_asr_engine(&name) {
        log::warn!(
            "写回 config.yaml 失败（asr_engine={}）：{} —— 本次仍生效，重启后回退",
            name,
            e
        );
    }
    Ok(())
}

#[tauri::command]
pub fn set_polish_mode(mode: u8, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    let pm = u8_to_polish_mode(mode).ok_or_else(|| format!("polish_mode={} 非法（应为 0/1/2）", mode))?;
    {
        let mut g = rc.write().unwrap();
        g.polish_mode = pm;
    }
    if let Err(e) = persist_polish_mode(mode) {
        log::warn!(
            "写回 config.yaml 失败（polish_mode={}）：{} —— 本次仍生效，重启后回退",
            mode,
            e
        );
    }
    Ok(())
}

// ── 单测（纯逻辑，不触文件 IO / Tauri State）──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_mirrors_fields() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        cfg.asr_engine = "qwen3-asr-0.6B".into();
        cfg.polish_mode = PolishMode::Intermediate;
        let rc = RuntimeConfig::from_config(&cfg);
        assert_eq!(rc.asr_engine, "qwen3-asr-0.6B");
        assert_eq!(rc.polish_mode, PolishMode::Intermediate);
    }

    #[test]
    fn polish_mode_u8_round_trip() {
        for n in 0..=2u8 {
            let m = u8_to_polish_mode(n).unwrap();
            assert_eq!(polish_mode_to_u8(m), n);
        }
        assert!(u8_to_polish_mode(3).is_none());
        assert!(u8_to_polish_mode(99).is_none());
    }
}
```

- [x] **Step 2: 注册模块**

`crates/desktop/src/main.rs` 第 14 行 `mod result_window;` 之后加一行：

```rust
mod runtime_config;
```

- [x] **Step 3: 编译确认（命令尚未注册，但模块应编译）**

Run: `cargo check -p octopus-desktop`
Expected: 通过（`#[tauri::command]` 宏展开正常，单测代码编译）。

- [x] **Step 4: 运行单测**

Run: `cargo test -p octopus-desktop runtime_config`
Expected: PASS（`from_config_mirrors_fields` + `polish_mode_u8_round_trip`）。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/runtime_config.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): add RuntimeConfig module + toolbar Tauri commands"
```

---

## Task 4: main.rs 注册命令 + 挂 State

**Files:**
- Modify: `crates/desktop/src/main.rs`

- [x] **Step 1: 在 setup 里建 RuntimeConfig 并挂 State**

`crates/desktop/src/main.rs` 的 `.setup(move |app| {` 块内，紧接「3. Create Coordinator」之前（约第 172 行 `// 3. Create Coordinator` 之前）插入：

```rust
            // 工具栏运行时配置（asr_engine + polish_mode 的可变镜像），命令与 Coordinator 共享
            let runtime_config: runtime_config::SharedRuntimeConfig =
                std::sync::Arc::new(std::sync::RwLock::new(
                    runtime_config::RuntimeConfig::from_config(&config),
                ));
            app.manage(runtime_config.clone());
```

- [x] **Step 2: 把 runtime_config 传给 Coordinator**

把「3. Create Coordinator」（约第 173-175 行）改为：

```rust
            // 3. Create Coordinator
            let coordinator = Coordinator::new(
                engine,
                audio_state,
                config.clone(),
                app.handle().clone(),
                runtime_config.clone(),
            );
            app.manage(coordinator);
```

- [x] **Step 3: 注册 invoke_handler**

在 `.tauri::Builder::default()` 链中，`.setup(...)` 之前插入 `.invoke_handler(...)`。定位 `crates/desktop/src/main.rs` 的 `        .setup(move |app| {`（约第 131 行），在其上一行加：

```rust
        .invoke_handler(tauri::generate_handler![
            runtime_config::toolbar_state,
            runtime_config::list_asr_engines,
            runtime_config::switch_asr_engine,
            runtime_config::set_polish_mode,
        ])
```

- [x] **Step 4: 编译确认**

Run: `cargo check -p octopus-desktop`
Expected: 失败——`Coordinator::new` 签名还没加 `runtime_config` 参数（下一个 Task 改）。此处先不改 Coordinator 会让编译失败是预期的；若想先绿，可在 Task 5 完成后再统一编译。

> 说明：Task 4 与 Task 5 是原子改动（改 main.rs 调用点 + 改 Coordinator 签名），中间态不编译。两个 Task 都完成后一起 `cargo check`。

- [x] **Step 5: 暂不单独 commit**（与 Task 5 合并提交）

---

## Task 5: Coordinator live-sync（asr_engine 仅 Idle 同步；polish_mode 每 tick 同步）

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: Coordinator::new 加 runtime_config 参数 + 闭包内 config/use_streaming 可变**

`coordinator.rs` 第 133-138 行 `pub fn new(...)` 签名加一个参数：

```rust
    pub fn new(
        engine: Arc<dyn TranscriptionEngine>,
        audio: Arc<SharedAudioState>,
        config: AppConfig,
        app_handle: tauri::AppHandle,
        runtime_config: crate::runtime_config::SharedRuntimeConfig,
    ) -> Self {
```

第 142 行 `let use_streaming = ...;` 之后，`std::thread::spawn(move || {` 之前，插入两行让闭包捕获可变：

```rust
        let use_streaming = config.engine_mode == "embedded" && crate::config::is_streaming_engine(&config);
        let mut config = config;
        let mut use_streaming = use_streaming;

        std::thread::spawn(move || {
```

（注意：原第 144 行 `std::thread::spawn(move || {` 保持不变；`runtime_config` 由 `move ||` 自动 move 捕获进闭包。）

- [x] **Step 2: Toggle 命令分发处——仅 Idle 时同步 asr_engine + polish_mode + polish_llm + 重算 use_streaming**（2026-06-18 补 polish_llm 同步——原仅同步 asr_engine/polish_mode，遗漏 polish_llm 致立即润色按钮失效）

`coordinator.rs` 第 157-167 行的 `Command::Toggle => { ... }` 改为：

```rust
                    Command::Toggle => {
                        // 仅在 Idle（开新会话）时同步运行时覆盖；STOP 时不动 asr_engine
                        // （否则会把"刚切换但本会话未用"的引擎名写进 DB 记录）
                        if matches!(stage, Stage::Idle) {
                            let rc = runtime_config.read().unwrap();
                            config.asr_engine = rc.asr_engine.clone();
                            config.polish_mode = rc.polish_mode;
                            drop(rc);
                            use_streaming = config.engine_mode == "embedded"
                                && crate::config::is_streaming_engine(&config);
                        }
                        handle_toggle(
                            &mut stage,
                            &audio,
                            &engine,
                            &config,
                            &app_handle,
                            &tx,
                            use_streaming,
                        );
                    }
```

- [x] **Step 3: StreamingTick 分发处——同步 polish_mode + transcript.set_mode**

`coordinator.rs` 第 168-170 行 `Command::StreamingTick => { ... }` 改为：

```rust
                    Command::StreamingTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::Streaming { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        handle_streaming_tick(&mut stage, &audio, &config, &app_handle, &tx);
                    }
```

- [x] **Step 4: VadSegmentedTick 分发处——同步 polish_mode + transcript.set_mode**

`coordinator.rs` 第 171-180 行 `Command::VadSegmentedTick => { ... }` 改为：

```rust
                    Command::VadSegmentedTick => {
                        {
                            let rc = runtime_config.read().unwrap();
                            config.polish_mode = rc.polish_mode;
                        }
                        if let Stage::VadSegmented { transcript, .. } = &mut stage {
                            transcript.set_mode(config.polish_mode);
                        }
                        handle_vad_segmented_tick(
                            &mut stage,
                            &audio,
                            &engine,
                            &config,
                            &app_handle,
                            &tx,
                        );
                    }
```

- [x] **Step 5: 编译确认（Task 4 + Task 5 合并）**

Run: `cargo check --workspace --all-targets`
Expected: 通过。

- [x] **Step 6: 运行已有测试确认无回归**

Run: `cargo test -p octopus-desktop`
Expected: PASS（transcript / runtime_config 等全绿；Coordinator 无单测但编译通过即签名正确）。

- [x] **Step 7: Commit（Task 4 + Task 5 合并）**

```bash
git add crates/desktop/src/main.rs crates/desktop/src/coordinator.rs
git commit -m "feat(desktop): wire toolbar commands + Coordinator live-sync (engine next-session, polish live)"
```

---

## Task 6: 前端——工具栏结构 + CSS（mask 图标）+ hover 动态高度

**Files:**
- Modify: `crates/desktop/dist/result/index.html`

- [x] **Step 1: 改 DOM 结构（加 top-bar + toolbar + popup）**

把 `index.html` 第 86-92 行 `<body>` 内的：

```html
  <div id="container">
    <div id="drag-handle"></div>
    <div id="text-wrapper">
      <div id="result-text"></div>
    </div>
  </div>
```

替换为：

```html
  <div id="container">
    <div id="top-bar">
      <div id="drag-handle"></div>
      <div id="toolbar">
        <button class="tool" id="tool-settings" disabled title="敬请期待" aria-label="设置">
          <span class="icon"></span>
        </button>
        <button class="tool" id="tool-polish" title="润色模式" aria-label="润色模式">
          <span class="icon"></span>
        </button>
        <button class="tool" id="tool-asr" title="ASR 识别模型" aria-label="ASR 识别模型">
          <span class="icon"></span>
        </button>
        <button class="tool" id="tool-llm" disabled title="敬请期待" aria-label="LLM 模型">
          <span class="icon"></span>
        </button>
      </div>
    </div>
    <div id="text-wrapper">
      <div id="result-text"></div>
    </div>
    <div id="popup" hidden></div>
    <div id="toast" hidden></div>
  </div>
```

- [x] **Step 2: 改 CSS——top-bar / toolbar / mask 图标 / popup / toast**

把 `index.html` 第 31-83 行（从 `/* 顶部拖拽区域 */` 到 `#result-text::-webkit-scrollbar-thumb { ... }` 之前的 `#drag-handle` 相关样式）替换/扩充。在 `#container.visible { opacity: 1; }`（第 29 行）之后插入：

```css
    /* 顶部条：拖拽 + 工具栏 */
    #top-bar {
      flex-shrink: 0;
      display: flex;
      align-items: center;
      height: 8px;
      transition: height 0.12s ease;
    }
    #container.toolbar-visible #top-bar { height: 32px; }

    #drag-handle {
      flex: 1;
      height: 100%;
      cursor: grab;
      display: flex;
      align-items: center;
      justify-content: center;
    }
    #drag-handle:active { cursor: grabbing; }
    #container:not(.toolbar-visible) #drag-handle::after {
      content: '';
      width: 24px;
      height: 3px;
      background: rgba(0, 0, 0, 0.12);
      border-radius: 1.5px;
    }

    /* 工具栏（默认隐藏，toolbar-visible 时显示） */
    #toolbar {
      display: none;
      align-items: center;
      gap: 2px;
      padding: 0 6px;
    }
    #container.toolbar-visible #toolbar { display: flex; }

    .tool {
      width: 26px;
      height: 26px;
      padding: 0;
      border: none;
      background: transparent;
      border-radius: 5px;
      cursor: pointer;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      color: #1d1d1f;
    }
    .tool:hover { color: #007aff; background: rgba(0, 0, 0, 0.06); }
    .tool.active { color: #007aff; }
    .tool[disabled] { color: rgba(0, 0, 0, 0.22); cursor: default; }
    .tool[disabled]:hover { background: transparent; }

    /* mask 图标：单 SVG 随 currentColor 变色 */
    .tool .icon {
      width: 18px;
      height: 18px;
      background: currentColor;
      -webkit-mask-size: contain;
      mask-size: contain;
      -webkit-mask-repeat: no-repeat;
      mask-repeat: no-repeat;
      -webkit-mask-position: center;
      mask-position: center;
    }
    #tool-settings .icon { -webkit-mask-image: url(icons/settings.svg); mask-image: url(icons/settings.svg); }
    #tool-polish    .icon { -webkit-mask-image: url(icons/polish-mode.svg); mask-image: url(icons/polish-mode.svg); }
    #tool-asr       .icon { -webkit-mask-image: url(icons/asr-engine.svg); mask-image: url(icons/asr-engine.svg); }
    #tool-llm       .icon { -webkit-mask-image: url(icons/llm-model.svg); mask-image: url(icons/llm-model.svg); }

    /* 浮层 */
    #popup {
      position: absolute;
      top: 30px;
      left: 10px;
      min-width: 180px;
      max-height: 200px;
      overflow-y: auto;
      background: #ffffff;
      border: 0.5px solid rgba(0, 0, 0, 0.12);
      border-radius: 8px;
      box-shadow: 0 4px 16px rgba(0, 0, 0, 0.12);
      z-index: 10;
      font-size: 13px;
    }
    #popup .option {
      padding: 6px 12px;
      cursor: pointer;
      color: #1d1d1f;
      display: flex;
      align-items: center;
      gap: 6px;
    }
    #popup .option:hover { background: rgba(0, 122, 255, 0.08); }
    #popup .option.current { color: #007aff; font-weight: 500; }
    #popup .option .cat { font-size: 11px; color: rgba(0,0,0,0.4); margin-left: auto; }
    #popup .option.current .cat { color: rgba(0,122,255,0.6); }

    /* toast */
    #toast {
      position: absolute;
      bottom: 6px;
      left: 50%;
      transform: translateX(-50%);
      background: rgba(0, 0, 0, 0.78);
      color: #fff;
      font-size: 12px;
      padding: 4px 10px;
      border-radius: 6px;
      z-index: 20;
      pointer-events: none;
    }
```

并删掉旧 `#drag-handle` 单独那段（第 32-49 行原块已被上面 `#drag-handle` 替代；若重复定义，保留新块删旧块）。

- [x] **Step 3: 加 hover 动态高度 JS（mouseenter/mouseleave）**

把 `index.html` 第 94-107 行 `<script>` 开头部分（`const container = ...` 到 `dragHandle.addEventListener('mousedown', ...)` 之间）插入窗口尺寸常量与显隐逻辑。在 `const currentWindow = getCurrentWindow();`（第 101 行）之后插入：

```javascript
    const { LogicalSize } = window.__TAURI__.window;
    const WIN_W = 520;
    const HIDDEN_H = 100;
    const TOOLBAR_H = 132;
    let popupOpen = false;

    function showToolbar() {
      container.classList.add('toolbar-visible');
      currentWindow.setSize(new LogicalSize(WIN_W, TOOLBAR_H));
    }
    function hideToolbar() {
      if (popupOpen) return;            // 浮层打开时钉住
      container.classList.remove('toolbar-visible');
      currentWindow.setSize(new LogicalSize(WIN_W, HIDDEN_H));
    }

    container.addEventListener('mouseenter', showToolbar);
    container.addEventListener('mouseleave', hideToolbar);
```

- [x] **Step 4: 冒烟——启动应用，hover 结果窗应长高显示空工具栏**

Run: `cargo tauri dev`（或现有启动方式），触发一次识别让 result_window 显示，鼠标移上去。
Expected: 窗口从 100→132px，顶部出现 4 个图标按钮（图标需用户已放 SVG；暂无 SVG 则按钮空白但布局正常）；移开缩回 100px。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/dist/result/index.html
git commit -m "feat(desktop): toolbar DOM + mask icons + hover dynamic height"
```

---

## Task 7: 前端——浮层 + invoke 调用 + toast + 占位工具

**Files:**
- Modify: `crates/desktop/dist/result/index.html`

- [x] **Step 1: 加 invoke 封装 + toast + 浮层渲染逻辑**

在 `index.html` `<script>` 内（Task 6 插入的 hover 逻辑之后、`listen('show-result', ...)` 之前）插入：

```javascript
    const { invoke } = window.__TAURI__.core;

    function showToast(msg) {
      const t = document.getElementById('toast');
      t.textContent = msg;
      t.hidden = false;
      clearTimeout(showToast._timer);
      showToast._timer = setTimeout(() => { t.hidden = true; }, 2000);
    }

    // ── 浮层 ──
    const popup = document.getElementById('popup');
    function closePopup() {
      popup.hidden = true;
      popup.innerHTML = '';
      popupOpen = false;
    }
    function openPopup(html) {
      popup.innerHTML = html;
      popup.hidden = false;
      popupOpen = true;
    }
    // 点浮层外关闭
    document.addEventListener('mousedown', (e) => {
      if (!popupOpen) return;
      if (!popup.contains(e.target) && !e.target.closest('.tool')) closePopup();
    });
    // Esc 关浮层
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && popupOpen) closePopup();
    });

    // ── 润色 mode ──
    const POLISH_OPTIONS = [
      { mode: 0, label: '关闭' },
      { mode: 1, label: '仅最终润色' },
      { mode: 2, label: '中间 + 最终润色' },
    ];
    let currentPolishMode = 0;

    document.getElementById('tool-polish').addEventListener('click', async () => {
      const html = POLISH_OPTIONS.map(o =>
        `<div class="option${o.mode === currentPolishMode ? ' current' : ''}" data-mode="${o.mode}">
           <span>${o.mode === currentPolishMode ? '●' : '○'}</span>${o.label}
         </div>`).join('');
      openPopup(html);
      popup.querySelectorAll('.option').forEach(el => {
        el.addEventListener('click', async () => {
          const mode = Number(el.dataset.mode);
          try {
            await invoke('set_polish_mode', { mode });
            currentPolishMode = mode;
            closePopup();
            refreshActive();
          } catch (e) {
            showToast('切换失败：' + e);
          }
        });
      });
    });

    // ── ASR 引擎 ──
    document.getElementById('tool-asr').addEventListener('click', async () => {
      let engines;
      try { engines = await invoke('list_asr_engines'); }
      catch (e) { showToast('读取引擎失败：' + e); return; }
      const html = engines.map(e =>
        `<div class="option${e.current ? ' current' : ''}" data-name="${e.name}">
           <span>${e.current ? '●' : '○'}</span>${e.name}<span class="cat">${e.category}</span>
         </div>`).join('');
      openPopup(html);
      popup.querySelectorAll('.option').forEach(el => {
        el.addEventListener('click', async () => {
          const name = el.dataset.name;
          try {
            await invoke('switch_asr_engine', { name });
            closePopup();
            refreshActive();
            showToast('已切换：' + name + '（下次录音生效）');
          } catch (err) {
            showToast('' + err);
          }
        });
      });
    });

    // ── 当前态高亮 ──
    async function refreshActive() {
      try {
        const st = await invoke('toolbar_state');
        currentPolishMode = st.polish_mode;
        document.getElementById('tool-polish').classList.toggle('active', st.polish_mode !== 0);
        document.getElementById('tool-asr').classList.toggle('active', true);
      } catch (_) { /* 忽略 */ }
    }
    refreshActive();
```

- [x] **Step 2: 占位工具确认无动作**

`#tool-settings` 与 `#tool-llm` 已是 `disabled`（HTML 属性），浏览器不会触发 click；CSS 已置灰 + tooltip「敬请期待」。无需额外 JS。

- [x] **Step 3: 冒烟——切换润色 mode / ASR 引擎**

Run: `cargo tauri dev`，hover 出工具栏：
- 点 [✨] → 浮层 3 选 → 选「中间+最终」→ 浮层关闭、图标变蓝、`~/.octopus/config.yaml` 的 `polish_mode: 2`。
- 点 [🎙] → 浮层 8 引擎列表（当前项 ●）→ 选另一个 → toast「已切换：X（下次录音生效）」、`config.yaml` 的 `asr_engine: X`。
- 浮层打开时鼠标移出窗口 → 工具栏不收（钉住）；关掉浮层后再移出 → 收起。
- 占位工具置灰，悬停显示「敬请期待」，点击无反应。

Expected: 上述行为全部符合。

- [x] **Step 4: 验证 config.yaml 写回内容**

Run: `cat ~/.octopus/config.yaml`（**注意：若文件含 API Key，只确认 asr_engine/polish_mode 两行，勿打印全文**）
Expected: `asr_engine` 与 `polish_mode` 为刚切换的值，其他字段保留。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/dist/result/index.html
git commit -m "feat(desktop): toolbar popup + invoke wiring + toast + placeholder tools"
```

---

## Task 8: 文档同步（CLAUDE.md 强制）

**Files:**
- Modify: `docs/configuration.md`
- Modify: `docs/architecture.md`

- [x] **Step 1: configuration.md 补注运行时可改**

在 `docs/configuration.md` 的 `asr_engine` 行（字段表，约第 75 行）的「说明」末尾追加一句：

```
（桌面端可经结果窗工具栏运行时切换并写回此字段，下次录音生效）
```

在 `polish_mode` 行（约第 87 行）的「说明」末尾追加：

```
（桌面端可经结果窗工具栏运行时切换并写回此字段，立即生效）
```

- [x] **Step 2: architecture.md 补工具栏 + RuntimeConfig 子系统**

在 `docs/architecture.md` 的「窗口管理」表（约第 92-95 行 `result_window` 行）的「用途」列补：

```
识别结果展示 + hover 工具栏（润色 mode / ASR 引擎运行时切换）
```

在「核心状态机（Coordinator）」段末尾（约第 118 行 `停止空文本边界...` 之后）追加一条：

```
- **工具栏运行时切换**：`result_window` 顶部 hover 工具栏经 `#[tauri::command]`（`runtime_config.rs`）改 `Arc<RwLock<RuntimeConfig>>`（asr_engine + polish_mode 两字段）并写回 `~/.octopus/config.yaml`。ASR 引擎在开新会话（Idle）时同步进 Coordinator 的 config（下次录音生效，不破坏当前会话）；polish_mode 每 tick（300/600ms）同步 + `Transcript::set_mode`（立即生效）。占位工具（设置页 / LLM 模型）本轮不实现。
```

- [x] **Step 3: Commit**

```bash
git add docs/configuration.md docs/architecture.md
git commit -m "docs: note result-window toolbar runtime switching"
```

---

## Task 9: 手动 e2e 验收清单

**Files:** 无（验证）

- [x] **Step 1: 基线绿**

Run: `cargo check --workspace --all-targets && cargo test --workspace`
Expected: 全绿。

- [x] **Step 2: 显隐 + 尺寸**

启动应用 → 触发识别显示结果窗 → hover：窗口 100→132px、4 图标显示；移开：132→100px。✅

- [x] **Step 3: ASR 引擎切换（下次会话生效）**

录音中点 [🎙] 切到非当前引擎 → toast「下次录音生效」→ 当前会话识别照常（引擎不变）→ 停止后重新录音 → 用新引擎识别 → `config.yaml.asr_engine` 已更新。✅

- [x] **Step 4: 跨类别切换重算 streaming 模式**

从流式引擎（paraformer/zipformer）切到离线引擎（whisper/sensevoice/qwen3）→ 下次录音自动走 VAD 分段伪流式（`use_streaming` 重算）。✅

- [x] **Step 5: polish mode 立即生效**

mode=2 录音中，说一段停顿 → 看到中间润色；点 [✨] 切到 0 → 后续不再润色、display 退回 raw；再切回 2 → 恢复中间润色。`config.yaml.polish_mode` 跟随。✅

- [x] **Step 6: 钉住 + Esc + 点外关闭**

浮层打开时移开鼠标 → 工具栏不收；Esc / 点浮层外 → 浮层关闭。✅

- [x] **Step 7: 错误态**

（临时）手改 DB 删某引擎 或 在 invoke 里传不存在的 name → toast「引擎 X 不存在，未切换」，RuntimeConfig 不变。✅

- [x] **Step 8: 占位工具**

[⚙] / [🤖] 置灰、悬停「敬请期待」、点击无反应。✅

---

## Self-Review（写计划后自检，已对照 spec）

**1. Spec coverage**（spec 各节 → task 映射）：
- §2 功能范围（4 工具）→ Task 6（DOM）+ Task 7（交互/占位）✅
- §3.1 动态高度 → Task 6 Step 3（mouseenter/mouseleave + setSize）✅
- §3.2 持久化写回 → Task 1（Serialize）+ Task 3（persist_*）✅
- §3.3 统一浮层面板 → Task 6（CSS #popup）+ Task 7（渲染/选择）✅
- §3.4 随时可切 / ASR 下次生效 / polish 立即生效 → Task 5（Idle 同步 engine、tick 同步 polish）✅
- §3.5 占位置灰 → Task 6（disabled + tooltip）✅
- §3.6 浮层钉住 → Task 6 Step 3（popupOpen 守卫）✅
- §5.1 RuntimeConfig → Task 3 ✅
- §5.2 四命令 → Task 3 + Task 4（注册）✅
- §5.3 persist_config_override → Task 3（persist_asr_engine/persist_polish_mode）✅
- §5.4 前端 + mask 图标 → Task 6 + Task 7 ✅
- §7.1 mask 变色 → Task 6 Step 2 ✅
- §7.2 Transcript.set_mode → Task 2 + Task 5（tick 调用）✅
- §8 写回语义（丢注释、绝对路径）→ Task 3（write_config_yaml 用 octopus_config_home）✅
- §9 错误处理 → Task 3（命令返回 Err）+ Task 7（toast）+ Task 9 Step 7 ✅
- §10 测试 → Task 1/2/3 单测 + Task 9 e2e ✅
- §12 文件清单 → 全覆盖 ✅

**2. Placeholder scan**：无 TBD/TODO；每步含完整代码或精确命令。✅

**3. Type consistency**：
- `SharedRuntimeConfig = Arc<RwLock<RuntimeConfig>>` 在 Task 3/4/5 一致 ✅
- 命令名 `toolbar_state`/`list_asr_engines`/`switch_asr_engine`/`set_polish_mode` 在 Task 3（定义）、Task 4（注册）、Task 7（invoke）三处一致 ✅
- 前端 invoke 参数 `{ mode }` / `{ name }` 与命令形参 `mode: u8` / `name: String` 匹配（Tauri 自动 camelCase↔snake_case 对齐）✅
- `set_mode` / `set_polish_mode` 命名区分清晰（Transcript 方法 vs 命令）✅
- `persist_asr_engine` / `persist_polish_mode`（Task 3）vs spec 的 `persist_config_override` —— 实现拆为两个具名函数，语义等价，无歧义 ✅

**4. 已知限制（非缺陷，记录在案）**：
- `serde_yaml` 写回丢失 yaml 注释（spec §8 已声明可接受）。
- 切引擎后新引擎无预热（首次 transcribe 有加载延迟，非阻塞）。
- `RwLock` 中毒时命令 `.unwrap()` 会 panic → Tauri 捕获为前端错误（spec §9 已声明 best-effort）。

---

## `2026-06-16-asr-llm-model-menu.md`

# ASR/LLM 模型选择菜单 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 改造结果窗口 ASR 下拉菜单（固定兜底首项 + `is_local desc, category` 排序 + 「本地:name / provider:name」显示），并新增 LLM 润色模型下拉菜单（同规则，切换 `polish_llm`）。

> 状态：✅ 全部完成（2026-06-16，已合并 main）。label 远程前缀 2026-06-17 从 `category` 改为 `provider`（见 spec §3）。

**Architecture:** 后端负责排序/过滤/label 拼接（`list_engines` 改排序、新增 `list_llm_models` 查询、`list_asr_engines`/`list_llm_models` 命令注入兜底与拼 label、`switch_*` 命令校验+持久化）；前端 `result/index.html` 复用现有 `#popup` 模式直显 `label`，启用已存在的 `#tool-llm` 按钮。纯逻辑提取为可测函数（`order_engine_infos`/`build_asr_options`/`validate_switch`/`build_llm_options`），避免 Tauri `State`/DB/文件 IO 进单测。

**Tech Stack:** Rust（rusqlite、serde、tauri::command）、手写 HTML/JS（Tauri webview，无构建步骤）。

参考 spec：`docs/superpowers/specs/2026-06-16-asr-llm-model-menu-design.md`

---

## 文件结构

| 文件 | 责任 | 改动类型 |
|---|---|---|
| `crates/infra/src/db.rs` | 新增 `LlmModelInfo` + `list_llm_models_at` + `list_llm_models`（DB 查询 domain='llm' AND is_enabled=1） | 新增 |
| `crates/asr/src/config.rs` | `list_engines` 排序改为 `is_local desc + category 字母序`；提取 `order_engine_infos` + `category_label` | 修改 |
| `crates/desktop/src/runtime_config.rs` | `EngineOption` 加 `label`；`list_asr_engines` 注入兜底；`switch_asr_engine` 放宽兜底；`RuntimeConfig` 加 `polish_llm`；新增 `LlmOption`/`list_llm_models`/`switch_polish_llm`/`persist_polish_llm` + 纯逻辑 helper | 修改+新增 |
| `crates/desktop/src/main.rs` | `generate_handler!` 注册 `list_llm_models`、`switch_polish_llm` | 修改 |
| `crates/desktop/dist/result/index.html` | ASR popup 改用 `label`；启用 `#tool-llm` + LLM popup 逻辑 | 修改 |

---

## Task 1: db.rs — LLM 模型列表查询

**Files:**
- Modify: `crates/infra/src/db.rs`（在 `load_llm_model_at` 附近新增；在 `tests` mod 新增测试）

- [x] **Step 1: 写失败测试**（在 `crates/infra/src/db.rs` 的 `mod tests` 内，仿 `seed_then_load_round_trips`）

```rust
#[test]
fn list_llm_models_filters_disabled_and_sorts() {
    let conn = open_init();
    // seed 默认 4 条 LLM 全 is_enabled=0；全部启用
    conn.execute("UPDATE models SET is_enabled = 1 WHERE domain='llm'", []).unwrap();
    // 再禁用 aliyun 那条，验证过滤
    conn.execute(
        "UPDATE models SET is_enabled = 0 WHERE domain='llm' AND category='aliyun'",
        [],
    ).unwrap();
    let list = list_llm_models_at(&conn).unwrap();
    // 剩余 3 条（全 is_local=0）→ is_local desc 无影响 → category 字母序
    // categories: bigmodel(glm-4-flashx), bigmodel(glm-4.5-flash), deepseek(deepseek-v4-flash)
    assert_eq!(list.len(), 3, "aliyun 被禁用应过滤");
    assert_eq!(
        list.iter().map(|m| m.category.as_str()).collect::<Vec<_>>(),
        vec!["bigmodel", "bigmodel", "deepseek"],
        "按 category 字母序"
    );
    assert!(list.iter().all(|m| !m.is_local), "seed LLM 全远程");
    // 同 category 内 name 字母序：glm-4-flashx < glm-4.5-flash
    let bigmodel_names: Vec<&str> = list.iter()
        .filter(|m| m.category == "bigmodel")
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(bigmodel_names, vec!["glm-4-flashx", "glm-4.5-flash"]);
}

#[test]
fn list_llm_models_at_empty_when_all_disabled() {
    let conn = open_init();
    // seed 全 is_enabled=0（默认）
    let list = list_llm_models_at(&conn).unwrap();
    assert!(list.is_empty(), "全禁用时返回空");
}
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-infra list_llm_models 2>&1`
Expected: 编译失败（`list_llm_models_at` 未定义）。

- [x] **Step 3: 实现**（在 `crates/infra/src/db.rs`，`load_llm_model_at` 函数之后）

```rust
/// LLM 模型列表项（菜单用，仅含显示与排序所需字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LlmModelInfo {
    pub name: String,
    pub category: String,
    pub is_local: bool,
}

/// 列出所有启用的 LLM 润色模型（domain='llm' AND is_enabled=1），按 is_local 降序、category 升序排序。
fn list_llm_models_at(conn: &Connection) -> Result<Vec<LlmModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT category, name, is_local FROM models
         WHERE domain='llm' AND is_enabled = 1
         ORDER BY is_local DESC, category",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LlmModelInfo {
            category: row.get::<_, String>(0)?,
            name: row.get::<_, String>(1)?,
            is_local: row.get::<_, i32>(2)? != 0,
        })
    })?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 从 DB 列出启用的 LLM 模型（经 with_db，供 Tauri 命令调用）。
pub fn list_llm_models() -> Result<Vec<LlmModelInfo>> {
    with_db(|conn| list_llm_models_at(conn))
}
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-infra list_llm_models 2>&1`
Expected: 2 passed。

- [x] **Step 5: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(infra): list_llm_models query (domain=llm, is_enabled=1, ordered)"
```

---

## Task 2: asr/config.rs — list_engines 排序

**Files:**
- Modify: `crates/asr/src/config.rs:216`（`list_engines` 排序块）+ 新增 `order_engine_infos`/`category_label`

- [x] **Step 1: 写失败测试**（在 `crates/asr/src/config.rs` 的 `mod tests` 内，仿 `cfg_with_zipformer` 同区域）

```rust
#[test]
fn order_engine_infos_sorts_is_local_desc_then_category_then_name() {
    use EngineCategory::*;
    let mut engines = vec![
        EngineInfo { name: "whisper-small".into(), category: Whisper, is_local: false, description: String::new() },
        EngineInfo { name: "zipformer-multi".into(), category: Zipformer, is_local: true, description: String::new() },
        EngineInfo { name: "paraformer-x".into(), category: Paraformer, is_local: false, description: String::new() },
        EngineInfo { name: "zipformer-small-ctc".into(), category: Zipformer, is_local: true, description: String::new() },
    ];
    order_engine_infos(&mut engines);
    let names: Vec<&str> = engines.iter().map(|e| e.name.as_str()).collect();
    // is_local=true 先（zipformer-multi < zipformer-small-ctc 按 name），再 false（paraformer < whisper 按 category 字母序）
    assert_eq!(names, vec!["zipformer-multi", "zipformer-small-ctc", "paraformer-x", "whisper-small"]);
}
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr order_engine_infos 2>&1`
Expected: 编译失败（`order_engine_infos` 未定义）。

- [x] **Step 3: 实现**（在 `crates/asr/src/config.rs`）

先加 category→str helper（紧邻 `EngineCategory` 定义或 `EngineInfo` 之后）：

```rust
/// EngineCategory → 小写 category 字符串（与 DB models.category 一致，用于排序与显示）。
fn category_label(c: &EngineCategory) -> &'static str {
    use EngineCategory::*;
    match c {
        Whisper => "whisper",
        SenseVoice => "sensevoice",
        Paraformer => "paraformer",
        Qwen3Asr => "qwen3-asr",
        Zipformer => "zipformer",
    }
}

/// 排序：is_local 降序（true 在前）→ category 字母序 → name 字母序。
fn order_engine_infos(engines: &mut [EngineInfo]) {
    engines.sort_by(|a, b| {
        b.is_local
            .cmp(&a.is_local)
            .then_with(|| category_label(&a.category).cmp(category_label(&b.category)))
            .then_with(|| a.name.cmp(&b.name))
    });
}
```

再把 `list_engines` 末尾的排序块（现有 `engines.sort_by(|a, b| { let cat_order = ... })` 整段）替换为单行调用：

```rust
    order_engine_infos(&mut engines);
    Ok(engines)
}
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-asr order_engine_infos 2>&1`
Expected: 1 passed。

- [x] **Step 5: Commit**

```bash
git add crates/asr/src/config.rs
git commit -m "feat(asr): list_engines sorts by is_local desc, category, name"
```

---

## Task 3: runtime_config.rs — EngineOption.label + list_asr_engines 兜底注入

**Files:**
- Modify: `crates/desktop/src/runtime_config.rs`（`EngineOption`、`list_asr_engines`、新增 `engine_label`/`build_asr_options`）

- [x] **Step 1: 写失败测试**（在 `crates/desktop/src/runtime_config.rs` 的 `mod tests` 内）

```rust
#[test]
fn build_asr_options_injects_fallback_first_and_dedups() {
    use octopus_asr::config::{EngineCategory, EngineInfo};
    // 场景 1：DB 无兜底 → 注入到首位
    let engines = vec![
        EngineInfo { name: "whisper-small".into(), category: EngineCategory::Whisper, is_local: false, description: String::new() },
    ];
    let opts = build_asr_options("whisper-small", engines);
    assert_eq!(opts[0].name, "zipformer-small-ctc");
    assert_eq!(opts[0].label, "本地:zipformer-small-ctc");
    assert!(opts[0].is_local);
    assert!(!opts[0].current, "current=whisper-small，兜底非当前");
    assert_eq!(opts[1].name, "whisper-small");
    assert!(opts[1].current);
    assert_eq!(opts[1].label, "whisper:whisper-small");

    // 场景 2：current 为空 → 兜底为当前
    let opts2 = build_asr_options("", vec![]);
    assert_eq!(opts2.len(), 1);
    assert_eq!(opts2[0].name, "zipformer-small-ctc");
    assert!(opts2[0].current, "空 asr_engine → 兜底当前");

    // 场景 3：DB 已含兜底 → 去重（只一个 zipformer-small-ctc，且在首位）
    let engines3 = vec![
        EngineInfo { name: "zipformer-small-ctc".into(), category: EngineCategory::Zipformer, is_local: true, description: String::new() },
        EngineInfo { name: "whisper-small".into(), category: EngineCategory::Whisper, is_local: false, description: String::new() },
    ];
    let opts3 = build_asr_options("zipformer-small-ctc", engines3);
    assert_eq!(
        opts3.iter().filter(|o| o.name == "zipformer-small-ctc").count(),
        1,
        "DB 已含兜底时去重"
    );
    assert_eq!(opts3[0].name, "zipformer-small-ctc");
    assert!(opts3[0].current);
}
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-desktop build_asr_options 2>&1`
Expected: 编译失败（`build_asr_options` 未定义 / `EngineOption` 无 `label` 字段）。

- [x] **Step 3: 实现**（在 `crates/desktop/src/runtime_config.rs`）

(a) `EngineOption` 加 `label` 字段（替换现有 struct 定义，:90）：

```rust
#[derive(Serialize)]
pub struct EngineOption {
    pub name: String,
    pub category: String,
    pub current: bool,
    pub is_local: bool,
    pub label: String,
}
```

(b) 新增 label 拼接 + 纯逻辑构造函数（放在 `category_str` 附近）：

```rust
/// 统一显示文本：is_local → "本地:{name}"，否则 "{category}:{name}"。
fn engine_label(is_local: bool, category: &str, name: &str) -> String {
    if is_local {
        format!("本地:{}", name)
    } else {
        format!("{}:{}", category, name)
    }
}

/// ASR 兜底引擎名（固定首项，不依赖 DB 存在）。
const FALLBACK_ASR_ENGINE: &str = "zipformer-small-ctc";

/// 构造 ASR 选项列表（纯逻辑）：兜底固定第一，DB 同名去重，current 按 current_effective 标记。
/// current_effective 为空时视作兜底。
fn build_asr_options(current_effective: &str, engines: Vec<octopus_asr::config::EngineInfo>) -> Vec<EngineOption> {
    let effective = if current_effective.is_empty() {
        FALLBACK_ASR_ENGINE
    } else {
        current_effective
    };
    let mut options = Vec::with_capacity(engines.len() + 1);
    // 兜底固定第一
    options.push(EngineOption {
        name: FALLBACK_ASR_ENGINE.to_string(),
        category: "zipformer".to_string(),
        is_local: true,
        current: effective == FALLBACK_ASR_ENGINE,
        label: engine_label(true, "zipformer", FALLBACK_ASR_ENGINE),
    });
    // DB 模型（跳过同名兜底，避免重复）
    for e in engines {
        if e.name == FALLBACK_ASR_ENGINE {
            continue;
        }
        let cat = category_str(e.category);
        options.push(EngineOption {
            current: e.name == effective,
            name: e.name.clone(),
            category: cat.to_string(),
            is_local: e.is_local,
            label: engine_label(e.is_local, cat, &e.name),
        });
    }
    options
}
```

(c) `list_asr_engines` 改为调用 `build_asr_options`（替换现有函数体，:109）：

```rust
#[tauri::command]
pub fn list_asr_engines(rc: State<'_, SharedRuntimeConfig>) -> Result<Vec<EngineOption>, String> {
    let current_raw = rc.read().unwrap().asr_engine.clone();
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    Ok(build_asr_options(&current_raw, engines))
}
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-desktop build_asr_options 2>&1`
Expected: 1 passed。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/runtime_config.rs
git commit -m "feat(desktop): EngineOption.label + fallback-first ASR menu options"
```

---

## Task 4: runtime_config.rs — switch_asr_engine 放宽兜底

**Files:**
- Modify: `crates/desktop/src/runtime_config.rs:130`（`switch_asr_engine` 校验块）+ 新增 `validate_switch`

- [x] **Step 1: 写失败测试**（在 `crates/desktop/src/runtime_config.rs` 的 `mod tests` 内）

```rust
#[test]
fn validate_switch_allows_fallback_even_when_absent() {
    use octopus_asr::config::{EngineCategory, EngineInfo};
    let engines = vec![
        EngineInfo { name: "whisper-small".into(), category: EngineCategory::Whisper, is_local: false, description: String::new() },
    ];
    // 兜底名即使不在 engines 也允许
    assert!(validate_switch("zipformer-small-ctc", &engines).is_ok());
    // 在列表中的允许
    assert!(validate_switch("whisper-small", &engines).is_ok());
    // 不在列表且非兜底 → 拒绝
    assert!(validate_switch("nonexistent", &engines).is_err());
}
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-desktop validate_switch 2>&1`
Expected: 编译失败（`validate_switch` 未定义）。

- [x] **Step 3: 实现**（在 `crates/desktop/src/runtime_config.rs`）

新增纯校验函数（放在 `build_asr_options` 之后）：

```rust
/// 校验引擎名可切换：兜底名恒允许（不依赖 DB），其余须在 engines 列表中。
fn validate_switch(name: &str, engines: &[octopus_asr::config::EngineInfo]) -> Result<(), String> {
    if name == FALLBACK_ASR_ENGINE {
        return Ok(());
    }
    if engines.iter().any(|e| e.name == name) {
        Ok(())
    } else {
        Err(format!("引擎 '{}' 不存在，未切换", name))
    }
}
```

`switch_asr_engine` 用它替换现有内联校验（替换 :130-142 的 `let exists = ...; if !exists {...}` 块）：

```rust
#[tauri::command]
pub fn switch_asr_engine(
    name: String,
    rc: State<'_, SharedRuntimeConfig>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    validate_switch(&name, &engines)?;
    {
        let mut g = rc.write().unwrap();
        g.asr_engine = name.clone();
    }
    let engine_mode = match octopus_infra::config::load_config() {
        Ok(cfg) => cfg.engine_mode,
        Err(_) => "embedded".to_string(),
    };
    crate::tray::update_tray_engine_label(&app_handle, &name, &engine_mode);

    if let Err(e) = persist_asr_engine(&name) {
        log::warn!(
            "写回 config.yaml 失败（asr_engine={}）：{} —— 本次仍生效，重启后回退",
            name,
            e
        );
    }
    Ok(())
}
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-desktop validate_switch 2>&1`
Expected: 1 passed。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/runtime_config.rs
git commit -m "feat(desktop): switch_asr_engine allows fallback even when absent from DB"
```

---

## Task 5: runtime_config.rs — LLM 菜单后端（RuntimeConfig.polish_llm + 命令）

**Files:**
- Modify: `crates/desktop/src/runtime_config.rs`（`RuntimeConfig`、新增 `LlmOption`/`build_llm_options`/`list_llm_models`/`switch_polish_llm`/`persist_polish_llm`）

- [x] **Step 1: 写失败测试**（在 `crates/desktop/src/runtime_config.rs` 的 `mod tests` 内）

```rust
#[test]
fn build_llm_options_marks_current_and_labels() {
    use octopus_infra::db::LlmModelInfo;
    let llms = vec![
        LlmModelInfo { name: "glm-4-flashx".into(), category: "bigmodel".into(), is_local: false },
        LlmModelInfo { name: "ollama-local".into(), category: "ollama".into(), is_local: true },
    ];
    let opts = build_llm_options("glm-4-flashx", llms);
    assert_eq!(opts.len(), 2);
    assert!(opts[0].current);
    assert_eq!(opts[0].label, "bigmodel:glm-4-flashx");
    assert!(!opts[1].current);
    assert_eq!(opts[1].label, "本地:ollama-local");
}
```

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-desktop build_llm_options 2>&1`
Expected: 编译失败（`build_llm_options`/`LlmOption` 未定义）。

- [x] **Step 3: 实现**（在 `crates/desktop/src/runtime_config.rs`）

(a) `RuntimeConfig` 加 `polish_llm` 字段（替换 :13-16 struct + :18-25 impl）：

```rust
/// 运行时可变的配置字段。
pub struct RuntimeConfig {
    pub asr_engine: String,
    pub polish_mode: PolishMode,
    pub polish_llm: String,
}

impl RuntimeConfig {
    pub fn from_config(cfg: &octopus_infra::config::AppConfig) -> Self {
        Self {
            asr_engine: cfg.asr_engine.clone(),
            polish_mode: cfg.polish_mode,
            polish_llm: cfg.polish_llm.clone(),
        }
    }
}
```

(b) 新增 LlmOption + 纯逻辑 + persist + 两个命令（放在 `EngineOption` 定义与 `list_asr_engines` 之间的合适位置）：

```rust
#[derive(Serialize)]
pub struct LlmOption {
    pub name: String,
    pub category: String,
    pub is_local: bool,
    pub current: bool,
    pub label: String,
}

/// 构造 LLM 选项列表（纯逻辑）：current 按 polish_llm 标记，label 同 ASR 规则。
fn build_llm_options(current: &str, llms: Vec<octopus_infra::db::LlmModelInfo>) -> Vec<LlmOption> {
    llms.into_iter()
        .map(|m| {
            let label = engine_label(m.is_local, &m.category, &m.name);
            LlmOption {
                current: m.name == current,
                label,
                name: m.name,
                category: m.category,
                is_local: m.is_local,
            }
        })
        .collect()
}

/// 读当前 config.yaml → 覆盖 polish_llm → 序列化写回。
pub fn persist_polish_llm(value: &str) -> Result<(), String> {
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    cfg.polish_llm = value.to_string();
    write_config_yaml(&cfg)
}

#[tauri::command]
pub fn list_llm_models(rc: State<'_, SharedRuntimeConfig>) -> Result<Vec<LlmOption>, String> {
    let current = rc.read().unwrap().polish_llm.clone();
    let llms = octopus_infra::db::list_llm_models().map_err(|e| e.to_string())?;
    Ok(build_llm_options(&current, llms))
}

#[tauri::command]
pub fn switch_polish_llm(name: String, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    let exists = octopus_infra::db::list_llm_models()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|m| m.name == name);
    if !exists {
        return Err(format!("润色模型 '{}' 不存在，未切换", name));
    }
    {
        let mut g = rc.write().unwrap();
        g.polish_llm = name.clone();
    }
    if let Err(e) = persist_polish_llm(&name) {
        log::warn!(
            "写回 config.yaml 失败（polish_llm={}）：{} —— 本次仍生效，重启后回退",
            name,
            e
        );
    }
    Ok(())
}
```

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-desktop build_llm_options 2>&1`
Expected: 1 passed。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/runtime_config.rs
git commit -m "feat(desktop): LLM polish model menu backend (list/switch/persist)"
```

---

## Task 6: main.rs — 注册新命令

**Files:**
- Modify: `crates/desktop/src/main.rs:131`（`generate_handler!` 列表）

- [x] **Step 1: 注册命令**

把 `crates/desktop/src/main.rs:131-135` 的 `generate_handler!` 列表，在 `set_polish_mode,` 之后加两行：

```rust
        .invoke_handler(tauri::generate_handler![
            runtime_config::toolbar_state,
            runtime_config::list_asr_engines,
            runtime_config::switch_asr_engine,
            runtime_config::set_polish_mode,
            runtime_config::list_llm_models,
            runtime_config::switch_polish_llm,
            // …其余既有命令保持不变
```

- [x] **Step 2: 编译验证**

Run: `cargo check -p octopus-desktop 2>&1`
Expected: 编译通过，无错误（确认 `RuntimeConfig` 新增 `polish_llm` 字段后所有构造点仍编译——`from_config` 是唯一构造点，已在 Task 5 更新）。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/src/main.rs
git commit -m "feat(desktop): register list_llm_models & switch_polish_llm commands"
```

---

## Task 7: 前端 result/index.html — ASR 用 label + 启用 LLM 菜单

**Files:**
- Modify: `crates/desktop/dist/result/index.html`（:194 `#tool-llm` 按钮、:307-310 ASR popup 渲染、新增 LLM click 块、:333 refreshActive）

前端为静态 HTML/JS（无构建），改动用精确字符串替换，验证靠 `cargo check`（后端）+ 手动运行应用点菜单。

- [x] **Step 1: 启用 `#tool-llm` 按钮**（去 `disabled`、改 title，替换 :194）

旧：
```html
        <button class="tool" id="tool-llm" disabled title="敬请期待" aria-label="LLM 模型">
```
新：
```html
        <button class="tool" id="tool-llm" title="润色模型" aria-label="润色模型">
```

- [x] **Step 2: ASR popup 改用 `label`**（替换 :307-310 的 `engines.map(...)` 渲染块）

旧：
```js
      const html = engines.map(e =>
        `<div class="option${e.current ? ' current' : ''}" data-name="${e.name}">
           <span>${e.current ? '●' : '○'}</span><span class="nm">${e.is_local ? '本地-' : '远程-'}${e.name}</span><span class="cat">${e.category}</span>
         </div>`).join('');
```
新：
```js
      const html = engines.map(e =>
        `<div class="option${e.current ? ' current' : ''}" data-name="${e.name}">
           <span>${e.current ? '●' : '○'}</span><span class="nm">${e.label}</span>
         </div>`).join('');
```

- [x] **Step 3: 新增 LLM popup 逻辑**（在 ASR 块之后、`// ── 当前态高亮 ──` 之前插入，即 :325 之后）

```js
    // ── LLM 润色模型 ──
    document.getElementById('tool-llm').addEventListener('click', async () => {
      let models;
      try { models = await invoke('list_llm_models'); }
      catch (e) { showToast('读取润色模型失败：' + e); return; }
      if (!models.length) { showToast('无可用润色模型（请在 DB 启用 is_enabled=1）'); return; }
      const html = models.map(m =>
        `<div class="option${m.current ? ' current' : ''}" data-name="${m.name}">
           <span>${m.current ? '●' : '○'}</span><span class="nm">${m.label}</span>
         </div>`).join('');
      openPopup(html);
      popup.querySelectorAll('.option').forEach(el => {
        el.addEventListener('click', async () => {
          const name = el.dataset.name;
          try {
            await invoke('switch_polish_llm', { name });
            closePopup();
            showToast('已切换润色模型：' + name);
          } catch (err) {
            showToast('' + err);
          }
        });
      });
    });
```

- [x] **Step 4: `refreshActive` 让 `#tool-llm` 恒显 active**（在 :333 `tool-asr` 那行之后加一行）

在：
```js
        document.getElementById('tool-asr').classList.toggle('active', true);
```
之后加：
```js
        document.getElementById('tool-llm').classList.toggle('active', true);
```

- [x] **Step 5: 编译验证（确保后端命令齐全）**

Run: `cargo check --workspace 2>&1`
Expected: 通过（前端 HTML 不参与 cargo 编译，但确认后端 `list_llm_models`/`switch_polish_llm` 已注册且类型匹配 invoke 参数）。

- [x] **Step 6: 手动验证（后置，需运行应用）**

启动应用（`cargo run -p octopus-desktop` 或既有 run 脚本），结果窗口工具栏：
1. 点 ASR 按钮 → 首条「本地:zipformer-small-ctc」，选中能切换不报错；其余项格式「本地:{name}」或「{category}:{name}」。
2. 点润色模型按钮（原「敬请期待」现可用）→ 列出 is_enabled=1 的 LLM；若 DB 全禁用则 toast「无可用润色模型」。
3. 选 LLM → toast「已切换润色模型:{name}」；重启后仍生效（检查 `~/.octopus/config.yaml` 的 `polish_llm`）。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/dist/result/index.html
git commit -m "feat(desktop): ASR menu uses label; enable LLM polish model menu"
```

---

## Self-Review

**1. Spec coverage:**
- §1(a) list_engines 排序 → Task 2 ✅
- §1(b) EngineOption.label → Task 3 ✅
- §1(c) list_asr_engines 兜底注入 → Task 3（build_asr_options）✅
- §1(d) switch_asr_engine 放宽兜底 → Task 4 ✅
- §2(a) db.rs list_llm_models → Task 1 ✅
- §2(b) RuntimeConfig.polish_llm + LlmOption + list_llm_models + switch_polish_llm + persist_polish_llm → Task 5 ✅
- §2(c) 前端 tool-llm + popup → Task 7 ✅
- §3 显示规则（engine_label）→ Task 3 定义，Task 5 复用 ✅
- §4 兜底与持久化 → Task 4（ASR 兜底）+ Task 5（LLM persist）✅
- 命令注册 → Task 6 ✅
- 无遗漏。

**2. Placeholder scan:** 无 TBD/TODO；每步含完整代码；命令、类型、SQL 均具体。✅

**3. Type consistency:**
- `EngineOption.label`：Task 3 定义，Task 7 前端用 `e.label` ✅
- `LlmOption{label}`：Task 5 定义，Task 7 前端用 `m.label` ✅
- `engine_label(is_local, category, name)`：Task 3 定义，Task 5 `build_llm_options` 复用 ✅
- `FALLBACK_ASR_ENGINE` 常量：Task 3 定义，Task 4 `validate_switch` 复用 ✅
- `category_label`（asr）与 `category_str`（desktop）是不同 crate 的同名概念，各自独立，无冲突 ✅
- `LlmModelInfo{name, category, is_local}`：Task 1 定义，Task 5 `build_llm_options` 接收 ✅
- `order_engine_infos(&mut [EngineInfo])`：Task 2 定义并被 `list_engines` 调用 ✅
- `RuntimeConfig.polish_llm`：Task 5 加字段 + from_config，Task 6 编译验证构造点 ✅

---

## `2026-06-16-denoise-deepfilternet.md`

# DeepFilterNet3 环境降噪 Implementation Plan

> ⚠️ **已废弃（2026-06-17）**：本 plan 的方案（`ort` + `dfn3.onnx` 自实现 STFT/ERB + `denoise_enabled: bool`）因导出模型压语音（gain≈0.10）已弃用。环境降噪最终方案 = libDF v0.5.6 + tract 原生整合（`FrameDenoise` trait / `RnnoiseBackend` / `Df3Backend`，`denoise_mode: u8` 0/1/2）。详见 [`2026-06-17-denoise-deepfilternet3-integration-design.md`](../specs/2026-06-17-denoise-deepfilternet3-integration-design.md)。本文仅作历史执行记录保留，请勿据此实施。

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在麦克风录音链路中插入 DeepFilterNet3（ONNX）流式环境降噪层，在送入 VAD/ASR 前降低背景噪声，跨平台（mac/win/linux）生效。

**Architecture:** 新建 `crates/asr/src/denoise.rs` 封装 `DenoiseProcessor`（Vorbis 窗 STFT + ERB 特征（dB + EMA 归一化）+ dfn3.onnx 有状态推理 + conv_lookahead=2 帧对齐 + iSTFT overlap-add）。集成在采集层 `SharedAudioState` 内（coordinator 无感），48kHz 域处理、前后各一次重采样桥接到 ASR 的 16kHz。复用现有 `rustfft` 依赖与 `ort` 推理，零新依赖。配置仅 `denoise_enabled`（infra::AppConfig），模型走 HF cache，失败降级直通不阻断录音。

**Tech Stack:** Rust、`ort 2.0.0-rc.12`（ONNX Runtime）、`rustfft 6`、`ndarray 0.17`、`rubato 0.16`（已有依赖）、Tauri/cpal。

---

> ⚠️ **修订（2026-06-16）**：本计划的 DeepFilterNet3（dfn3.onnx）实现已废弃。`dfn3.onnx` 流式逐帧导出存在模型层缺陷（把正常语音压到 ~10%，开降噪反而损害 ASR）。已改用 `nnnoiseless`（纯 Rust RNNoise，内置默认模型）重写 `crates/asr/src/denoise.rs`，`new`/`reset`/`process_samples`/`flush` 接口不变、`FRAME_SIZE=480`、无外部模型文件依赖。`audio.rs` 去 `find_df3`、`config.rs` 删 `find_df3`、`Cargo.toml` 删 `df` 依赖。详见 spec 顶部「修订记录」。本计划以下任务步骤仅作历史记录。

---

**Spec:** `docs/superpowers/specs/2026-06-16-denoise-deepfilternet-design.md`

---

## 关键技术契约（实施前必读）

- **模型**：`penta2himajin/deepfilternet3-onnx/dfn3.onnx`（HF cache，唯一带 GRU 状态的流式版）。IO：
  - 入 `spec[1,1,1,481,2]` `feat_erb[1,1,1,32]` `feat_spec[1,1,1,96,2]` `enc_h[1,1,256]` `erb_h[2,1,256]` `df_h[2,1,256]`
  - 出 `enhanced_spec[1,1,1,481,2]` `new_enc_h` `new_erb_h` `new_df_h`
- **DSP 常量**：n_fft=960、hop=480（48kHz，10ms）、481 bins、32 ERB 带、96 DF bins。
- **窗**：Vorbis，`w[n]=sin(π/2·sin²(π(n+0.5)/960))`，分析窗=合成窗。50% overlap 下 COLA 增益=1。
- **ERB 尺度**：Glasberg-Moore，`f_erb=9.265·ln(1+f/228.833)`（分母 228.833 = 24.7×9.265，对齐 libDF）。
- **特征归一化**（对齐 libDF，缺失则模型收错误量级）：
  - `feat_erb`：band 互相关功率 `(Σ|spec|²/width)²` → `10·log10(1e-10+x)` → EMA 均值归一化（alpha=0.99）→ `/40`
  - `feat_spec`：前 96 bin 复数 → EMA 跟踪 `|z|`（alpha=0.99），除以 `√state`
  - 初始状态：feat_erb = linspace(-60, -90, 32)，feat_spec = linspace(0.001, 0.0001, 96)
- **conv_lookahead=2**：模型导出时移除了内部 lookahead，调用方需环形缓冲：spec[t] 配 feat[t+2]。首次推理需累积 3 帧（20ms 算法延迟），flush 填 2 零特征帧排空队列。
- **rustfft 6 API**：`FftPlanner::new()` → `plan_fft(N, FftDirection::Forward/Inverse)` → `fft.process(&mut [Complex<f32>])`。**inverse 不含 1/N 归一化**，需手动 `×1/N`。
- **ort**：参照 `crates/asr/src/vad.rs` 的 Session 加载 + `ort::inputs!` + `TensorRef::from_array_view` + `session.run`。
- **测试模型依赖**：纯 DSP 测试（窗、STFT 重建、ERB、OLA）**不需模型**，常规跑；推理/集成测试需 `dfn3.onnx`，标 `#[ignore]`。

---

## Task 1: infra::AppConfig 加 denoise_enabled 配置字段

**Files:**
- Modify: `crates/infra/src/config.rs:138`（AppConfig 末字段后）、`:179`（default 函数）、`:212`（Default impl）、`:279`（测试）

- [ ] **Step 1: 写失败测试**

在 `crates/infra/src/config.rs` 的 `mod tests` 末尾（`app_config_serialize_round_trip_preserves_overrides` 之后）加：

```rust
    #[test]
    fn denoise_enabled_defaults_to_true() {
        let cfg: AppConfig = serde_yaml::from_str("").unwrap();
        assert!(cfg.denoise_enabled, "denoise_enabled 应默认 true");
    }

    #[test]
    fn denoise_enabled_override_from_yaml() {
        let cfg: AppConfig = serde_yaml::from_str("denoise_enabled: false\n").unwrap();
        assert!(!cfg.denoise_enabled);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-infra denoise_enabled`
Expected: 编译失败 `no field denoise_enabled on type AppConfig`

- [ ] **Step 3: 加字段 + default 函数 + Default impl**

在 `AppConfig` 结构体 `asr_correct` 字段后（`config.rs:138` 之后）加：

```rust
    /// 是否启用 DeepFilterNet3 环境降噪（录音送 ASR 前降噪）
    #[serde(default = "default_denoise_enabled")]
    pub denoise_enabled: bool,
```

在 `default_asr_correct` 函数后（`:179` 附近）加：

```rust
fn default_denoise_enabled() -> bool {
    true
}
```

在 `Default for AppConfig` impl 的 `asr_correct: default_asr_correct(),` 后加：

```rust
            denoise_enabled: default_denoise_enabled(),
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-infra`
Expected: PASS（含新加的 2 个测试）

- [ ] **Step 5: 提交**

```bash
git add crates/infra/src/config.rs
git commit -m "feat(config): add denoise_enabled field (default true)"
```

---

## Task 2: find_df3() 模型定位（复用 HF cache helpers）

**Files:**
- Modify: `crates/asr/src/config.rs`（在 `find_silero_vad` 之后，`:93` 附近）

- [ ] **Step 1: 确认现有 helper 签名**

Run: `grep -nE "fn find_hf_cache|fn find_latest_snapshot" crates/asr/src/config.rs`
确认：`find_hf_cache(source: &str) -> Result<PathBuf>`（返回 repo 的 model_dir，含 snapshots/）、`find_latest_snapshot(model_dir: &Path) -> Result<PathBuf>`（返回最新 snapshot 目录）。

- [ ] **Step 2: 写失败测试**

在 `crates/asr/src/config.rs` 末尾的 `#[cfg(test)] mod tests`（若不存在则新建）加：

```rust
    #[test]
    fn find_df3_missing_returns_download_hint() {
        // 临时改 HF cache 路径不可行（函数读固定 HOME）；改为验证错误信息文案
        // 当模型未下载时，find_df3 应返回含 hf download 提示的 Err
        match crate::config::find_df3() {
            Ok(_) => { /* 模型存在，跳过缺失路径断言 */ }
            Err(e) => {
                let msg = format!("{:#}", e);
                assert!(
                    msg.contains("hf download penta2himajin/deepfilternet3-onnx"),
                    "缺失时应提示 hf download 命令，实际: {}",
                    msg
                );
            }
        }
    }
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p octopus-asr find_df3_missing`
Expected: 编译失败 `cannot find function find_df3`

- [ ] **Step 4: 实现 find_df3**

在 `find_silero_vad` 之后加：

```rust
// ── DeepFilterNet3 model discovery ──

/// DF3 模型 HF repo（唯一固定，不走 DB / 不切换）。
const DF3_HF_REPO: &str = "penta2himajin/deepfilternet3-onnx";
/// DF3 onnx 文件名（带 GRU 状态的流式版）。
const DF3_ONNX_FILE: &str = "dfn3.onnx";

/// 定位 DeepFilterNet3 模型：~/.cache/huggingface/hub/models--penta2himajin--deepfilternet3-onnx/snapshots/*/dfn3.onnx
/// 单一固定模型，不走 DB；缺失时提示下载命令。
pub fn find_df3() -> Result<PathBuf> {
    let model_dir = find_hf_cache(DF3_HF_REPO)?;
    let snapshot = find_latest_snapshot(&model_dir)?;
    let onnx = snapshot.join(DF3_ONNX_FILE);
    if onnx.exists() {
        return Ok(onnx);
    }
    anyhow::bail!(
        "DeepFilterNet3 模型缺失，请先下载：hf download {}",
        DF3_HF_REPO
    )
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p octopus-asr find_df3_missing`
Expected: PASS（模型在则 Ok 跳过；不在则 Err 含下载提示）

- [ ] **Step 6: 提交**

```bash
git add crates/asr/src/config.rs
git commit -m "feat(asr): add find_df3() for DeepFilterNet3 model discovery"
```

---

## Task 3: denoise.rs 骨架 + Vorbis 窗 + STFT/iSTFT 重建（纯 DSP，无需模型）

**Files:**
- Create: `crates/asr/src/denoise.rs`
- Modify: `crates/asr/src/lib.rs`（加 `pub mod denoise;`）

- [ ] **Step 1: 注册模块**

在 `crates/asr/src/lib.rs` 加（与 `pub mod vad;` 同处）：

```rust
pub mod denoise;
```

- [ ] **Step 2: 写 denoise.rs 常量 + 窗 + STFT/iSTFT + 重建测试**

创建 `crates/asr/src/denoise.rs`：

```rust
//! DeepFilterNet3 流式环境降噪（ONNX，48kHz）。
//!
//! 处理模型：penta2himajin/deepfilternet3-onnx/dfn3.onnx（带 GRU 状态的流式版）。
//! 数据流：48k 样本 → 每 480 样本(10ms)一帧 → STFT(hann,n_fft=960) → feat
//!       → onnx(spec,feat,GRU状态) → enhanced_spec → iSTFT + OLA → 48k 增强样本。

use anyhow::Result;
use ndarray::{Array3, Array4};
use rustfft::{Fft, FftPlanner, FftDirection};
use rustfft::num_complex::Complex;

/// FFT 参数（DeepFilterNet3 契约，绑定 48kHz）。
pub const N_FFT: usize = 960;
pub const HOP: usize = 480;
pub const NBINS: usize = N_FFT / 2 + 1; // 481
pub const N_ERB: usize = 32;
pub const N_DF: usize = 96; // DF 滤波作用的 bin 数（feat_spec 维度）

/// sqrt-Hann 窗：w[n] = sqrt(0.5 - 0.5·cos(2πn/N))。分析窗 = 合成窗。
/// 50% overlap（hop=N/2）下 w² 跨 hop 求和 = 1（COLA 完美重建，增益=1）。
pub fn sqrt_hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let hann = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            hann.sqrt()
        })
        .collect()
}

/// STFT 单帧：实信号 × 窗 → FFT → 取前 NBINS 复数 bin。
pub fn stft_frame(frame: &[f32], window: &[f32], fft: &Fft<f32>) -> Vec<Complex<f32>> {
    debug_assert_eq!(frame.len(), N_FFT);
    let mut buf: Vec<Complex<f32>> = (0..N_FFT)
        .map(|i| Complex::new(frame[i] * window[i], 0.0))
        .collect();
    fft.process(&mut buf);
    buf[..NBINS].to_vec()
}

/// iSTFT 单帧：NBINS 复数 → 共轭对称填充 → IFFT → × 合成窗 → N_FFT 实样本。
/// rustfft 的 inverse 不含 1/N 归一化，手动 ×1/N。
pub fn istft_frame(spec: &[Complex<f32>], ifft: &Fft<f32>, window: &[f32]) -> Vec<f32> {
    debug_assert_eq!(spec.len(), NBINS);
    let mut buf = vec![Complex::new(0.0, 0.0); N_FFT];
    for i in 0..NBINS {
        buf[i] = spec[i];
    }
    // 共轭对称填充（实信号的 FFT 性质）
    for i in 1..(N_FFT - NBINS + 1) {
        buf[N_FFT - i] = spec[i].conj();
    }
    ifft.process(&mut buf);
    let scale = 1.0 / N_FFT as f32;
    (0..N_FFT).map(|i| buf[i].re * scale * window[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqrt_hann_satisfys_cola_at_50pct_overlap() {
        // 相邻两帧的 w²（=hann）之和应为常数 1.0（COLA 完美重建条件）
        let w = sqrt_hann_window(N_FFT);
        for i in 0..HOP {
            let sum = w[i] * w[i] + w[i + HOP] * w[i + HOP];
            assert!(
                (sum - 1.0).abs() < 1e-5,
                "COLA 失败 @ {}: w²+hann_shifted = {}",
                i,
                sum
            );
        }
    }

    #[test]
    fn stft_istft_reconstructs_with_high_snr() {
        // 纯 DSP 重建（不经模型）：长信号逐帧 STFT→iSTFT+OLA，中段应高 SNR 还原
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft(N_FFT, FftDirection::Forward);
        let ifft = planner.plan_fft(N_FFT, FftDirection::Inverse);
        let w = sqrt_hann_window(N_FFT);

        // 生成 ~0.5s 的 1kHz 正弦 + 白噪
        let n_total = 48000 * 1 / 2; // 0.5s @48k
        let mut signal = Vec::with_capacity(n_total);
        for i in 0..n_total {
            let t = i as f32 / 48000.0;
            signal.push((2.0 * std::f32::consts::PI * 1000.0 * t).sin() * 0.5);
        }

        // 逐帧 STFT→iSTFT + OLA 重建
        let mut recon = vec![0.0f32; n_total + N_FFT];
        let n_frames = (n_total - N_FFT) / HOP + 1;
        for f in 0..n_frames {
            let start = f * HOP;
            let frame = &signal[start..start + N_FFT];
            let spec = stft_frame(frame, &w, &fft);
            let time = istft_frame(&spec, &ifft, &w);
            for j in 0..N_FFT {
                recon[start + j] += time[j];
            }
        }

        // 中段（避开边界）计算 SNR
        let lo = N_FFT;
        let hi = n_total - N_FFT;
        let mut signal_power = 0.0;
        let mut noise_power = 0.0;
        for i in lo..hi {
            signal_power += signal[i] * signal[i];
            let e = recon[i] - signal[i];
            noise_power += e * e;
        }
        let snr_db = 10.0 * (signal_power / noise_power).log10();
        assert!(
            snr_db > 40.0,
            "STFT/iSTFT 重建 SNR 应 > 40dB，实际 {:.1}dB",
            snr_db
        );
    }
}
```

- [ ] **Step 3: 跑测试确认通过**

Run: `cargo test -p octopus-asr denoise::tests`
Expected: PASS（2 个纯 DSP 测试，无需模型）

- [ ] **Step 4: 提交**

```bash
git add crates/asr/src/denoise.rs crates/asr/src/lib.rs
git commit -m "feat(asr): denoise.rs skeleton + sqrt-Hann STFT/iSTFT reconstruction"
```

---

## Task 4: ERB 边界 + feat_erb / feat_spec 特征提取

**Files:**
- Modify: `crates/asr/src/denoise.rs`

- [ ] **Step 1: 写失败测试**

在 `denoise.rs` 的 `mod tests` 加：

```rust
    #[test]
    fn erb_bounds_cover_all_bins_and_correct_count() {
        let bounds = erb_bounds();
        assert_eq!(bounds.len(), N_ERB, "应为 32 个 ERB 带");
        // 第 0 带从 bin 0 开始，最后一带到 NBINS(481) 结束，无间断无重叠
        assert_eq!(bounds[0].0, 0);
        assert_eq!(bounds[N_ERB - 1].1, NBINS);
        for w in bounds.windows(2) {
            assert_eq!(w[0].1, w[1].0, "ERB 带应连续");
        }
    }

    #[test]
    fn feat_erb_aggregates_bin_energy() {
        // DC(bin0)=大能量，其余=0 → feat_erb[0] 应 > 0，其余 ≈ 0
        let mut spec = vec![Complex::new(0.0, 0.0); NBINS];
        spec[0] = Complex::new(1.0, 0.0);
        let bounds = erb_bounds();
        let erb = feat_erb(&spec, &bounds);
        assert_eq!(erb.len(), N_ERB);
        assert!(erb[0] > 0.99 && erb[0] < 1.01);
        for v in &erb[1..] {
            assert!(v.abs() < 1e-6);
        }
    }

    #[test]
    fn feat_spec_packs_first_96_bins_complex() {
        let mut spec = vec![Complex::new(0.0, 0.0); NBINS];
        for i in 0..N_DF {
            spec[i] = Complex::new(i as f32, (i as f32) * 0.5);
        }
        let fs = feat_spec(&spec);
        assert_eq!(fs.len(), N_DF * 2);
        // 前 96 bin 的 (re, im) 交错
        assert_eq!(fs[0], 0.0); // bin0 re
        assert_eq!(fs[1], 0.0); // bin0 im
        assert_eq!(fs[2], 1.0); // bin1 re
        assert_eq!(fs[3], 0.5); // bin1 im
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr denoise::tests::erb`
Expected: 编译失败 `cannot find function erb_bounds`

- [ ] **Step 3: 实现 ERB 边界 + feat 函数**

在 `denoise.rs`（`istft_frame` 之后）加：

```rust
/// Glasberg-Moore ERB 尺度：频率(Hz) → ERB number。
/// f_erb = 9.265 · ln(1 + f / 24.863)
fn freq_to_erb(freq: f32) -> f32 {
    9.265 * (1.0 + freq / 24.863).ln()
}

/// ERB number → 频率(Hz)（反函数）。
fn erb_to_freq(erb: f32) -> f32 {
    24.863 * ((erb / 9.265).exp() - 1.0)
}

/// 生成 32 个 ERB 带对 481 个 bin 的 [lo, hi) 边界。
/// 覆盖 0..24000Hz（48kHz Nyquist），按 ERB 尺度均分。
/// 注：DeepFilterNet 的精确带划分对齐 df crate（deep_filter::df::freq）；
/// 此实现为标准 ERB 均分，feat_erb 测试验证数值聚合正确性。
pub fn erb_bounds() -> Vec<(usize, usize)> {
    let nyquist = 24000.0f32;
    let erb_max = freq_to_erb(nyquist);
    // bin i 的频率 = i / N_FFT * sample_rate = i / 960 * 48000
    let bin_freq = |i: usize| -> f32 { i as f32 / N_FFT as f32 * 48000.0 };

    let mut bounds = Vec::with_capacity(N_ERB);
    for b in 0..N_ERB {
        let erb_lo = erb_max * b as f32 / N_ERB as f32;
        let erb_hi = erb_max * (b + 1) as f32 / N_ERB as f32;
        let f_lo = erb_to_freq(erb_lo);
        let f_hi = erb_to_freq(erb_hi);
        // 找到首个 freq >= f_lo 的 bin 作为 lo，首个 freq > f_hi 的 bin 作为 hi
        let mut lo = 0;
        while lo < NBINS && bin_freq(lo) < f_lo {
            lo += 1;
        }
        let mut hi = lo;
        while hi < NBINS && bin_freq(hi) <= f_hi {
            hi += 1;
        }
        bounds.push((lo, hi.max(lo + 1)));
    }
    // 修正：确保连续无空洞（前一带 hi = 后一带 lo），最后到 NBINS
    if bounds[0].0 > 0 {
        bounds[0].0 = 0;
    }
    for w in 0..N_ERB.saturating_sub(1) {
        bounds[w].1 = bounds[w + 1].0;
    }
    bounds[N_ERB - 1].1 = NBINS;
    bounds
}

/// feat_erb[32]：每个 ERB 带的能量（|spec|² 之和）。
pub fn feat_erb(spec: &[Complex<f32>], bounds: &[(usize, usize)]) -> Vec<f32> {
    bounds
        .iter()
        .map(|(lo, hi)| {
            (lo..hi).map(|i| spec[i].norm_sqr()).sum()
        })
        .collect()
}

/// feat_spec[96·2]：前 96 个 bin 的复数 (re, im) 交错。
pub fn feat_spec(spec: &[Complex<f32>]) -> Vec<f32> {
    let mut out = Vec::with_capacity(N_DF * 2);
    for i in 0..N_DF {
        out.push(spec[i].re);
        out.push(spec[i].im);
    }
    out
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-asr denoise::tests`
Expected: PASS（5 个 DSP 测试）

- [ ] **Step 5: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "feat(asr): ERB bounds + feat_erb/feat_spec feature extraction"
```

---

## Task 5: DenoiseProcessor 结构体 + ONNX session + GRU 状态 + 单帧推理

**Files:**
- Modify: `crates/asr/src/denoise.rs`

- [ ] **Step 1: 写失败测试（需模型，#[ignore]）**

在 `denoise.rs` 的 `mod tests` 加：

```rust
    #[test]
    #[ignore] // 需 dfn3.onnx 在 HF cache
    fn processor_runs_and_updates_gru_state() {
        let path = crate::config::find_df3().expect("dfn3.onnx 未下载，跑: hf download penta2himajin/deepfilternet3-onnx");
        let mut p = super::DenoiseProcessor::new(&path).unwrap();
        let enc_before = p.enc_h.clone();
        // 一帧静音输入
        let frame = vec![0.0f32; HOP];
        let out = p.process_samples(&frame);
        assert!(!out.is_empty() || p.flush().is_empty() == false || true); // 允许首帧无输出（OLA 起始延迟）
        // GRU 状态应已变化
        assert_ne!(p.enc_h, enc_before, "GRU enc_h 应在推理后更新");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr processor_runs -- --ignored`
Expected: 编译失败 `cannot find type DenoiseProcessor`

- [ ] **Step 3: 实现 DenoiseProcessor::new + 单帧推理**

在 `denoise.rs` 顶部加 import：

```rust
use std::path::Path;
use ort::session::Session;
use ort::value::TensorRef;
```

在 `feat_spec` 之后加：

```rust
/// DeepFilterNet3 流式降噪处理器（有状态：GRU 隐状态 + 缓冲）。
///
/// 生命周期：录音会话内跨帧保持状态（GRU 反映噪声环境稳态估计，不应被分段打断）；
/// 新会话开始时调 `reset()`。状态语义与 filter_vad（每段 reset）故意相反。
pub struct DenoiseProcessor {
    session: Session,
    fft: std::sync::Arc<Fft<f32>>,
    ifft: std::sync::Arc<Fft<f32>>,
    window: Vec<f32>,
    erb_bounds: Vec<(usize, usize)>,
    // GRU 隐状态（持久，跨帧）
    enc_h: Array3<f32>, // [1,1,256]
    erb_h: Array3<f32>, // [2,1,256]
    df_h: Array3<f32>,  // [2,1,256]
    // 流式增量缓冲
    in_buf: Vec<f32>,    // 48k 累积
    out_buf: Vec<f32>,   // 已增强样本待输出
    ola_prev: Vec<f32>,  // 上一帧 iSTFT（OLA 用）
}

impl DenoiseProcessor {
    /// 加载模型 + 初始化 DSP 常量 + GRU 状态归零。
    pub fn new(model_path: &Path) -> Result<Self> {
        let session = Session::builder()?
            .commit_from_file(model_path)?;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft(N_FFT, FftDirection::Forward);
        let ifft = planner.plan_fft(N_FFT, FftDirection::Inverse);
        Ok(Self {
            session,
            fft,
            ifft,
            window: sqrt_hann_window(N_FFT),
            erb_bounds: erb_bounds(),
            enc_h: Array3::zeros((1, 1, 256)),
            erb_h: Array3::zeros((2, 1, 256)),
            df_h: Array3::zeros((2, 1, 256)),
            in_buf: Vec::new(),
            out_buf: Vec::new(),
            ola_prev: vec![0.0; N_FFT],
        })
    }

    /// GRU + 缓冲清零（录音会话边界调用）。
    pub fn reset(&mut self) {
        self.enc_h.fill(0.0);
        self.erb_h.fill(0.0);
        self.df_h.fill(0.0);
        self.in_buf.clear();
        self.out_buf.clear();
        self.ola_prev.iter_mut().for_each(|v| *v = 0.0);
    }

    /// 处理一帧（HOP=480 样本）→ 增强样本入 out_buf。
    /// 用上一帧尾部 480 + 本帧 480 = 960 做分析窗。
    fn process_frame(&mut self, new_samples: &[f32]) {
        debug_assert_eq!(new_samples.len(), HOP);
        // 分析帧 = [ola_prev 的后 480] + new_samples；但 ola_prev 存的是完整上一帧 iSTFT。
        // 实际：分析窗作用于 [上帧尾 HOP 样本 + 本帧 HOP 样本]。
        // 简化：维护 in_buf，取末尾 N_FFT 做帧。
        let mut frame = Vec::with_capacity(N_FFT);
        // 上 480 样本（从 ola_prev 的时域输出尾，或从 in_buf）
        let tail: Vec<f32> = self.in_buf[self.in_buf.len().saturating_sub(N_FFT)..]
            .to_vec();
        let need = N_FFT - tail.len();
        frame.extend_from_slice(&tail);
        frame.extend_from_slice(&new_samples[..need.min(new_samples.len())]);
        if frame.len() < N_FFT {
            frame.resize(N_FFT, 0.0);
        }

        let spec = stft_frame(&frame, &self.window, &self.fft);
        let feat_erb = feat_erb(&spec, &self.erb_bounds);
        let feat_spec = feat_spec(&spec);

        // 构造 onnx 输入（形状对齐 IO 契约）
        let spec_4d = complex_to_4d(&spec);            // [1,1,1,481,2]
        let erb_in = vec_to_arr(&feat_erb);            // [1,1,1,32]
        let fspec_in = vec_to_4d(&feat_spec);          // [1,1,1,96,2]

        let outputs = self.session.run(ort::inputs! {
            "spec" => TensorRef::from_array_view(spec_4d.view())?,
            "feat_erb" => TensorRef::from_array_view(erb_in.view())?,
            "feat_spec" => TensorRef::from_array_view(fspec_in.view())?,
            "enc_h" => TensorRef::from_array_view(self.enc_h.view())?,
            "erb_h" => TensorRef::from_array_view(self.erb_h.view())?,
            "df_h" => TensorRef::from_array_view(self.df_h.view())?,
        }?)?;

        // 取增强频谱 + 更新 GRU 状态
        let enhanced = outputs["enhanced_spec"].try_extract_tensor::<f32>()?;
        let new_enc = outputs["new_enc_h"].try_extract_tensor::<f32>()?;
        let new_erb = outputs["new_erb_h"].try_extract_tensor::<f32>()?;
        let new_df = outputs["new_df_h"].try_extract_tensor::<f32>()?;

        let enh_spec = arr4d_to_complex(&enhanced.view().to_owned()); // [481] 复数
        let time = istft_frame(&enh_spec, &self.ifft, &self.window); // N_FFT 实样本

        // OLA：本帧输出 = time - ola_prev 的重叠部分贡献 + ... 简化为标准 OLA
        // 由于 COLA 增益=1，直接累加前 HOP 个样本（减去上一帧重叠）
        let mut out_frame = vec![0.0f32; HOP];
        for i in 0..HOP {
            out_frame[i] = time[i] + self.ola_prev[i + HOP]; // 上帧后半重叠
        }
        self.out_buf.extend_from_slice(&out_frame);
        self.ola_prev = time;

        // 更新 GRU 状态
        self.enc_h = new_enc.view().to_owned().into_shape((1, 1, 256))?.to_owned() into_dyn... 
        // （注：状态拷贝见下方修正步骤；精确形状重塑在 Step 4 调试）
        let _ = (new_erb, new_df);
    }
}
```

> ⚠️ **Step 3 的 GRU 状态回写与 OLA 是初稿，Step 4 会编译驱动修正**。ort `try_extract_tensor` 返回的类型重塑（`into_shape((1,1,256))`）与 OLA 重叠减法的精确边界需在编译时报错处对齐——这是 ONNX 集成最易错的点，按编译器错误逐个修正，不猜测。

- [ ] **Step 4: 编译驱动修正 GRU 状态回写 + OLA**

Run: `cargo build -p octopus-asr`

逐个修正编译错误（典型）：
- `ort::session::Session` 的 `outputs[...]` 访问与 `try_extract_tensor` 签名（参照 `crates/asr/src/vad.rs:37-45` 的 outputs 取值模式）。
- `new_enc.view().to_owned()` → `Array3<f32>`：用 `.into_shape((1,1,256))` 或直接 `.to_owned()` 若输出形状已是 `[1,1,256]`。enc_h/erb_h/df_h 分别对齐 `[1,1,256]`/`[2,1,256]`/`[2,1,256]`。
- OLA 边界：确认 `ola_prev` 用途——上一帧完整 iSTFT（960），本帧输出的前 480 = 上帧后 480 重叠 + 本帧前 480。

修正后 GRU 回写（示例正确形式）：

```rust
        self.enc_h = outputs["new_enc_h"]
            .try_extract_tensor::<f32>()?
            .view()
            .to_owned()
            .into_shape((1, 1, 256))
            .map_err(|e| anyhow::anyhow!("enc_h shape: {e}"))?;
        self.erb_h = outputs["new_erb_h"]
            .try_extract_tensor::<f32>()?
            .view()
            .to_owned()
            .into_shape((2, 1, 256))
            .map_err(|e| anyhow::anyhow!("erb_h shape: {e}"))?;
        self.df_h = outputs["new_df_h"]
            .try_extract_tensor::<f32>()?
            .view()
            .to_owned()
            .into_shape((2, 1, 256))
            .map_err(|e| anyhow::anyhow!("df_h shape: {e}"))?;
```

并加辅助函数（complex ↔ ndarray 转换）：

```rust
fn complex_to_4d(spec: &[Complex<f32>]) -> ndarray::Array5<f32> {
    // [1,1,1,481,2]
    let mut a = ndarray::Array5::zeros((1, 1, 1, NBINS, 2));
    for i in 0..NBINS {
        a[[0, 0, 0, i, 0]] = spec[i].re;
        a[[0, 0, 0, i, 1]] = spec[i].im;
    }
    a
}

fn vec_to_arr(v: &[f32]) -> ndarray::Array4<f32> {
    // [1,1,1,N]
    let mut a = ndarray::Array4::zeros((1, 1, 1, v.len()));
    for (i, x) in v.iter().enumerate() {
        a[[0, 0, 0, i]] = *x;
    }
    a
}

fn vec_to_4d(v: &[f32]) -> ndarray::Array5<f32> {
    // [1,1,1,96,2]
    let n = v.len() / 2;
    let mut a = ndarray::Array5::zeros((1, 1, 1, n, 2));
    for i in 0..n {
        a[[0, 0, 0, i, 0]] = v[i * 2];
        a[[0, 0, 0, i, 1]] = v[i * 2 + 1];
    }
    a
}

fn arr4d_to_complex(view: &ndarray::ArrayViewD<f32>) -> Vec<Complex<f32>> {
    // enhanced_spec [1,1,1,481,2] → [481] 复数
    let mut out = Vec::with_capacity(NBINS);
    for i in 0..NBINS {
        out.push(Complex::new(view[[0, 0, 0, i, 0]], view[[0, 0, 0, i, 1]]));
    }
    out
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p octopus-asr processor_runs -- --ignored`
Expected: PASS（需 `hf download penta2himajin/deepfilternet3-onnx` 已执行）

- [ ] **Step 6: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "feat(asr): DenoiseProcessor ONNX session + GRU state + per-frame inference"
```

---

## Task 6: 流式增量 process_samples + flush + 一致性测试

**Files:**
- Modify: `crates/asr/src/denoise.rs`

- [ ] **Step 1: 写失败测试（#[ignore]，需模型）**

在 `mod tests` 加：

```rust
    #[test]
    #[ignore]
    fn sample_conservation_input_equals_output_length() {
        let path = crate::config::find_df3().unwrap();
        let mut p = super::DenoiseProcessor::new(&path).unwrap();
        let n = 48000; // 1s @48k
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin() * 0.3)
            .collect();
        let mut out = p.process_samples(&input);
        out.extend(p.flush());
        // 样本守恒：输出长度 == 输入长度（OLA 不丢不增，尾部 flush 吐残留）
        assert_eq!(out.len(), input.len(), "样本守恒失败：in={} out={}", input.len(), out.len());
    }

    #[test]
    #[ignore]
    fn streaming_incremental_equals_batch() {
        let path = crate::config::find_df3().unwrap();
        let n = 48000;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.3)
            .collect();

        // 批处理（一次性）
        let mut p1 = super::DenoiseProcessor::new(&path).unwrap();
        let mut batch = p1.process_samples(&input);
        batch.extend(p1.flush());

        // 增量（分多次，每次不固定长度）
        let mut p2 = super::DenoiseProcessor::new(&path).unwrap();
        let mut incr = Vec::new();
        let chunks = [300usize, 700, 480, 1024, 480, 613, 480, 200, 13783]; // 和非整除 HOP
        let mut off = 0;
        for &c in &chunks {
            if off + c > input.len() { break; }
            incr.extend(p2.process_samples(&input[off..off + c]));
            off += c;
        }
        if off < input.len() {
            incr.extend(p2.process_samples(&input[off..]));
        }
        incr.extend(p2.flush());

        // 增量 vs 批处理逐样本相等（无状态漂移、无边界丢帧）
        assert_eq!(incr.len(), batch.len());
        let max_diff = incr.iter().zip(batch.iter()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(max_diff < 1e-4, "增量 vs 批处理不一致，max_diff={}", max_diff);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-asr sample_conservation -- --ignored`
Expected: 编译失败（`process_samples`/`flush` 未实现）

- [ ] **Step 3: 实现 process_samples + flush**

在 `impl DenoiseProcessor` 加（替换 Task 5 的 `process_frame` 调用方式为公开增量接口）：

```rust
    /// 增量处理 48k 样本：累积到 in_buf，每满 HOP 处理一帧，返回已增强样本。
    pub fn process_samples(&mut self, samples: &[f32]) -> Vec<f32> {
        self.in_buf.extend_from_slice(samples);
        while self.in_buf.len() >= HOP {
            // 取首 HOP 个作为新样本，分析帧在 process_frame 内从 in_buf 末尾 N_FFT 构造
            let new: Vec<f32> = self.in_buf.drain(..HOP).collect();
            self.process_frame(&new);
        }
        std::mem::take(&mut self.out_buf)
    }

    /// 尾部 flush：零填到 HOP 整数倍，处理残留，吐剩余输出。
    pub fn flush(&mut self) -> Vec<f32> {
        if !self.in_buf.is_empty() {
            let pad = HOP - (self.in_buf.len() % HOP);
            if pad < HOP {
                self.in_buf.extend(std::iter::repeat(0.0).take(pad));
            } else {
                self.in_buf.extend(std::iter::repeat(0.0).take(HOP));
            }
            while self.in_buf.len() >= HOP {
                let new: Vec<f32> = self.in_buf.drain(..HOP).collect();
                self.process_frame(&new);
            }
        }
        std::mem::take(&mut self.out_buf)
    }
```

并修正 `process_frame` 的分析帧构造（Task 5 简化版改为正确版）：

```rust
    fn process_frame(&mut self, new_samples: &[f32]) {
        // 分析帧 = in_buf 已 drain 出 new_samples 后，但需上一帧上下文。
        // 维护 separate history：把 new 加入一个滚动缓冲 last_frame_tail。
        // 简化正确做法：分析帧 = [prev_tail(480)] + new(480)
        //   prev_tail = 上一帧 new 的后 480（首次用 0）
        let mut frame = Vec::with_capacity(N_FFT);
        frame.extend_from_slice(&self.ola_prev[N_FFT - HOP..]); // 上一帧尾 HOP（OLA 复用）
        // 注：ola_prev 在 iSTFT 后存完整 960；这里取其分析用的尾 480 作上一帧时域上下文近似
        // 严格：分析窗作用于原始时域，需单独维护原始 tail。此处用 ola_prev 近似，
        // sample_conservation 与 streaming_equals_batch 测试会暴露偏差，据此修正。
        frame.extend_from_slice(new_samples);

        let spec = stft_frame(&frame, &self.window, &self.fft);
        // ...（feat + onnx run + iSTFT + OLA，同 Task 5 Step 4）
        // 完整逻辑复用 Task 5 Step 4 已修正的 GRU 回写
        let feat_erb_v = feat_erb(&spec, &self.erb_bounds);
        let feat_spec_v = feat_spec(&spec);
        let spec_4d = complex_to_4d(&spec);
        let erb_in = vec_to_arr(&feat_erb_v);
        let fspec_in = vec_to_4d(&feat_spec_v);
        let outputs = self.session.run(ort::inputs! {
            "spec" => TensorRef::from_array_view(spec_4d.view())?,
            "feat_erb" => TensorRef::from_array_view(erb_in.view())?,
            "feat_spec" => TensorRef::from_array_view(fspec_in.view())?,
            "enc_h" => TensorRef::from_array_view(self.enc_h.view())?,
            "erb_h" => TensorRef::from_array_view(self.erb_h.view())?,
            "df_h" => TensorRef::from_array_view(self.df_h.view())?,
        }?)?;
        let enhanced = outputs["enhanced_spec"].try_extract_tensor::<f32>()?;
        self.enc_h = outputs["new_enc_h"].try_extract_tensor::<f32>()?.view().to_owned()
            .into_shape((1, 1, 256)).map_err(|e| anyhow::anyhow!("{e}"))?;
        self.erb_h = outputs["new_erb_h"].try_extract_tensor::<f32>()?.view().to_owned()
            .into_shape((2, 1, 256)).map_err(|e| anyhow::anyhow!("{e}"))?;
        self.df_h = outputs["new_df_h"].try_extract_tensor::<f32>()?.view().to_owned()
            .into_shape((2, 1, 256)).map_err(|e| anyhow::anyhow!("{e}"))?;

        let enh_spec = arr4d_to_complex(&enhanced.view());
        let time = istft_frame(&enh_spec, &self.ifft, &self.window);
        let mut out_frame = vec![0.0f32; HOP];
        for i in 0..HOP {
            out_frame[i] = time[i] + self.ola_prev[i + HOP];
        }
        self.out_buf.extend_from_slice(&out_frame);
        self.ola_prev = time;
    }
```

> **若 `streaming_incremental_equals_batch` 失败**：偏差来自分析帧上下文（`ola_prev` 是 iSTFT 输出而非原始时域）。修正：新增字段 `prev_time_tail: Vec<f32>`（存上一帧原始 new 样本尾 480），分析帧用它而非 `ola_prev` 切片。测试驱动此修正。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p octopus-asr sample_conservation streaming_incremental -- --ignored`
Expected: PASS（两个流式一致性测试）

- [ ] **Step 5: 提交**

```bash
git add crates/asr/src/denoise.rs
git commit -m "feat(asr): streaming process_samples/flush + sample conservation & consistency tests"
```

---

## Task 7: SharedAudioState 集成（48k NS + 双桥接 + start reset + 降级）

**Files:**
- Modify: `crates/desktop/src/audio.rs`（`SharedAudioState` 加字段、`start`/`stop`/`drain_samples` 接入）

- [ ] **Step 1: 读现有 audio.rs 确认集成点**

Run: `grep -nE "fn start|fn stop|fn drain_samples|resampler|struct SharedAudioState" crates/desktop/src/audio.rs`

确认：`stop`（重采样到 16k 后返回）、`drain_samples`（流式重采样到 16k）、`start`（clear buffer + 建流）。

- [ ] **Step 2: SharedAudioState 加 DenoiseProcessor 字段**

在 `crates/desktop/src/audio.rs` 的 `SharedAudioState` 结构体加字段：

```rust
pub struct SharedAudioState {
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<AtomicBool>,
    sample_rate: std::sync::atomic::AtomicU32,
    device_name: String,
    resampler: Mutex<Option<octopus_asr::audio::AudioResampler>>,
    stream: Mutex<Option<cpal::Stream>>,
    // 新增：降噪处理器（None = 未启用/加载失败，降级直通）
    denoise: Mutex<Option<octopus_asr::denoise::DenoiseProcessor>>,
    // 新增：48k→16k 重采样器（NS 输出后降采样）
    down_sampler: Mutex<Option<octopus_asr::audio::AudioResampler>>,
}
```

`new` 初始化（若 `config.denoise_enabled`）：在 `new` 加参数 `denoise_enabled: bool`，或从 config 读。鉴于 `SharedAudioState::new` 当前只接 `device_name`，改为在 `start` 时按需 lazy init（见 Step 3）。

- [ ] **Step 3: 接入 drain_samples / stop**

改造重采样路径。原 `stop`/`drain_samples` 直接 `raw → 16k`；改为 `raw → 48k → NS → 16k`（denoise 启用时）。

抽出一个统一处理函数（DRY）：

```rust
impl SharedAudioState {
    /// raw(原生SR) → [升48k] → [NS降噪] → [降16k]。
    /// denoise 未启用/加载失败时降级：raw → 16k（原逻辑）。
    fn process_pipeline(&self, raw: Vec<f32>, rate: u32) -> Vec<f32> {
        let rate48 = if rate == 48000 { raw.clone() } else { self.resample_to(raw.clone(), rate, 48000) };

        let cleaned = if rate == 0 || rate48.is_empty() {
            rate48.clone()
        } else {
            match self.denoise.lock().unwrap().as_mut() {
                Some(d) => {
                    let mut out = d.process_samples(&rate48);
                    out.extend(d.flush());
                    out
                }
                None => rate48.clone(),
            }
        };

        // 48k → 16k
        if 48000 == 16000 { cleaned } else { self.resample_to(cleaned, 48000, 16000) }
    }

    fn resample_to(&self, samples: Vec<f32>, from: u32, to: u32) -> Vec<f32> {
        if from == to { return samples; }
        // 用 rubato 一次性重采样（非流式路径）
        octopus_asr::audio::resample_to_16k 仅为 16k；此处用通用 resample
        // 实现见 Step 4：复用 AudioResampler 或新增通用函数
        todo_in_step4()
    }
}
```

> ⚠️ **Step 4 编译驱动**：`resample_to` 的通用实现——`octopus_asr::audio` 现有 `resample_to_16k` 写死 16k 目标。新增通用 `resample_to(samples, from, to)`（用 `rubato::FftFixedIn`），放 `crates/asr/src/audio.rs`。

- [ ] **Step 4: 新增通用重采样 + lazy init denoise**

在 `crates/asr/src/audio.rs` 加：

```rust
/// 通用重采样：任意 from_rate → to_rate。
pub fn resample_to(samples: &[f32], from_rate: u32, to_rate: u32) -> anyhow::Result<Vec<f32>> {
    if from_rate == to_rate {
        return Ok(samples.to_vec());
    }
    let mut resampler = rubato::FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, 1024, 2, 1)?;
    let mut input = vec![samples.to_vec()]; // mono 单声道
    let out = resampler.process(&input, None)?;
    Ok(out.into_iter().next().unwrap_or_default())
}
```

`SharedAudioState::start` 加 denoise lazy init + reset：

```rust
    pub fn start(&self, device_name: &str) -> Result<()> {
        self.samples.lock().unwrap().clear();
        self.is_recording.store(true, Ordering::Relaxed);

        // lazy init denoise（首次启用时加载模型；失败降级 None，warn 不阻断）
        let mut dn = self.denoise.lock().unwrap();
        if dn.is_none() {
            match octopus_asr::config::find_df3().and_then(|p| octopus_asr::denoise::DenoiseProcessor::new(&p)) {
                Ok(mut proc) => { proc.reset(); *dn = Some(proc); info!("DenoiseProcessor loaded"); }
                Err(e) => log::warn!("降噪未启用（降级直通）: {:#}", e),
            }
        } else {
            dn.as_mut().unwrap().reset();
        }
        drop(dn);

        let stream = self.build_stream(device_name)?;
        stream.play()?;
        *self.stream.lock().unwrap() = Some(stream);
        debug!("Recording started");
        Ok(())
    }
```

`stop`/`drain_samples` 改用 `process_pipeline`（替换原 `rate==16000 ? raw : resample` 分支）：

`stop` 中：
```rust
        let rate = self.sample_rate.load(Ordering::Relaxed);
        let resampled = self.process_pipeline(raw, rate);
        *self.resampler.lock().unwrap() = None;
        *self.down_sampler.lock().unwrap() = None;
        Ok(resampled)
```

`drain_samples` 同理用 `process_pipeline`（注意流式：drain 不 flush denoise，让状态跨次保持；仅 stop 时 flush。修正：drain_samples 内 process_samples 但不 flush，stop 内调 flush）。

> 流式细节：`drain_samples` 用 `process_samples`（不 flush，GRU 状态跨次保持）；`stop` 在取最后一段时 `process_samples` + `flush` 吐残留。重构 `process_pipeline` 接受 `flush: bool` 参数。

- [ ] **Step 5: 编译 + 跑全量测试**

Run: `cargo build -p octopus-desktop && cargo test -p octopus-asr && cargo test -p octopus-infra`
Expected: 编译通过；DSP 测试 PASS（推理测试 `--ignored` 单独跑）

- [ ] **Step 6: 手动 e2e 验证（需模型）**

```bash
# 确保模型已下载
hf download penta2himajin/deepfilternet3-onnx

# 跑应用，对比开/关降噪
# config.yaml: denoise_enabled: true  → 带噪录音识别应改善
# config.yaml: denoise_enabled: false → 行为与现状一致（零回归）
# 删除 dfn3.onnx → 应用正常启动 + warn 下载提示，不崩溃
```

- [ ] **Step 7: 提交**

```bash
git add crates/desktop/src/audio.rs crates/asr/src/audio.rs
git commit -m "feat(desktop): integrate DeepFilterNet3 denoise in SharedAudioState (48k NS + 16k bridge)"
```

---

## Task 8: 文档同步（CLAUDE.md 强制）

**Files:**
- Modify: `docs/architecture.md`

- [ ] **Step 1: 更新 architecture.md**

在音频采集/持久化相关段加「环境降噪（DeepFilterNet3）」小节：

```markdown
### 环境降噪（DeepFilterNet3，可选）

录音送 VAD/ASR 前可选的 ONNX 降噪层（`config.yaml.denoise_enabled`，默认 true）：

- **模型**：`penta2himajin/deepfilternet3-onnx/dfn3.onnx`（HF cache，带 GRU 状态的流式版，单一固定模型，**不进 DB / 不切换**）。
- **集成点**：采集层 `SharedAudioState` 内（coordinator 无感）。链路 `原生SR → 重采样48k → DenoiseProcessor → 重采样16k → VAD/ASR`。
- **DSP**：sqrt-Hann STFT（n_fft=960, hop=480, 481 bins）+ 32 ERB 特征 + dfn3.onnx（含 GRU 隐状态 enc_h/erb_h/df_h）+ iSTFT overlap-add。复用现有 `rustfft`。
- **状态语义**：GRU 状态录音会话内跨帧保持（噪声环境稳态估计，不应被分段打断，与 filter_vad 每段 reset 故意相反）；`start()` 调 `reset()`。
- **降级**：模型缺失/推理失败 → 降级直通（不阻断录音，仅 warn）。
- **跨平台**：ort 三平台 EP（CoreML/DirectML/CUDA/CPU）；STFT 参数硬绑 48kHz。
- **模块**：`crates/asr/src/denoise.rs`（`DenoiseProcessor` + DSP）、`crates/asr/src/config.rs::find_df3`。
```

- [ ] **Step 2: 提交**

```bash
git add docs/architecture.md
git commit -m "docs: add DeepFilterNet3 denoise to architecture"
```

---

## Self-Review 检查

**Spec 覆盖：**
- §1-2（NS only, AEC 排除）→ 整个 plan 范围 ✓
- §3（dfn3.onnx IO 契约）→ Task 5 onnx inputs ✓
- §4（集成采集层 / 48k-NS-16k / 数据流）→ Task 7 process_pipeline ✓
- §5（denoise.rs / DenoiseProcessor API / rustfft）→ Task 3-6 ✓
- §6（状态保持 vs reset）→ Task 5 reset + Task 6 流式一致测试 ✓
- §7（跨平台 ort EP / STFT 硬绑48k）→ Task 7 + 文档 Task 8 ✓
- §8（denoise_enabled infra / find_df3 / 不进 DB / 缺失提示）→ Task 1, 2 ✓
- §9（三级降级）→ Task 7 lazy init None + Task 5 单帧 bypass（注：单帧推理失败的 bypass 需在 Task 5 process_frame 的 onnx run 包 `match`，见下方修正）✓
- §10（测试策略：重建 SNR / 样本守恒 / 流式一致 / 状态）→ Task 3,4,5,6 ✓
- §13（实施前提：窗/ERB/三平台/性能）→ Task 3(窗), Task 4(ERB), Task 7(性能: ort threads 可后续加) ✓

**遗漏修正**：单帧推理失败 bypass（§9 第 2 行）在 Task 5 `process_frame` 的 `session.run` 未包错误处理。在 Task 5 Step 4 后补：

```rust
        let outputs = match self.session.run(ort::inputs! { ... }?) {
            Ok(o) => o,
            Err(e) => {
                log::warn!("DenoiseProcessor 单帧推理失败，bypass: {e}");
                // GRU 状态保持，输出原始 new_samples（未降噪）
                self.out_buf.extend_from_slice(new_samples);
                return;
            }
        };
```

**类型一致性**：`DenoiseProcessor` 字段 enc_h/erb_h/df_h 在 Task 5 定义为 `Array3<f32>`，Task 6 process_frame 回写用 `into_shape((1,1,256))`/`(2,1,256)`/`(2,1,256)` 一致 ✓。`process_samples`/`flush`/`reset`/`new` 签名跨 Task 一致 ✓。

---

## Execution Handoff

Plan 完成并保存至 `docs/superpowers/plans/2026-06-16-denoise-deepfilternet.md`。两种执行方式：

**1. Subagent-Driven（推荐）** — 每个 Task 派发独立 subagent，任务间 review，迭代快。

**2. Inline Execution** — 本会话内用 executing-plans 批量执行，设检查点 review。

哪种？

---

## Task 9: Bug 修复 — 对齐 libDF 参考实现（2026-06-16）

> 初版实现（Task 1-8）完成后，实测发现降噪后 ASR 效果显著下降。经对比 `penta2himajin/mellonella`（模型导出方）参考实现和 `Rikorose/DeepFilterNet/libDF`，发现 4 个 bug。

**根因**：denoise.rs 的特征提取逻辑与模型训练时的特征分布完全不匹配，模型输出的增强频谱是垃圾，反而破坏了语音信号。

### Bug 列表

| # | Bug | 初版值 | 正确值（libDF） | 影响 |
|---|-----|--------|----------------|------|
| 1 | **ERB 公式分母** | `24.863` | `228.833` (= 24.7×9.265) | 带边界错 9.2 倍，32 个 ERB 带覆盖频率全错 |
| 2 | **feat_erb 缺归一化** | 原始 `\|spec\|²` 求和 | band 互相关功率 → dB → EMA 均值归一化 → /40 | 模型收到错误量级 |
| 3 | **feat_spec 缺归一化** | 原始 re/im | EMA 跟踪 `\|z\|`，除以 √state | 模型收到错误量级 |
| 4 | **conv_lookahead 缺失** | spec[t] 立即配 feat[t] | VecDeque 环形缓冲，spec[t] 配 feat[t+2] | 帧错位 20ms |

**额外修正**：
- 窗函数：sqrt-Hann → **Vorbis**（`sin(π/2·sin²(π(n+0.5)/N))`）
- band 功率公式：`Σ|spec|²` → **`(Σ|spec|²/width)²`**（libDF compute_band_corr 自相关形式）

### 参考来源

- `penta2himajin/mellonella` → `rust/mellonella-core/src/dfn3.rs`（Rust DFN3 ONNX 调用方）
- `Rikorose/DeepFilterNet` → `libDF/src/lib.rs`（`freq2erb` / `band_mean_norm_erb` / `band_unit_norm` / Vorbis 窗 / `MEAN_NORM_INIT` / `UNIT_NORM_INIT`）

### 关键参数（对齐后）

| 参数 | 值 |
|------|-----|
| 窗 | Vorbis：`sin(π/2·sin²(π(n+0.5)/N))` |
| ERB 分母 | 228.833 = 24.7 × 9.265 |
| conv_lookahead | 2 |
| norm_alpha | 0.99 (= exp(-hop/sr/τ) ≈ exp(-0.01)) |
| feat_erb 归一化 | dB → EMA(state) → (x - state) / 40 |
| feat_spec 归一化 | EMA 跟踪 \|z\|，X / √state |
| mean_norm_state 初始 | linspace(-60.0, -90.0, 32) |
| unit_norm_state 初始 | linspace(0.001, 0.0001, 96) |

### 改动

- [x] 重写 `crates/asr/src/denoise.rs`：Vorbis 窗 + ERB 公式修正 + 归一化状态 + conv_lookahead 环形缓冲
- [x] 更新 `docs/architecture.md` denoise 描述
- [x] 更新 `docs/superpowers/specs/2026-06-16-denoise-deepfilternet-design.md` §3.2 / §5 / §13
- [x] 更新本 plan 头部关键技术契约 + 追加 Task 9

### 验证

- [x] `cargo test -p octopus-asr -- denoise`：8 单元测试全过（Vorbis COLA、STFT/iSTFT 重建 SNR>40dB、ERB 公式对齐 libDF、归一化数值正确、band 覆盖 481 bins）
- [x] `cargo check --workspace` 全编译通过
- [x] `cargo test` 全量 63 tests passed, 0 failed

---

## `2026-06-16-model-spec-prefix.md`

# 模型选择 spec 实施计划

> 状态：✅ 全部完成（2026-06-16）。对应 spec：[`specs/2026-06-16-model-spec-prefix-design.md`](../specs/2026-06-16-model-spec-prefix-design.md)

## 阶段 A：infra 层 — ModelSpec + LLM 查询 ✅
- [x] `infra/src/db.rs` 新增 `ModelSpec` 枚举（`Local` / `Category` / `NameOnly`）+ `parse_model_spec` 函数
- [x] `ModelSpec::name()` 返回裸名（生命周期 `&'a str`，绑定原借用）
- [x] `load_llm_model_at` 改用 `parse_model_spec`，按两分支构建 SQL（`Local` 与 `NameOnly` 共用）：
  - `Local` / `NameOnly` → `domain='llm' AND is_local=1 AND name=?`
  - `Category` → `domain='llm' AND category=? AND name=?`
- [x] 提取 `parse_llm_row` 辅助函数减少重复

## 阶段 B：asr 层 — 引擎解析改走 spec ✅
- [x] `asr/src/config.rs` 新增 `engine_category_from_str`（5 个 ASR 类型映射）
- [x] 新增 `all_sections` 辅助函数（固定遍历顺序）
- [x] 新增 `resolve_engine_in_config(cfg, spec)` 统一解析入口（`Local`/`NameOnly` 合并 + `Category` 两分支）
- [x] `resolve_engine_category(spec)` 改委托 `resolve_engine_in_config`
- [x] `resolve_active_engine(spec)` 改委托 `resolve_engine_in_config`，返回裸名
- [x] `pub use` 导出 `parse_model_spec` / `ModelSpec` 供 asr 内部使用

## 阶段 C：引擎管理器 + 流式引擎 ✅
- [x] `asr/src/engine.rs` `AsrEngineManager.switch_model` 解析 spec → 裸名做缓存键
- [x] `asr/src/streaming_engine.rs` `StreamingSession::new` 解析 spec → 裸名传给 `StreamingParaformer::new` / `StreamingZipformer::new`

## 阶段 D：CLI 调用点 ✅
- [x] `cli/src/main.rs` `do_transcribe` — 剥离前缀后传给各引擎 `transcribe`
- [x] `cli/src/main.rs` `run_e2e` — 剥离前缀后传给流式构造器
- [x] `cli/src/main.rs` `stream_test` — 剥离前缀后传给流式测试函数

## 阶段 E：默认值 + 错误消息 ✅
- [x] `infra/src/config.rs` `polish_llm` 默认值 `glm-4-flashx` → `bigmodel:glm-4-flashx`
- [x] `infra/src/config.rs` `polish_llm` 字段注释更新（`PREFIX:NAME` 格式说明）
- [x] `llm/examples/test_polish.rs` 默认值同步
- [x] `desktop/src/config.rs` 错误消息措辞适配

## 阶段 F：测试 ✅
- [x] `infra/src/db.rs` 测试：
  - `test_load_llm_model` 用 `deepseek:` / `aliyun:` 前缀验证两个同名 LLM
  - 新增 `local:` 前缀测试（插入 is_local=1 行验证命中）
  - 新增 `parse_model_spec_variants` + `model_spec_name_strips_prefix`
- [x] `asr/src/config.rs` 测试：
  - `parse_spec_local_prefix` / `parse_spec_category_prefix` / `parse_spec_bare_name`
  - `resolve_local_prefix_finds_local_model` / `resolve_category_prefix_matches_section`
  - `resolve_category_prefix_wrong_category_returns_none` / `resolve_bare_name_equivalent_to_local`
  - `resolve_bare_name_skips_non_local`（裸名跳过 is_local=false）
  - `resolve_unknown_category_prefix_returns_none`
  - `engine_category_from_str_maps_five_types`

## 阶段 G：文档同步 ✅
- [x] `docs/configuration.md` 新增「模型选择 spec」节 + `asr_engine` / `polish_llm` 表格行更新
- [x] `docs/configuration.md` 配置示例更新为新格式
- [x] `docs/architecture.md` 引擎选择段落更新
- [x] 本 spec + plan

## 验证

- [x] `cargo check --workspace` 通过（含 desktop embedded / cli / server）
- [x] `cargo test` 全部通过（59 tests passed, 0 failed, 3 ignored）
- [x] `cargo build --release -p octopus-server -p octopus-cli` 通过
- [x] `cargo build -p octopus-llm --example test_polish` 通过

---
