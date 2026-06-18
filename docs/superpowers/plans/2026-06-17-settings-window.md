# 设置窗口实施计划（Settings Window）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建独立 Tauri 窗口，提供 GUI 设置界面替代手编 config.yaml，含识别记录浏览、系统设置、模型管理占位三个页面。

**Architecture:** 新建 `settings_window.rs`（窗口创建）+ 扩展 `runtime_config.rs`（`get_config` / `set_config` / `get_history` 通用命令）+ 新建 `dist/settings/index.html`（vanilla HTML 三页面）。入口为工具栏设置按钮 + 托盘菜单"设置..."项，实时保存写 config.yaml + RuntimeConfig。

**Tech Stack:** Rust + Tauri 2 + vanilla HTML/CSS/JS（无构建步骤）+ serde_json + rusqlite

**设计 spec:** `docs/superpowers/specs/2026-06-17-settings-window-design.md`

---

## 文件结构

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/src/settings_window.rs` | 设置窗口创建 + `open_settings` 命令 + 单例管理 | **新建** |
| `crates/desktop/src/settings_commands.rs` | `get_config` / `set_config` / `get_history` 命令 + 类型校验逻辑 | **新建**（独立文件避免 `runtime_config.rs` 膨胀） |
| `crates/desktop/src/runtime_config.rs` | RuntimeConfig 新增 `asr_correct` / `output_simplified` / `hide_toolbar` 字段 | **修改** |
| `crates/desktop/src/tray.rs` | 托盘菜单新增"设置..."项 | **修改** |
| `crates/desktop/src/main.rs` | 注册新命令 + 设置窗口模块声明 | **修改** |
| `crates/desktop/src/mod.rs` (或 `lib.rs`) | 模块声明 `settings_window` + `settings_commands` | **修改**（如存在） |
| `crates/infra/src/db.rs` | 新增 `list_transcriptions(limit, offset)` 查询函数 + DTO | **修改** |
| `crates/desktop/dist/settings/index.html` | 三页面 vanilla HTML | **新建** |

---

### Task 1: DB 历史查询函数

**Files:**
- Modify: `crates/infra/src/db.rs`
- Test: `crates/infra/src/db.rs`（内联 `#[cfg(test)]` 模块）

- [ ] **Step 1: 在 `db.rs` 新增 `TranscriptionRecord` DTO 和 `list_transcriptions` 查询函数**

在 `crates/infra/src/db.rs` 的 `finalize_transcription` 函数之后（约 line 392），添加：

```rust
/// 历史识别记录（设置窗口识别记录页用）。
#[derive(Debug, serde::Serialize)]
pub struct TranscriptionRecord {
    pub id: i64,
    pub created_at: String,
    pub engine: String,
    pub raw_text: String,
    pub polished_text: Option<String>,
    pub polish_status: String,
    pub duration_ms: Option<i64>,
}

/// 分页查询历史识别记录（按 id 降序 = 最新在前）。
pub fn list_transcriptions(limit: u32, offset: u32) -> Result<Vec<TranscriptionRecord>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, created_at, engine, raw_text, polished_text, polish_status, duration_ms
             FROM transcriptions ORDER BY id DESC LIMIT ?1 OFFSET ?2"
        )?;
        let rows = stmt.query_map(params![limit, offset], |row| {
            Ok(TranscriptionRecord {
                id: row.get(0)?,
                created_at: row.get(1)?,
                engine: row.get(2)?,
                raw_text: row.get(3)?,
                polished_text: row.get(4)?,
                polish_status: row.get(5)?,
                duration_ms: row.get(6)?,
            })
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    })
}
```

- [ ] **Step 2: 编译验证**

Run: `cargo check -p octopus-infra`
Expected: 编译通过（`serde::Serialize` 需确认 infra crate 已依赖 serde — 检查 `ModelEntry` 等 DTO 已有 `#[derive(Serialize)]` 确认）。

- [ ] **Step 3: 在 db.rs 内联测试模块新增测试**

在 db.rs 的 `#[cfg(test)]` 模块末尾添加（需确认测试模块位置——在 `days_to_ymd` 测试之后）：

```rust
    #[test]
    fn list_transcriptions_returns_records_descending() {
        let conn = create_test_db().unwrap();
        // 插入两条记录
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (100, '2026-06-17 10:00:00', 'whisper', '你好', 'off')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polished_text, polish_status)
             VALUES (200, '2026-06-17 11:00:00', 'qwen3', '你好世界', '你好，世界。', 'done')",
            [],
        ).unwrap();
        // 查询全部
        let rows = list_transcriptions_at(&conn, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, 200, "最新在前（id 降序）");
        assert_eq!(rows[1].id, 100);
        assert_eq!(rows[0].raw_text, "你好世界");
        assert_eq!(rows[0].polished_text.as_deref(), Some("你好，世界。"));
        // 分页：第一页只取 1 条
        let page1 = list_transcriptions_at(&conn, 1, 0).unwrap();
        assert_eq!(page1.len(), 1);
        assert_eq!(page1[0].id, 200);
        // 第二页
        let page2 = list_transcriptions_at(&conn, 1, 1).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].id, 100);
        // 越界：空
        let page3 = list_transcriptions_at(&conn, 10, 2).unwrap();
        assert!(page3.is_empty());
    }
```

注意：测试用 `list_transcriptions_at(&conn, ...)`（直接传 Connection 的版本），需要把 `list_transcriptions` 的核心逻辑拆出一个 `_at` 版本（与现有 `load_models` / `load_models_at` 模式一致）。

- [ ] **Step 4: 重构 `list_transcriptions` 拆出 `_at` 版本**

```rust
/// 分页查询历史识别记录（按 id 降序 = 最新在前）。
pub fn list_transcriptions(limit: u32, offset: u32) -> Result<Vec<TranscriptionRecord>> {
    with_db(|conn| list_transcriptions_at(conn, limit, offset))
}

fn list_transcriptions_at(
    conn: &rusqlite::Connection,
    limit: u32,
    offset: u32,
) -> Result<Vec<TranscriptionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at, engine, raw_text, polished_text, polish_status, duration_ms
         FROM transcriptions ORDER BY id DESC LIMIT ?1 OFFSET ?2"
    )?;
    let rows = stmt.query_map(params![limit, offset], |row| {
        Ok(TranscriptionRecord {
            id: row.get(0)?,
            created_at: row.get(1)?,
            engine: row.get(2)?,
            raw_text: row.get(3)?,
            polished_text: row.get(4)?,
            polish_status: row.get(5)?,
            duration_ms: row.get(6)?,
        })
    })?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}
```

- [ ] **Step 5: 运行测试验证通过**

Run: `cargo test -p octopus-infra`
Expected: 全部通过（含新增 `list_transcriptions_returns_records_descending`）。

- [ ] **Step 6: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(infra): list_transcriptions 分页查询历史识别记录"
```

---

### Task 2: RuntimeConfig 扩展新增字段

**Files:**
- Modify: `crates/desktop/src/runtime_config.rs:13-28`（`RuntimeConfig` struct + `from_config`）

- [ ] **Step 1: 扩展 `RuntimeConfig` struct**

在 `crates/desktop/src/runtime_config.rs` line 13 的 `RuntimeConfig` struct 新增 3 个字段：

```rust
pub struct RuntimeConfig {
    pub asr_engine: String,
    pub polish_mode: PolishMode,
    pub polish_llm: String,
    pub denoise_mode: u8,
    pub asr_correct: bool,
    pub output_simplified: bool,
    pub hide_toolbar: bool,
}
```

- [ ] **Step 2: 扩展 `from_config`**

```rust
impl RuntimeConfig {
    pub fn from_config(cfg: &octopus_infra::config::AppConfig) -> Self {
        Self {
            asr_engine: cfg.asr_engine.clone(),
            polish_mode: cfg.polish_mode,
            polish_llm: cfg.polish_llm.clone(),
            denoise_mode: cfg.denoise_mode,
            asr_correct: cfg.asr_correct,
            output_simplified: cfg.output_simplified,
            hide_toolbar: cfg.hide_toolbar,
        }
    }
}
```

- [ ] **Step 3: 更新 `from_config_mirrors_fields` 测试**

在 `runtime_config.rs` 测试模块中扩展 `from_config_mirrors_fields`：

```rust
    #[test]
    fn from_config_mirrors_fields() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        cfg.asr_engine = "qwen3-asr-0.6B".into();
        cfg.polish_mode = PolishMode::Intermediate;
        cfg.asr_correct = true;
        cfg.output_simplified = false;
        cfg.hide_toolbar = false;
        let rc = RuntimeConfig::from_config(&cfg);
        assert_eq!(rc.asr_engine, "qwen3-asr-0.6B");
        assert_eq!(rc.polish_mode, PolishMode::Intermediate);
        assert!(rc.asr_correct);
        assert!(!rc.output_simplified);
        assert!(!rc.hide_toolbar);
    }
```

- [ ] **Step 4: 编译 + 测试**

Run: `cargo check -p octopus-desktop --features embedded && cargo test -p octopus-desktop --features embedded`
Expected: 编译通过，16+ 测试通过（`from_config_mirrors_fields` 更新）。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/runtime_config.rs
git commit -m "feat(desktop): RuntimeConfig 新增 asr_correct/output_simplified/hide_toolbar"
```

---

### Task 3: `set_config` 通用写命令 — 类型校验逻辑

**Files:**
- Create: `crates/desktop/src/settings_commands.rs`
- Modify: `crates/desktop/src/main.rs`（模块声明，在 Task 6 统一注册命令时改）

- [ ] **Step 1: 创建 `settings_commands.rs`，实现 `set_config` 命令 + 类型校验**

```rust
//! 设置窗口的 Tauri 命令：get_config / set_config / get_history。
//!
//! 与 runtime_config.rs 的区别：后者是工具栏专用命令（每个字段一个命令），
//! 本模块提供通用 get/set（方案 A），供设置窗口 GUI 表单使用。

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::runtime_config::SharedRuntimeConfig;
use crate::config::PolishMode;

// ── get_config 返回 DTO ──

#[derive(Serialize)]
pub struct ConfigResponse {
    pub config: Value,
    pub asr_engines: Vec<crate::runtime_config::EngineOption>,
    pub llm_models: Vec<crate::runtime_config::LlmOption>,
    pub microphones: Vec<String>,
}

// ── get_config 命令 ──

#[tauri::command]
pub fn get_config(rc: State<'_, SharedRuntimeConfig>) -> Result<ConfigResponse, String> {
    let cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;
    let config_json = serde_json::to_value(&cfg).map_err(|e| e.to_string())?;

    // ASR 引擎列表（复用 runtime_config 的逻辑）
    let g = rc.read().unwrap();
    let engines = octopus_asr::config::list_engines().map_err(|e| e.to_string())?;
    let asr_engines = crate::runtime_config::build_asr_options_public(&g.asr_engine, engines);

    // LLM 模型列表
    let llms = octopus_infra::db::list_llm_models().map_err(|e| e.to_string())?;
    let llm_models = crate::runtime_config::build_llm_options_public(&g.polish_llm, llms);

    // 麦克风设备列表
    let microphones = list_microphones();

    Ok(ConfigResponse {
        config: config_json,
        asr_engines,
        llm_models,
        microphones,
    })
}

/// 枚举系统麦克风设备（cpal 跨平台）。
fn list_microphones() -> Vec<String> {
    let host = match cpal::default_host() {
        h => h,
    };
    match host.input_devices() {
        Ok(devices) => {
            devices
                .filter_map(|d| d.name().ok())
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

// ── set_config 命令 ──

#[tauri::command]
pub fn set_config(
    key: String,
    value: Value,
    rc: State<'_, SharedRuntimeConfig>,
) -> Result<(), String> {
    // 读当前 config.yaml（而非 OnceLock 缓存——确保写回时保留所有字段）
    let mut cfg = octopus_infra::config::load_config().map_err(|e| e.to_string())?;

    // 按字段校验 + 赋值
    apply_config_value(&mut cfg, &key, &value)?;

    // 写 RuntimeConfig（如字段属于运行时镜像）
    sync_runtime_config(&rc, &key, &cfg);

    // 持久化 config.yaml
    write_config_yaml(&cfg)?;

    Ok(())
}

/// 按字段名校验类型/范围并赋值到 AppConfig。非法值返回 Err。
fn apply_config_value(
    cfg: &mut octopus_infra::config::AppConfig,
    key: &str,
    value: &Value,
) -> Result<(), String> {
    match key {
        // ── 字符串枚举 ──
        "language" => {
            let v = value.as_str().ok_or("language 需要字符串")?;
            if !["auto", "zh", "en", "ja", "ko"].contains(&v) {
                return Err(format!("language 非法值 '{}'（应为 auto/zh/en/ja/ko）", v));
            }
            cfg.language = v.to_string();
        }
        "engine_mode" => {
            let v = value.as_str().ok_or("engine_mode 需要字符串")?;
            if !["embedded", "websocket", "grpc"].contains(&v) {
                return Err(format!("engine_mode 非法值 '{}'（应为 embedded/websocket/grpc）", v));
            }
            cfg.engine_mode = v.to_string();
        }
        // ── u8 枚举 ──
        "polish_mode" => {
            let v = value.as_u64().ok_or("polish_mode 需要 0/1/2")? as u8;
            cfg.polish_mode = match v {
                0 => PolishMode::Disabled,
                1 => PolishMode::FinalOnly,
                2 => PolishMode::Intermediate,
                _ => return Err(format!("polish_mode={} 非法（应为 0/1/2）", v)),
            };
        }
        "denoise_mode" => {
            let v = value.as_u64().ok_or("denoise_mode 需要 0/1/2")? as u8;
            if v > 2 {
                return Err(format!("denoise_mode={} 非法（应为 0/1/2）", v));
            }
            cfg.denoise_mode = v;
        }
        // ── bool ──
        "asr_hardware_accelerated" => {
            cfg.asr_hardware_accelerated = value.as_bool().ok_or("asr_hardware_accelerated 需要 bool")?;
        }
        "asr_correct" => {
            cfg.asr_correct = value.as_bool().ok_or("asr_correct 需要 bool")?;
        }
        "output_simplified" => {
            cfg.output_simplified = value.as_bool().ok_or("output_simplified 需要 bool")?;
        }
        "hide_toolbar" => {
            cfg.hide_toolbar = value.as_bool().ok_or("hide_toolbar 需要 bool")?;
        }
        // ── f64 正数 ──
        "segment_duration" => {
            let v = value.as_f64().ok_or("segment_duration 需要数值")?;
            if v <= 0.0 { return Err("segment_duration 必须大于 0".into()); }
            cfg.segment_duration = v;
        }
        "segment_silence" => {
            let v = value.as_f64().ok_or("segment_silence 需要数值")?;
            if v <= 0.0 { return Err("segment_silence 必须大于 0".into()); }
            cfg.segment_silence = v;
        }
        "segment_overlap" => {
            let v = value.as_f64().ok_or("segment_overlap 需要数值")?;
            if v < 0.0 { return Err("segment_overlap 不能为负".into()); }
            cfg.segment_overlap = v;
        }
        "polish_interval" => {
            let v = value.as_f64().ok_or("polish_interval 需要数值")?;
            if v < 0.0 { return Err("polish_interval 不能为负".into()); }
            cfg.polish_interval = v;
        }
        "pause_polish_threshold_ms" => {
            let v = value.as_f64().ok_or("pause_polish_threshold_ms 需要数值")?;
            if v <= 500.0 {
                return Err("pause_polish_threshold_ms 必须 > 500（Active Flush 阈值）".into());
            }
            cfg.pause_polish_threshold_ms = v;
        }
        // ── string（自由）──
        "shortcut" => {
            cfg.shortcut = value.as_str().ok_or("shortcut 需要字符串")?.to_string();
        }
        "microphone" => {
            cfg.microphone = value.as_str().ok_or("microphone 需要字符串")?.to_string();
        }
        "asr_engine" => {
            let bare_name = value.as_str().ok_or("asr_engine 需要字符串")?;
            // 前端传裸 model_name，需构造 3-part spec
            cfg.asr_engine = build_asr_engine_spec(bare_name)?;
        }
        "polish_llm" => {
            let bare_name = value.as_str().ok_or("polish_llm 需要字符串")?;
            // 前端传裸 model_name，空串=不选择模型，其余构造 3-part spec
            cfg.polish_llm = build_polish_llm_spec(bare_name)?;
        }
        _ => return Err(format!("未知配置字段: {}", key)),
    }
    Ok(())
}

/// 字段属于 RuntimeConfig 镜像范围的，同步更新。
fn sync_runtime_config(
    rc: &SharedRuntimeConfig,
    key: &str,
    cfg: &octopus_infra::config::AppConfig,
) {
    let mut g = rc.write().unwrap();
    match key {
        "asr_engine" => g.asr_engine = cfg.asr_engine.clone(),
        "polish_mode" => g.polish_mode = cfg.polish_mode,
        "polish_llm" => g.polish_llm = cfg.polish_llm.clone(),
        "denoise_mode" => g.denoise_mode = cfg.denoise_mode,
        "asr_correct" => g.asr_correct = cfg.asr_correct,
        "output_simplified" => g.output_simplified = cfg.output_simplified,
        "hide_toolbar" => g.hide_toolbar = cfg.hide_toolbar,
        _ => {}
    }
}

fn write_config_yaml(cfg: &octopus_infra::config::AppConfig) -> Result<(), String> {
    let path = octopus_infra::octopus_config_home().join("config.yaml");
    let text = serde_yaml::to_string(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

// ── get_history 命令 ──

#[tauri::command]
pub fn get_history(limit: u32, offset: u32) -> Result<Vec<octopus_infra::db::TranscriptionRecord>, String> {
    octopus_infra::db::list_transcriptions(limit, offset).map_err(|e| e.to_string())
}

// ── 单测（纯逻辑校验，不触文件 IO / Tauri State）──

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_config_valid_bool() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "asr_correct", &json!(true)).unwrap();
        assert!(cfg.asr_correct);
        apply_config_value(&mut cfg, "asr_correct", &json!(false)).unwrap();
        assert!(!cfg.asr_correct);
    }

    #[test]
    fn apply_config_invalid_bool() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "asr_correct", &json!("yes")).is_err());
    }

    #[test]
    fn apply_config_valid_f64() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "segment_duration", &json!(10.0)).unwrap();
        assert_eq!(cfg.segment_duration, 10.0);
    }

    #[test]
    fn apply_config_invalid_f64_zero() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "segment_duration", &json!(0.0)).is_err());
        assert!(apply_config_value(&mut cfg, "segment_duration", &json!(-1.0)).is_err());
    }

    #[test]
    fn apply_config_pause_polish_threshold_must_ge_500() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(499.0)).is_err());
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(500.0)).is_ok());
        assert!(apply_config_value(&mut cfg, "pause_polish_threshold_ms", &json!(600.0)).is_ok());
    }

    #[test]
    fn apply_config_valid_polish_mode() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        for n in 0..=2u8 {
            apply_config_value(&mut cfg, "polish_mode", &json!(n)).unwrap();
        }
        assert!(apply_config_value(&mut cfg, "polish_mode", &json!(3)).is_err());
    }

    #[test]
    fn apply_config_valid_language() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "language", &json!("zh")).unwrap();
        assert_eq!(cfg.language, "zh");
        assert!(apply_config_value(&mut cfg, "language", &json!("fr")).is_err());
    }

    #[test]
    fn apply_config_unknown_key() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        assert!(apply_config_value(&mut cfg, "nonexistent_field", &json!(1)).is_err());
    }

    #[test]
    fn apply_config_string_fields() {
        let mut cfg = octopus_infra::config::AppConfig::default();
        apply_config_value(&mut cfg, "shortcut", &json!("Ctrl+Alt+Z")).unwrap();
        assert_eq!(cfg.shortcut, "Ctrl+Alt+Z");
        apply_config_value(&mut cfg, "microphone", &json!("External Mic")).unwrap();
        assert_eq!(cfg.microphone, "External Mic");
    }
}
```

- [ ] **Step 2: 在 `runtime_config.rs` 暴露 `build_asr_options` / `build_llm_options` 的公开包装**

`settings_commands.rs` 需要调用 `build_asr_options` 和 `build_llm_options`，但它们目前是私有的。在 `runtime_config.rs` 添加公开包装函数：

```rust
/// 公开包装（供 settings_commands 调用）。
pub fn build_asr_options_public(
    current_effective: &str,
    engines: Vec<octopus_asr::config::EngineInfo>,
) -> Vec<EngineOption> {
    build_asr_options(current_effective, engines)
}

pub fn build_llm_options_public(
    current: &str,
    llms: Vec<octopus_infra::db::LlmModelInfo>,
) -> Vec<LlmOption> {
    build_llm_options(current, llms)
}
```

- [ ] **Step 3: 在 `main.rs` 添加模块声明**

在 `main.rs` 的模块声明区域（`mod runtime_config;` 附近）添加：

```rust
mod settings_commands;
mod settings_window;
```

（`settings_window` 在 Task 4 创建，先声明不影响编译——如编译报错可先注释 `settings_window` 行。）

- [ ] **Step 4: 检查 `Cargo.toml` 是否已有 `cpal` 依赖**

Run: `grep cpal crates/desktop/Cargo.toml`
Expected: 已有（audio.rs 使用）。如无，添加 `cpal = { workspace = true }`（但应已有）。

- [ ] **Step 5: 编译 + 测试**

Run: `cargo check -p octopus-desktop --features embedded && cargo test -p octopus-desktop --features embedded`
Expected: 编译通过，新增 8 个测试通过。

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/src/settings_commands.rs crates/desktop/src/runtime_config.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): set_config/get_config/get_history 通用命令 + 类型校验"
```

---

### Task 4: 设置窗口创建（`settings_window.rs`）

**Files:**
- Create: `crates/desktop/src/settings_window.rs`

- [ ] **Step 1: 创建 `settings_window.rs`**

```rust
//! 设置窗口：独立 Tauri 窗口，原生标题栏，800×600 可调大小。
//!
//! 单例管理：已打开则 set_focus，不重复创建。
//! 参考 result_window.rs 但更简单——设置窗无需 ready/pending 机制，
//! 前端加载后主动 invoke('get_config') 拉数据。

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const SETTINGS_WIDTH: f64 = 800.0;
const SETTINGS_HEIGHT: f64 = 600.0;
const MIN_WIDTH: f64 = 640.0;
const MIN_HEIGHT: f64 = 480.0;
const WINDOW_LABEL: &str = "settings_window";

/// 打开设置窗口（单例：已存在则 set_focus）。
#[tauri::command]
pub fn open_settings(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        WebviewUrl::App("settings/index.html".into()),
    )
    .title("Octopus 设置")
    .inner_size(SETTINGS_WIDTH, SETTINGS_HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .visible(true)
    .build();
}
```

- [ ] **Step 2: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 编译通过。

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/src/settings_window.rs
git commit -m "feat(desktop): settings_window 模块 — 窗口创建 + open_settings 命令"
```

---

### Task 5: 工具栏设置按钮接通 + 托盘菜单"设置..."

**Files:**
- Modify: `crates/desktop/src/main.rs`（注册命令）
- Modify: `crates/desktop/src/tray.rs`（托盘菜单加项）
- Modify: `crates/desktop/dist/result/index.html`（工具栏设置按钮 invoke）

- [ ] **Step 1: 在 `main.rs` 注册新命令**

在 `main.rs` 的 `invoke_handler` 中（line 152 附近），添加 4 个新命令：

```rust
        .invoke_handler(tauri::generate_handler![
            runtime_config::toolbar_state,
            runtime_config::list_asr_engines,
            runtime_config::switch_asr_engine,
            runtime_config::set_polish_mode,
            runtime_config::list_llm_models,
            runtime_config::switch_polish_llm,
            runtime_config::set_denoise_mode,
            coordinator::cancel_recording,
            coordinator::polish_now,
            result_window::result_window_ready,
            // 设置窗口命令
            settings_window::open_settings,
            settings_commands::get_config,
            settings_commands::set_config,
            settings_commands::get_history,
        ])
```

- [ ] **Step 2: 在 `tray.rs` 托盘菜单添加"设置..."项**

在 `create_tray` 函数中，`quit` 菜单项之前添加 `settings` 菜单项：

```rust
    let settings = MenuItem::with_id(app, "settings", "设置...", true, None::<&str>)
        .expect("failed to create settings menu item");
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .expect("failed to create quit menu item");

    let menu = Menu::with_items(app, &[&toggle, &engine_info, &settings, &quit])
        .expect("failed to create tray menu");
```

在 `on_menu_event` 闭包中，`"quit"` 之前添加：

```rust
            "settings" => {
                info!("Tray: open settings");
                crate::settings_window::open_settings(app.clone());
            }
```

注意：`open_settings` 的签名是 `#[tauri::command] pub fn open_settings(app_handle: tauri::AppHandle)`，直接调用时传 `app.clone()`。

- [ ] **Step 3: 修改工具栏设置按钮点击事件**

在 `crates/desktop/dist/result/index.html` 中找到设置按钮的点击处理（当前是占位，无动作），改为：

找到工具栏设置按钮的 `addEventListener` 或其 `onclick`（如果没有，在 JS 初始化区域添加）：

```javascript
    document.getElementById('tool-settings').addEventListener('click', async () => {
      try { await invoke('open_settings'); }
      catch (e) { showToast('打开设置失败：' + e); }
    });
```

- [ ] **Step 4: 编译验证**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 编译通过。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/main.rs crates/desktop/src/tray.rs crates/desktop/dist/result/index.html
git commit -m "feat(desktop): 工具栏设置按钮 + 托盘菜单接通 open_settings"
```

---

### Task 6: 前端设置页面骨架（侧边栏 + 3 页面切换）

**Files:**
- Create: `crates/desktop/dist/settings/index.html`

这是最大的单步。先创建包含完整 CSS + JS 骨架 + 侧边栏导航 + 3 页面容器的 HTML，页面内容（设置表单 / 历史列表）在后续 Task 填充。

- [ ] **Step 1: 创建 `dist/settings/index.html` 骨架**

```html
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Octopus 设置</title>
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }
:root {
  --sidebar-bg: #f5f5f7;
  --content-bg: #ffffff;
  --primary: #007aff;
  --text-primary: #1d1d1f;
  --text-secondary: #86868b;
  --border: #e5e5e7;
  --card-bg: #ffffff;
  --toggle-on: #34c759;
  --toggle-off: #e5e5e7;
  --radius: 8px;
}
body {
  font-family: -apple-system, "Segoe UI", "Noto Sans", sans-serif;
  color: var(--text-primary);
  display: flex;
  height: 100vh;
  overflow: hidden;
}
/* ── 侧边栏 ── */
#sidebar {
  width: 180px;
  background: var(--sidebar-bg);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
}
#sidebar .logo {
  padding: 20px 16px 12px;
  font-size: 18px;
  font-weight: 700;
}
#sidebar nav { flex: 1; padding: 8px 0; }
#sidebar nav .nav-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 16px;
  cursor: pointer;
  color: var(--text-primary);
  transition: background 0.15s;
  font-size: 14px;
}
#sidebar nav .nav-item:hover { background: rgba(0,0,0,0.05); }
#sidebar nav .nav-item.active { color: var(--primary); background: rgba(0,122,255,0.08); }
#sidebar nav .nav-item .icon { width: 18px; height: 18px; background: currentColor;
  -webkit-mask-size: contain; mask-size: contain; -webkit-mask-repeat: no-repeat; mask-repeat: no-repeat;
  -webkit-mask-position: center; mask-position: center; flex-shrink: 0; }
/* ── 主内容区 ── */
#content { flex: 1; overflow-y: auto; background: var(--content-bg); }
.page { display: none; padding: 24px; }
.page.active { display: block; }
/* ── 卡片 ── */
.card { background: var(--card-bg); border: 1px solid var(--border); border-radius: var(--radius); padding: 16px; margin-bottom: 16px; }
.card h3 { font-size: 14px; font-weight: 600; margin-bottom: 12px; color: var(--text-primary); }
.card .row { display: flex; align-items: center; justify-content: space-between; padding: 8px 0; border-bottom: 1px solid var(--border); }
.card .row:last-child { border-bottom: none; }
.card .row .label-group { display: flex; flex-direction: column; gap: 2px; }
.card .row .label-text { font-size: 14px; }
.card .row .label-hint { font-size: 12px; color: var(--text-secondary); }
.card .row .badge { font-size: 11px; color: var(--text-secondary); background: var(--sidebar-bg); padding: 2px 6px; border-radius: 4px; margin-left: 8px; }
/* ── Toggle switch ── */
.toggle { position: relative; width: 44px; height: 24px; background: var(--toggle-off); border-radius: 12px; cursor: pointer; transition: background 0.2s; flex-shrink: 0; }
.toggle::after { content: ''; position: absolute; top: 2px; left: 2px; width: 20px; height: 20px; background: white; border-radius: 50%; transition: transform 0.2s; box-shadow: 0 1px 3px rgba(0,0,0,0.2); }
.toggle.on { background: var(--toggle-on); }
.toggle.on::after { transform: translateX(20px); }
/* ── select / input ── */
select, input[type="text"], input[type="number"] {
  padding: 6px 10px; border: 1px solid var(--border); border-radius: 6px;
  font-size: 14px; color: var(--text-primary); background: white; min-width: 180px;
}
select:focus, input:focus { outline: none; border-color: var(--primary); }
/* ── 历史记录 ── */
.history-item { padding: 12px 0; border-bottom: 1px solid var(--border); }
.history-item .timestamp { font-size: 12px; color: var(--text-secondary); margin-bottom: 4px; }
.history-item .raw-text { font-size: 14px; line-height: 1.5; }
.history-item .polished-text { font-size: 14px; color: var(--text-secondary); margin-top: 4px; display: none; }
.history-item.expanded .polished-text { display: block; }
.history-item .meta { font-size: 11px; color: var(--text-secondary); margin-top: 4px; display: flex; gap: 12px; }
.history-item .expand-btn { color: var(--primary); cursor: pointer; font-size: 12px; user-select: none; }
/* ── toast ── */
#toast { position: fixed; bottom: 24px; left: 50%; transform: translateX(-50%);
  background: rgba(0,0,0,0.8); color: white; padding: 8px 16px; border-radius: 8px;
  font-size: 14px; z-index: 9999; display: none; }
/* ── 占位 ── */
.placeholder-page { display: flex; align-items: center; justify-content: center; flex-direction: column; height: 100%; color: var(--text-secondary); }
.placeholder-page .icon-big { width: 48px; height: 48px; margin-bottom: 16px; opacity: 0.3; }
</style>
</head>
<body>
<!-- 侧边栏 -->
<div id="sidebar">
  <div class="logo">Octopus</div>
  <nav>
    <div class="nav-item active" data-page="history" onclick="switchPage('history')">
      <div class="icon" style="-webkit-mask-image: url(../result/icons/settings.svg?v=2); mask-image: url(../result/icons/settings.svg?v=2);"></div>
      <span>识别记录</span>
    </div>
    <div class="nav-item" data-page="settings" onclick="switchPage('settings')">
      <div class="icon" style="-webkit-mask-image: url(../result/icons/settings.svg?v=2); mask-image: url(../result/icons/settings.svg?v=2);"></div>
      <span>系统设置</span>
    </div>
    <div class="nav-item" data-page="models" onclick="switchPage('models')">
      <div class="icon" style="-webkit-mask-image: url(../result/icons/settings.svg?v=2); mask-image: url(../result/icons/settings.svg?v=2);"></div>
      <span>模型管理</span>
    </div>
  </nav>
</div>

<!-- 主内容区 -->
<div id="content">
  <!-- 页面 1: 识别记录 -->
  <div class="page active" id="page-history">
    <div id="history-current" style="margin-bottom: 24px;">
      <!-- 当前识别文本（录音中实时更新） -->
    </div>
    <div id="history-list">
      <!-- 历史记录列表 -->
    </div>
    <div id="history-loading" style="text-align: center; padding: 16px; color: var(--text-secondary); display: none;">
      加载中...
    </div>
  </div>

  <!-- 页面 2: 系统设置 -->
  <div class="page" id="page-settings">
    <!-- 由 JS 动态渲染 -->
  </div>

  <!-- 页面 3: 模型管理（占位） -->
  <div class="page" id="page-models">
    <div class="placeholder-page">
      <div style="font-size: 48px; margin-bottom: 16px; opacity: 0.2;">📦</div>
      <p>功能开发中，敬请期待</p>
    </div>
  </div>
</div>

<div id="toast"></div>

<script>
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
let historyOffset = 0;
let historyLoading = false;
let historyDone = false;
let currentConfig = null;

// ── 页面切换 ──
function switchPage(pageName) {
  document.querySelectorAll('.nav-item').forEach(el => el.classList.toggle('active', el.dataset.page === pageName));
  document.querySelectorAll('.page').forEach(el => el.classList.toggle('active', el.id === 'page-' + pageName));
}

// ── Toast ──
function showToast(msg) {
  const toast = document.getElementById('toast');
  toast.textContent = msg;
  toast.style.display = 'block';
  setTimeout(() => { toast.style.display = 'none'; }, 3000);
}
window.showToast = showToast;
window.switchPage = switchPage;

// ── 初始化 ──
async function init() {
  // 加载系统设置
  try {
    const resp = await invoke('get_config');
    currentConfig = resp.config;
    renderSettings(resp);
  } catch (e) {
    showToast('加载配置失败：' + e);
  }
  // 加载历史记录第一页
  await loadHistory();
  // 监听实时识别更新
  listen('update-result', (event) => {
    document.getElementById('history-current').innerHTML =
      '<div style="padding:12px;background:var(--sidebar-bg);border-radius:8px;"><div style="font-size:12px;color:var(--text-secondary);margin-bottom:4px;">当前识别</div><div style="font-size:14px;">' + event.payload + '</div></div>';
  });
}

// ── 历史记录 ──
async function loadHistory() {
  if (historyLoading || historyDone) return;
  historyLoading = true;
  document.getElementById('history-loading').style.display = 'block';
  try {
    const records = await invoke('get_history', { limit: 20, offset: historyOffset });
    if (records.length < 20) { historyDone = true; }
    const list = document.getElementById('history-list');
    if (historyOffset === 0) list.innerHTML = '';
    records.forEach(r => {
      const div = document.createElement('div');
      div.className = 'history-item';
      const time = r.created_at.split(' ')[1] || r.created_at;
      const statusText = { done: '已润色', failed: '润色失败', off: '未润色' }[r.polish_status] || r.polish_status;
      const duration = r.duration_ms ? (r.duration_ms / 1000).toFixed(1) + 's' : '';
      div.innerHTML = `
        <div class="timestamp">${time}</div>
        <div class="raw-text">${escapeHtml(r.raw_text)}</div>
        ${r.polished_text ? `<div class="polished-text">${escapeHtml(r.polished_text)}</div><div class="expand-btn" onclick="this.parentElement.classList.toggle('expanded')">展开/折叠润色</div>` : ''}
        <div class="meta"><span>${escapeHtml(r.engine)}</span><span>${statusText}</span><span>${duration}</span></div>
      `;
      list.appendChild(div);
    });
    historyOffset += records.length;
  } catch (e) {
    showToast('加载历史失败：' + e);
  }
  historyLoading = false;
  document.getElementById('history-loading').style.display = 'none';
}

function escapeHtml(text) {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}
window.loadHistory = loadHistory;

// 滚动加载
document.getElementById('content').addEventListener('scroll', (e) => {
  const el = e.target;
  if (el.scrollHeight - el.scrollTop - el.clientHeight < 100) {
    loadHistory();
  }
});

// ── 系统设置渲染 ──
function renderSettings(resp) {
  const cfg = resp.config;
  const container = document.getElementById('page-settings');
  const asrOptions = resp.asr_engines.map(e => `<option value="${e.name}" ${e.current ? 'selected' : ''}>${escapeHtml(e.label)}</option>`).join('');
  const llmOptions = resp.llm_models.map(m => `<option value="${m.name}" ${m.current ? 'selected' : ''}>${escapeHtml(m.label)}</option>`).join('');
  const micOptions = ['<option value="">系统默认</option>'].concat(resp.microphones.map(m => `<option value="${escapeHtml(m)}" ${cfg.microphone === m ? 'selected' : ''}>${escapeHtml(m)}</option>`)).join('');

  container.innerHTML = `
    <!-- 识别 -->
    <div class="card">
      <h3>识别</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">语言</span></div>
        <select onchange="setVal('language', this.value)"><option value="auto" ${cfg.language==='auto'?'selected':''}>自动</option><option value="zh" ${cfg.language==='zh'?'selected':''}>中文</option><option value="en" ${cfg.language==='en'?'selected':''}>英语</option><option value="ja" ${cfg.language==='ja'?'selected':''}>日语</option><option value="ko" ${cfg.language==='ko'?'selected':''}>韩语</option></select>
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">ASR 引擎</span></div>
        <select onchange="setVal('asr_engine', this.value)">${asrOptions}</select>
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">硬件加速</span><span class="label-hint">GPU/CoreML/DirectML 加速</span></div>
        <div class="toggle ${cfg.asr_hardware_accelerated?'on':''}" onclick="toggleVal('asr_hardware_accelerated', this)"></div>
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">ASR 纠错</span><span class="label-hint">拼音映射 + bigram 校正</span></div>
        <div class="toggle ${cfg.asr_correct?'on':''}" onclick="toggleVal('asr_correct', this)"></div>
        <span class="badge">立即</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">简繁输出</span><span class="label-hint">开启=简体，关闭=繁体</span></div>
        <div class="toggle ${cfg.output_simplified?'on':''}" onclick="toggleVal('output_simplified', this)"></div>
        <span class="badge">立即</span>
      </div>
    </div>
    <!-- 润色 -->
    <div class="card">
      <h3>润色</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">润色模式</span></div>
        <select onchange="setVal('polish_mode', parseInt(this.value))"><option value="0" ${cfg.polish_mode.Disabled!==undefined?'':''}>关闭</option><option value="1">仅最终润色</option><option value="2">中间+最终润色</option></select>
        <span class="badge">立即</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">润色模型</span></div>
        <select onchange="setVal('polish_llm', this.value)">${llmOptions}</select>
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">润色间隔</span><span class="label-hint">秒</span></div>
        <input type="number" min="0" step="0.5" value="${cfg.polish_interval}" onchange="setVal('polish_interval', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">停顿润色阈值</span><span class="label-hint">毫秒（须 > 500）</span></div>
        <input type="number" min="501" step="50" value="${cfg.pause_polish_threshold_ms}" onchange="setVal('pause_polish_threshold_ms', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
    </div>
    <!-- 降噪 -->
    <div class="card">
      <h3>降噪</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">降噪模式</span></div>
        <select onchange="setVal('denoise_mode', parseInt(this.value))"><option value="0" ${cfg.denoise_mode===0?'selected':''}>无</option><option value="1" ${cfg.denoise_mode===1?'selected':''}>轻度</option><option value="2" ${cfg.denoise_mode===2?'selected':''}>深度</option></select>
        <span class="badge">立即</span>
      </div>
    </div>
    <!-- VAD 分段 -->
    <div class="card">
      <h3>VAD 分段</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">分段时长</span><span class="label-hint">秒</span></div>
        <input type="number" min="1" step="0.5" value="${cfg.segment_duration}" onchange="setVal('segment_duration', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">静音阈值</span><span class="label-hint">毫秒</span></div>
        <input type="number" min="100" step="50" value="${cfg.segment_silence}" onchange="setVal('segment_silence', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">分段重叠</span><span class="label-hint">毫秒</span></div>
        <input type="number" min="0" step="50" value="${cfg.segment_overlap}" onchange="setVal('segment_overlap', parseFloat(this.value))">
        <span class="badge">下次录音</span>
      </div>
    </div>
    <!-- 音频 -->
    <div class="card">
      <h3>音频</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">麦克风设备</span></div>
        <select onchange="setVal('microphone', this.value)">${micOptions}</select>
        <span class="badge">下次录音</span>
      </div>
    </div>
    <!-- 交互 -->
    <div class="card">
      <h3>交互</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">全局快捷键</span></div>
        <input type="text" value="${escapeHtml(cfg.shortcut)}" onchange="setVal('shortcut', this.value)">
        <span class="badge">重启</span>
      </div>
      <div class="row">
        <div class="label-group"><span class="label-text">工具栏自动隐藏</span><span class="label-hint">关闭=工具栏始终显示</span></div>
        <div class="toggle ${cfg.hide_toolbar?'on':''}" onclick="toggleVal('hide_toolbar', this)"></div>
        <span class="badge">立即</span>
      </div>
    </div>
    <!-- 引擎模式 -->
    <div class="card">
      <h3>引擎模式</h3>
      <div class="row">
        <div class="label-group"><span class="label-text">引擎接入模式</span><span class="label-hint">embedded=本地推理</span></div>
        <select onchange="setVal('engine_mode', this.value)"><option value="embedded" ${cfg.engine_mode==='embedded'?'selected':''}>embedded</option><option value="websocket" ${cfg.engine_mode==='websocket'?'selected':''}>websocket</option><option value="grpc" ${cfg.engine_mode==='grpc'?'selected':''}>grpc</option></select>
        <span class="badge">重启</span>
      </div>
    </div>
  `;
}

// ── 设置写入 ──
async function setVal(key, value) {
  try {
    await invoke('set_config', { key, value });
  } catch (e) {
    showToast(e);
    // 失败：重新加载配置以恢复控件旧值
    const resp = await invoke('get_config');
    renderSettings(resp);
  }
}
window.setVal = setVal;

async function toggleVal(key, el) {
  const newVal = !el.classList.contains('on');
  try {
    await invoke('set_config', { key, value: newVal });
    el.classList.toggle('on', newVal);
  } catch (e) {
    showToast(e);
  }
}
window.toggleVal = toggleVal;

// polish_mode 需要 u8，select 返回的是字符串数字 — setVal 已在 onchange 用 parseInt 转换
// 但 polish_mode 的当前值选中状态需要修正：cfg.polish_mode 在 JSON 中可能是对象
// 修正 polish_mode 下拉框初始选中
function fixPolishModeSelected(cfg) {
  // polish_mode 被 serde 序列化为 "Disabled"/"FinalOnly"/"Intermediate" 字符串
  // 但实际 serde_json 对 unit struct 会序列化为其他形式——需测试确认
}
// 注意：polish_mode 的序列化值需运行时确认，可能需要调整。

// ── 启动 ──
init();
</script>
</body>
</html>
```

- [ ] **Step 2: 编译验证（确保 dist/settings/ 目录被 Tauri 识别）**

Run: `cargo check -p octopus-desktop --features embedded`
Expected: 编译通过（Tauri 的 `frontendDist: "dist"` 相对路径包含 `settings/` 子目录）。

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/dist/settings/index.html
git commit -m "feat(desktop): 设置窗口前端 — 侧边栏 + 3 页面 + 设置表单 + 历史记录"
```

---

### Task 7: polish_mode 序列化修正 + e2e 联调

**Files:**
- Modify: `crates/desktop/dist/settings/index.html`（如需修正 polish_mode）
- Modify: `crates/desktop/src/settings_commands.rs`（如需修正序列化）

`PolishMode` 是一个 enum，serde 默认序列化为 `{"Disabled":{}}` 形式或字符串。需要确认实际序列化形式并修正前端下拉框选中逻辑。

- [ ] **Step 1: 构建 release 并运行**

```bash
cd crates/desktop && cargo run --release --features embedded
```

- [ ] **Step 2: 手动测试 — 基本功能**

打开应用后：
1. 点击工具栏设置按钮 → 设置窗口打开
2. 切换三个页面正常
3. 识别记录页显示历史
4. 系统设置页控件渲染正确

- [ ] **Step 3: 手动测试 — 实时保存**

1. 切换"ASR 纠错"开关 → 检查 `~/.octopus/config.yaml` 中 `asr_correct` 值已更新
2. 修改"分段时长" → 检查 config.yaml
3. 输入非法值（停顿润色阈值=100）→ 检查 toast 错误提示
4. 切换润色模式下拉 → 确认 polish_mode 值正确（需确认序列化形式）

- [ ] **Step 4: 手动测试 — 历史记录**

1. 做几次录音 → 打开设置 → 识别记录页正确显示
2. 滚动到底部 → 确认翻页加载

- [ ] **Step 5: 如有 polish_mode 序列化问题，修正**

`PolishMode` 在 `infra::config` 中定义为 `#[derive(Serialize)]` 的 enum。serde 默认序列化为 `{"Disabled":{}}` 或 `"Disabled"`（取决于是否加了 `#[serde(rename_all = ...)]`）。检查实际序列化形式：

```bash
# 查看当前 config.yaml 中 polish_mode 的值（已被 serde_yaml 序列化过）
cat ~/.octopus/config.yaml | grep polish_mode
```

如果序列化为 `polish_mode: Disabled`（字符串），前端下拉框需用字符串值匹配。如果序列化为数字（自定义 Serialize），则按数字处理。根据实际形式修正 `renderSettings` 中的 polish_mode 选中逻辑。

- [ ] **Step 6: Commit 联调修正**

```bash
git add -A
git commit -m "fix(desktop): polish_mode 序列化修正 + e2e 联调"
```

---

### Task 8: 文档同步 + 最终提交

**Files:**
- Modify: `docs/architecture.md`
- Modify: `docs/configuration.md`

- [ ] **Step 1: 在 architecture.md 补充设置窗口说明**

在 architecture.md 的 desktop 窗口管理表格中（`result_window` 行之后），添加 `settings_window` 行：

```
| `settings_window` | GUI 设置界面（原生标题栏、可调大小 800×600、单例）。三页面：识别记录（transcriptions 表浏览）/ 系统设置（config.yaml 19 字段实时保存）/ 模型管理（占位）。入口：工具栏设置按钮 + 托盘"设置..."菜单。通用 `get_config`/`set_config(key,value)` 命令 + `get_history` 分页查询。 |
```

在 architecture.md 的 desktop 模块说明中补充设置窗口子系统段落（参考 RuntimeConfig 段落的风格）。

- [ ] **Step 2: 在 configuration.md 补注 GUI 编辑入口**

在 configuration.md 的 config.yaml 表格之后，添加：

```markdown
> **GUI 编辑**：`config.yaml` 的上述字段现可经设置窗口 GUI 编辑（工具栏设置按钮或托盘菜单"设置..."打开），实时保存 + 持久化。部分字段标注生效时机（立即 / 下次录音 / 重启）。
```

- [ ] **Step 3: 最终编译 + 测试**

Run: `cargo check -p octopus-desktop --features embedded && cargo test -p octopus-desktop --features embedded`
Expected: 编译零警告，全部测试通过。

- [ ] **Step 4: Commit**

```bash
git add docs/architecture.md docs/configuration.md
git commit -m "docs: 设置窗口架构 + 配置 GUI 编辑说明"
```

---

### Task 9: 识别记录页面增强（工具栏 + 批量删除 + 文本顺序反转 + 拷贝）

**Files:**
- Modify: `crates/infra/src/db.rs`（新增 `delete_transcriptions` / `delete_transcriptions_at`）
- Modify: `crates/desktop/src/settings_commands.rs`（新增 `delete_history` 命令）
- Modify: `crates/desktop/src/main.rs`（注册 `delete_history`）
- Modify: `crates/desktop/dist/settings/index.html`（历史页 UI 大改）

- [x] **Step 1: 后端 — `db.rs` 新增批量删除函数**

在 `list_transcriptions` 之后新增 `delete_transcriptions(ids: &[i64])`（公开，走 `with_db`）和 `delete_transcriptions_at(conn, ids)`（内部，可直连 Connection 测试）。SQL：`DELETE FROM transcriptions WHERE id IN (?,?,...)`，空列表直接返回 `Ok(0)` 不执行 SQL。

测试（3 个）：
- `delete_transcriptions_removes_specified_ids`：插入 3 条删 2 条，验证剩余 1 条
- `delete_transcriptions_at_empty_is_noop`：空列表不报错、不删数据
- `delete_transcriptions_at_via_internal_fn`：正常批量删除

- [x] **Step 2: 后端 — `settings_commands.rs` 新增 `delete_history` 命令**

```rust
#[tauri::command]
pub fn delete_history(ids: Vec<i64>) -> Result<usize, String> {
    octopus_infra::db::delete_transcriptions(&ids).map_err(|e| e.to_string())
}
```

`main.rs` invoke_handler 追加 `settings_commands::delete_history`。

- [x] **Step 3: 前端 — 历史页 UI 重构（`dist/settings/index.html`）**

**CSS 新增：**
- `#history-toolbar`：flex 布局，全选 checkbox + 已选计数（左侧）+ 删除按钮（右侧，红色边框，disabled 时灰）
- `.history-item` 改为 flex 布局：checkbox + item-body + item-actions
- `.item-check`：18×18 checkbox
- `.item-body`：flex:1（时间 + 润色 text + 原始 text 折叠 + meta）
- `.item-actions`：拷贝按钮
- `.polished-text`：主文本（黑色，默认显示）
- `.raw-text`：次要文本（灰色，默认 `display:none`，`.expanded` 时显示）— **逻辑反转**

**HTML 结构变更：**
- `#page-history` 内 `#history-current` 之后新增 `#history-toolbar`（全选 checkbox + 删除按钮），初始 `display:none`（有数据时显示）

**JS 变更：**
- `loadHistory()`：记录渲染改为「checkbox + 润色优先 + 拷贝按钮」结构。`data-id` 挂在 `.history-item` 上。首屏时显隐 toolbar。
- 新增 `selectedIds: Set<number>` 状态
- 新增 `onItemCheck(checkbox, id)`：增删 selectedIds + updateSelectedCount
- 新增 `updateSelectedCount()`：更新计数文字、删除按钮 disabled、全选 checkbox 状态（含 indeterminate）
- 新增 `toggleSelectAll(checked)`：批量勾选/取消可见记录
- 新增 `deleteSelected()`：`confirm()` → `invoke('delete_history', {ids})` → 刷新列表（重置 offset）
- 新增 `copyRecord(id)`：取 `.polished-text` 文本 → `navigator.clipboard.writeText`（fallback `execCommand`）

- [x] **Step 4: 测试验证**

```bash
cargo test -p octopus-infra  # 25 tests pass
cargo check -p octopus-desktop --features embedded  # 编译通过
node -e "..."  # JS 语法检查通过
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(desktop): 识别记录页增强—工具栏批量删除 + 文本顺序反转 + 拷贝"
```

---

## 自检清单

### Task 10: macOS Dock 图标动态显隐 + UI 微调

**Files:**
- Modify: `crates/desktop/src/settings_window.rs`（`open_settings` 加 `Regular`、新增 `on_settings_closed`）
- Modify: `crates/desktop/src/main.rs`（启动设 `Accessory`、run 回调监听窗口 Destroyed）
- Modify: `crates/desktop/dist/settings/index.html`（去 logo、拷贝图标、删除确认态 reset）

- [x] **Step 1: macOS 动态激活策略**

启动 `main.rs` 在 `app.run()` 前设 `ActivationPolicy::Accessory`（无 Dock 图标）。`open_settings` 创建窗口前切 `Regular`（Dock 图标出现）。新增 `on_settings_closed(&AppHandle)` 切回 `Accessory`。`main.rs` 的 `app.run()` 回调监听 `RunEvent::WindowEvent { event: Destroyed, label: "settings_window" }` 触发该回调。全 `#[cfg(target_os = "macos")]` 条件编译。

- [x] **Step 2: 侧边栏去 logo**

去掉 `dist/settings/index.html` 侧边栏的 `<div class="logo">Octopus</div>`（窗口 title 已有「Octopus 设置」）。

- [x] **Step 3: 拷贝按钮改图标**

将文字「拷贝」按钮替换为内联 `copy.svg` SVG 图标（16×16，灰色，hover 蓝色）。CSS `.btn-copy` 改为无边框透明背景 icon button。

- [x] **Step 4: 删除确认态自动 reset**

Tauri webview 不支持 `window.confirm()`（返回 `undefined` → falsy → 删除被跳过，数据库不删）。改为两次点击确认（首次变红「确认删除?」3 秒超时）。提取 `resetDeleteConfirm(btn)` 函数，在 `updateSelectedCount()` 中统一调用——勾选/取消任何条目、全选/全不选、超时均自动取消确认态恢复按钮。

- [x] **Step 5: 验证 + Commit**

```bash
cargo test -p octopus-infra -p octopus-desktop --features embedded  # 全绿
cargo build --release -p octopus-desktop --features embedded        # 编译通过
```

---

### Task 11: UI 精细化调整 + 快捷键热重载

**Files:**
- Modify: `crates/desktop/dist/settings/index.html`（侧边栏图标 / section 标题 / label 内联 badge / 语言选项 / 润色间隔+阈值改下拉 / 快捷键捕获）
- Modify: `crates/desktop/src/settings_commands.rs`（`check_shortcut` 命令、`set_config` 快捷键热重载、`pause_polish_threshold_ms >= 500`）
- Modify: `crates/desktop/src/main.rs`（注册 `check_shortcut`）

- [x] **Step 1: 侧边栏图标替换**
- 识别记录 → `message.svg`
- 模型管理 → `model.svg`
- 系统设置 → 保持 `settings.svg`

- [x] **Step 2: 去掉侧边栏 logo**
- 删掉 `<div class="logo">Octopus</div>`（窗口 title 已有「Octopus 设置」）

- [x] **Step 3: 语言选项精简**
- 语言下拉去掉日语/韩语，只保留 自动/中文/英语
- 「语言」label 改为「语言识别」

- [x] **Step 4: 卡片标题精简 + 交互卡置顶**
- 交互卡片移到第一位（在识别之前）
- 去掉交互/识别/润色/降噪/音频/引擎模式的 `<h3>` 标题，只保留 VAD 分段标题

- [x] **Step 5: 生效时间标签内联到 label**
- 去掉独立的右侧 `<span class="badge">`，改为 label 文字后面的灰色小字 `(.label-effect)` 带括号，如「语言识别 (下次录音)」

- [x] **Step 6: 快捷键捕获 + 冲突检测 + 热重载**
- 「全局快捷键」改名「激活/关闭快捷键」，text input 改为快捷键捕获按钮
- 捕获逻辑：点击 → 显示「按下快捷键…」→ keydown 捕获组合键（修饰键+主键）→ Esc 取消
- `check_shortcut` 后端命令：尝试 `on_shortcut` 注册 → 立即 `unregister` → 检测冲突
- 前端流程：捕获 → `check_shortcut` → 成功才 `setVal`，失败 toast + 恢复
- `set_config` 快捷键热重载：注销旧快捷键 + `register_shortcut` 新的，标签从「重启」改为「立即」

- [x] **Step 7: 润色间隔 / 说话换气间隔改下拉**
- 润色间隔：number input → 下拉（仅最后=0 / 每3~8秒），去掉 hint
- 停顿润色阈值 → 改名「说话换气间隔」，number input → 下拉（500~1000ms 六档），去掉 hint
- 后端约束从 `> 500` 改为 `>= 500`

---

## 自检清单

### Spec 覆盖
- ✅ 窗口架构（Task 4）
- ✅ get_config / set_config / get_history / delete_history / check_shortcut 命令（Task 3 + 9 + 11）
- ✅ RuntimeConfig 扩展（Task 2）
- ✅ 工具栏 + 托盘入口（Task 5）
- ✅ 前端三页面（Task 6）
- ✅ 识别记录页—增强（Task 9：工具栏 + 批量删除 + 文本反转 + 拷贝图标）
- ✅ 系统设置页（Task 6 + Task 11 精细化：标题精简 / label 内联 badge / 快捷键捕获 / 下拉化）
- ✅ 模型管理占位（Task 6 HTML）
- ✅ macOS Dock 动态显隐（Task 10）
- ✅ 快捷键冲突检测 + 热重载（Task 11）
- ✅ 侧边栏图标（Task 11：message.svg / model.svg / settings.svg）
- ✅ 跨平台（vanilla HTML + cpal + Tauri 标准 API + macOS 条件编译）
- ✅ 错误处理（Task 3 校验 + Task 6/9/10/11 toast）
- ✅ 文档同步（Task 8 + Task 10 + Task 11）

### 已知风险
- **PolishMode 序列化**：已确认序列化为 `u8`（0/1/2），前端 select 用数字 value（Task 7 已修）。
- **Tauri confirm() 不可用**：所有需要确认的操作均用两次点击替代（Task 10 已处理删除场景）。
- **macOS Dock 图标**：release 裸二进制无 .app bundle，通过 `objc2` 手动 `setApplicationIconImage`（Task 10）。
- **check_shortcut 注册+注销时序**：检测时短暂注册可能与其他应用极小概率竞争，实测可接受。
