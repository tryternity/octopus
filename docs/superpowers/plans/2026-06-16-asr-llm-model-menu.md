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
