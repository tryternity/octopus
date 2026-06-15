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

- [x] **Step 2: Toggle 命令分发处——仅 Idle 时同步 asr_engine + polish_mode + 重算 use_streaming**

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
