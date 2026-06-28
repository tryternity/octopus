# 设置页「模型选择」Card 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在系统设置页「交互」Card 下方新增独立「模型选择」Card，集中 asr_engine / polish_llm / ocr_model 三类模型选择；补 `ocr_model` 进 AppConfig 持久化链路；新增 `list_ocr_models` 数据源。

**Architecture:** 前端 Card 重组（搬运 asr/polish 行 + 新增 ocr 行）+ 后端补漏（ocr_model 纳入 AppConfig load/save）+ 新增 OCR 选项查询（db list_ocr_models → runtime_config build_ocr_options → ConfigResponse）。OCR 因 OnceLock 单例，改后重启生效（不热重载）。

**Tech Stack:** Rust（infra db/config + desktop runtime_config/settings_commands）/ TypeScript（React）/ SQLite models 表 domain='ocr'。

**关联 spec:** [2026-06-28-settings-model-selection-design.md](../specs/2026-06-28-settings-model-selection-design.md)

---

## File Structure

| 文件 | 责任 | 改动 |
|------|------|------|
| `crates/infra/src/config.rs` | AppConfig schema | +`ocr_model` 字段 + default fn + Default impl + 单测 |
| `crates/infra/src/db.rs` | DB load/save + 模型查询 | load +分支 / save +字段（27→28）/ +`OcrModelInfo` +`list_ocr_models` +单测 |
| `crates/desktop/src/runtime_config.rs` | 选项 DTO + 构造 | +`OcrOption` +`build_ocr_options_public` |
| `crates/desktop/src/settings_commands.rs` | get_config / set_config | ConfigResponse +`ocr_models` / get_config 组装 / apply_config_value +`ocr_model` 分支 |
| `crates/desktop/frontend/src/pages/Settings/index.tsx` | ConfigResponse 接口 | +`ocr_models` 字段 |
| `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` | 设置页 | +「模型选择」Card / 删识别引擎行 / 删润色模型行 / import Layers |
| `docs/architecture.md` | 架构文档 | 同步 Card 清单 + ocr_model 字段 + list_ocr_models |

---

## Task 1: `config.rs` 补 `ocr_model` 字段

**Files:**
- Modify: `crates/infra/src/config.rs`（字段 L176-177 区、default fn L248-250 区、Default impl L285 区、单测 L331 区）

- [x] **Step 1.1: 加字段定义**

在 `clipboard_max_age_days` 字段（L176-177）之后、结构体闭合 `}`（L178）之前插入：

```rust

    /// OCR 模型（当前激活），对应 ~/.octopus/models/ocr/<name>/ 目录名。
    /// 默认 "PP-OCRv6-small"。OCR 引擎 OnceLock 单例缓存，改后重启生效。
    #[serde(default = "default_ocr_model")]
    pub ocr_model: String,
```

- [x] **Step 1.2: 加 default 函数**

在 `default_clipboard_max_age_days`（L248-250）之后插入：

```rust
fn default_ocr_model() -> String {
    "PP-OCRv6-small".into()
}
```

- [x] **Step 1.3: Default impl 加初始化**

在 Default impl 的 `clipboard_max_age_days: default_clipboard_max_age_days(),`（L285）之后、闭合 `}`（L286）之前插入：

```rust
            ocr_model: default_ocr_model(),
```

- [x] **Step 1.4: 单测加断言**

在 `app_config_default_values` 测试的 `assert_eq!(cfg.polish_global_shortcut, "CmdOrCtrl+Shift+S");`（L331）之后插入：

```rust
        assert_eq!(cfg.ocr_model, "PP-OCRv6-small");
```

- [x] **Step 1.5: 验证 config 编译 + 单测**

Run: `cargo test -p octopus-infra config::tests -- --nocapture`
Expected: PASS，含 `ocr_model == "PP-OCRv6-small"` 断言通过。

---

## Task 2: `db.rs` load/save + `list_ocr_models`

**Files:**
- Modify: `crates/infra/src/db.rs`（load L321 区、save L356/370/386 区、LlmModelInfo 区 L650-684、测试区 L1340 区）

- [x] **Step 2.1: load_app_config_at 加 ocr_model 分支**

在 `"polish_llm" => cfg.polish_llm = value,`（L321）之后插入（字符串区分支）：

```rust
            "ocr_model" => cfg.ocr_model = value,
```

- [x] **Step 2.2: save_app_config_at 注释改 28 字段**

L356 注释 `/// 全量写入应用配置（27 字段 ON CONFLICT DO UPDATE）。` → `28 字段`：

```rust
/// 全量写入应用配置（28 字段 ON CONFLICT DO UPDATE）。set_config / yaml 迁移用。
```

- [x] **Step 2.3: save_app_config_at 数组长度 27 → 28**

L370 `let fields: [(&str, String); 27] = [` → `28`：

```rust
    let fields: [(&str, String); 28] = [
```

- [x] **Step 2.4: save_app_config_at 加 ocr_model 字段**

在 `("polish_llm", cfg.polish_llm.clone()),`（L386）之后插入：

```rust
        ("ocr_model", cfg.ocr_model.clone()),
```

- [x] **Step 2.5: 加 OcrModelInfo + list_ocr_models**

在 `list_llm_models`（L682-684）之后插入：

```rust

/// OCR 模型列表项（菜单用，仅含显示字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrModelInfo {
    pub model_name: String,
    pub description: String,
}

/// 列出所有启用的 OCR 模型（domain='ocr' AND is_enabled=1）。
fn list_ocr_models_at(conn: &Connection) -> Result<Vec<OcrModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT model_name, description FROM models
         WHERE domain='ocr' AND is_enabled = 1",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(OcrModelInfo {
            model_name: row.get::<_, String>(0)?,
            description: row.get::<_, String>(1)?,
        })
    })?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 从 DB 列出启用的 OCR 模型（经 with_db，供 Tauri 命令调用）。
pub fn list_ocr_models() -> Result<Vec<OcrModelInfo>> {
    with_db(|conn| list_ocr_models_at(conn))
}
```

- [x] **Step 2.6: 加 list_ocr_models 单测**

在 `list_llm_models_at_empty_when_all_disabled` 测试（L1334-1340）之后插入：

```rust

    #[test]
    fn list_ocr_models_returns_enabled() {
        let conn = open_init();
        let list = list_ocr_models_at(&conn).unwrap();
        // seed 默认 1 条 OCR（PP-OCRv6-small, is_enabled=1）
        assert_eq!(list.len(), 1, "seed 1 条启用 OCR");
        assert_eq!(list[0].model_name, "PP-OCRv6-small");
        assert!(!list[0].description.is_empty(), "description 非空");
    }

    #[test]
    fn list_ocr_models_filters_disabled() {
        let conn = open_init();
        conn.execute("UPDATE models SET is_enabled = 0 WHERE domain='ocr'", []).unwrap();
        let list = list_ocr_models_at(&conn).unwrap();
        assert!(list.is_empty(), "全禁用时返回空");
    }
```

- [x] **Step 2.7: 验证 infra 编译 + 单测**

Run: `cargo test -p octopus-infra -- --nocapture`
Expected: PASS（含 config ocr_model 默认值 + list_ocr_models 两测）。

- [x] **Step 2.8: Commit**

```bash
git add crates/infra/src/config.rs crates/infra/src/db.rs
git commit -m "feat(infra): ocr_model 纳入 AppConfig（28 字段）+ list_ocr_models 查询"
```

---

## Task 3: `runtime_config.rs` 加 `OcrOption` + 构造函数

**Files:**
- Modify: `crates/desktop/src/runtime_config.rs`（LlmOption L152-158 区、build_llm_options_public L198-203 区）

- [x] **Step 3.1: 加 OcrOption 结构**

在 `LlmOption` 结构（L152-158）之后插入：

```rust

/// OCR 模型菜单项（与 LlmOption 同构，current 标记当前选中的 ocr_model）。
/// 与 LLM 的区别：不做「不选择模型」首项——OCR 必须有一个模型。
#[derive(Serialize)]
pub struct OcrOption {
    pub name: String,
    pub label: String,
    pub current: bool,
}
```

- [x] **Step 3.2: 加 build_ocr_options + 公开包装**

在 `build_llm_options_public`（L198-203）之后插入：

```rust

/// 构造 OCR 选项列表（纯逻辑）：DB 启用的 OCR 模型，current 按裸 model_name 标记。
/// 不做「不选择」首项（OCR 必须有一个）。label 优先 description，空则 model_name。
fn build_ocr_options(current: &str, ocrs: Vec<octopus_infra::db::OcrModelInfo>) -> Vec<OcrOption> {
    ocrs.into_iter()
        .map(|m| OcrOption {
            current: m.model_name == current,
            label: if m.description.is_empty() {
                m.model_name.clone()
            } else {
                m.description
            },
            name: m.model_name,
        })
        .collect()
}

/// 公开包装（供 settings_commands 调用）。
pub fn build_ocr_options_public(
    current: &str,
    ocrs: Vec<octopus_infra::db::OcrModelInfo>,
) -> Vec<OcrOption> {
    build_ocr_options(current, ocrs)
}
```

- [x] **Step 3.3: 验证 desktop 编译**

Run: `cargo check -p octopus-desktop`
Expected: 0 error（OcrOption/build_ocr_options_public 暂未使用，可能有 dead_code warning，Task 4 消除）。

---

## Task 4: `settings_commands.rs` ConfigResponse + get_config + apply 分支

**Files:**
- Modify: `crates/desktop/src/settings_commands.rs`（ConfigResponse L19 区、get_config L34-35/54-59 区、apply_config_value L260-262 区）

- [x] **Step 4.1: ConfigResponse 加 ocr_models 字段**

在 `pub llm_models: Vec<crate::runtime_config::LlmOption>,`（L19）之后插入：

```rust
    pub ocr_models: Vec<crate::runtime_config::OcrOption>,
```

- [x] **Step 4.2: get_config 组装 ocr_models**

在 `let llm_models = crate::runtime_config::build_llm_options_public(&g.polish_llm, llms);`（L35）之后插入：

```rust

    let ocrs = octopus_infra::db::list_ocr_models().map_err(|e| e.to_string())?;
    let ocr_models = crate::runtime_config::build_ocr_options_public(&g.ocr_model, ocrs);
```

- [x] **Step 4.3: get_config 返回填 ocr_models**

L52-59 `Ok(ConfigResponse { ... })` 加 `ocr_models,`（在 `llm_models,` 之后）：

```rust
    Ok(ConfigResponse {
        config: config_json,
        asr_engines,
        llm_models,
        ocr_models,
        microphones,
        prompts,
        active_prompt_id,
    })
```

- [x] **Step 4.4: apply_config_value 加 ocr_model 分支**

在 `"polish_global_shortcut" => { ... }`（L260-262）之后插入（裸 model_name，简单字符串校验，照 asr_shortcut 模板）：

```rust
        "ocr_model" => {
            cfg.ocr_model = value.as_str().ok_or("ocr_model 需要字符串")?.to_string();
        }
```

- [x] **Step 4.5: 验证 desktop 编译 + 单测**

Run: `cargo test -p octopus-desktop settings_commands`
Expected: PASS（既有 apply_config_value 单测不受影响）。

- [x] **Step 4.6: Commit**

```bash
git add crates/desktop/src/runtime_config.rs crates/desktop/src/settings_commands.rs
git commit -m "feat(desktop): OcrOption + build_ocr_options + get_config 组装 + apply ocr_model 分支"
```

---

## Task 5: 前端 ConfigResponse 接口 + GeneralPanel 模型选择 Card

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/index.tsx`（ConfigResponse L15 区）
- Modify: `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx`（import L4、解构 L87、交互 Card 后 L145、删识别引擎行 L162-167、删润色模型行 L189-196）

- [x] **Step 5.1: index.tsx ConfigResponse 加 ocr_models**

在 `llm_models: { name: string; label: string; current: boolean }[];`（L15）之后插入：

```ts
  ocr_models: { name: string; label: string; current: boolean }[];
```

- [x] **Step 5.2: GeneralPanel import 加 Layers**

L4 改：

```tsx
import { Mic, Volume2, Sparkles, Keyboard, ClipboardList, Layers } from "lucide-react";
```

- [x] **Step 5.3: GeneralPanel 解构加 ocr_models**

L87 改：

```tsx
  const { config: cfg, asr_engines, llm_models, ocr_models, prompts, active_prompt_id, microphones } = configResp;
```

- [x] **Step 5.4: 新增「模型选择」Card（交互 Card 之后）**

在「交互」Card 闭合 `</Card>`（L145）之后、「快捷键」Card（L147）之前插入：

```tsx

      <Card icon={Layers} title="模型选择">
        <Row label="语音识别模型" effect="下次录音">
          <select className={selectClass} value={cfg.asr_engine as string} onChange={(e) => setVal("asr_engine", e.target.value)}>
            {asr_engines.map((e) => <option key={e.name} value={e.name}>{e.label}</option>)}
          </select>
        </Row>
        <Row label="润色模型" effect="立即">
          <select className={selectClass}
            value={llm_models.find((m) => m.current)?.name ?? ""}
            onChange={(e) => setVal("polish_llm", e.target.value)}>
            {llm_models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
          </select>
        </Row>
        <Row label="OCR 模型" effect="下次启动" hint="截图识别用，改后重启生效">
          <select className={selectClass} value={cfg.ocr_model as string} onChange={(e) => setVal("ocr_model", e.target.value)}>
            {ocr_models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
          </select>
        </Row>
      </Card>
```

- [x] **Step 5.5: 删除「语音识别」Card 的识别引擎行**

删除「语音识别」Card 内的识别引擎 Row（L162-167）：

```tsx
        <Row label="识别引擎" effect="下次录音">
          <select className={selectClass} value={cfg.asr_engine as string} onChange={(e) => setVal("asr_engine", e.target.value)}>
            {asr_engines.map((e) => <option key={e.name} value={e.name}>{e.label}</option>)}
          </select>
        </Row>
```

- [x] **Step 5.6: 删除「语音识别润色」Card 的润色模型行**

删除「语音识别润色」Card 内的润色模型 Row（L189-196）：

```tsx
        <Row label="润色模型" effect="立即">
          <select className={selectClass}
            value={llm_models.find((m) => m.current)?.name ?? ""}
            onChange={(e) => setVal("polish_llm", e.target.value)}>
            {llm_models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
          </select>
        </Row>
```

- [x] **Step 5.7: 验证前端 build**

Run: `npm --prefix crates/desktop/frontend run build`
Expected: tsc + vite 通过，新 bundle 生成（含模型选择 Card + ocr_models 接口）。

- [x] **Step 5.8: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/index.tsx crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx crates/desktop/dist
git commit -m "feat(desktop): 设置页模型选择 Card（asr/polish/ocr 集中）+ 删除原两行"
```

---

## Task 6: 全量验证

- [x] **Step 6.1: 后端全量编译 + 测试**

Run: `cargo test -p octopus-infra -p octopus-desktop`
Expected: 0 error，单测全绿（含 config ocr_model + list_ocr_models 两测）。

- [x] **Step 6.2: 前端最终 build**

Run: `npm --prefix crates/desktop/frontend run build`
Expected: 通过。

---

## Task 7: 文档同步 + plan checkbox

**Files:**
- Modify: `docs/architecture.md`（设置页 Card 清单 + ocr_model 字段 + list_ocr_models）
- Modify: 本 plan（checkbox 全勾）

- [x] **Step 7.1: architecture.md 设置页 Card 清单**

设置页 Card 清单由「交互/快捷键/语音识别/语音识别润色/剪贴板」改为「交互/模型选择/快捷键/语音识别/语音识别润色/剪贴板」；标注「模型选择」含 asr_engine（下次录音）/ polish_llm（立即）/ ocr_model（下次启动，OnceLock 重启生效）；「语音识别」Card 去识别引擎行、「语音识别润色」Card 去润色模型行。

- [x] **Step 7.2: architecture.md ocr_model 字段 + list_ocr_models**

AppConfig 字段表加 `ocr_model`（默认 PP-OCRv6-small，OCR 引擎 OnceLock 单例，改后重启生效）；models 查询列表加 `list_ocr_models`（domain='ocr' AND is_enabled=1）；save_app_config 字段数 27→28。

- [x] **Step 7.3: 本 plan checkbox 全勾 + Commit 文档**

```bash
git add docs/architecture.md docs/superpowers/plans/2026-06-28-settings-model-selection.md
git commit -m "docs: 模型选择 Card 同步 architecture + plan checkbox"
```

---

## 验证清单（e2e，待用户桌面环境确认）

1. 设置页「交互」正下方出现「模型选择」Card，含三行：语音识别模型 / 润色模型 / OCR 模型。
2. 语音识别模型下拉 = 原 asr_engines 选项，切换后下次录音生效（与原行为一致）。
3. 润色模型下拉 = 原 llm_models 选项（含「不选择模型」首项），切换后立即生效。
4. OCR 模型下拉显示 PP-OCRv6-small（description 标签），切换后写 DB；重启应用后 OCR 用新模型（OnceLock）。
5. 原「语音识别」Card 不再有「识别引擎」行；原「语音识别润色」Card 不再有「润色模型」行。
6. 重启应用：ocr_model 配置持久化（设置页显示上次选择）；DB app_config 表 ocr_model 行更新。

## 不改动

- `ocr/engine.rs::OcrEngine::instance()` 读取入口（仍 `load_config_key("ocr_model")`）、recognize、单例缓存。
- 侧栏「模型管理」Tab（ModelsPanel，下载/校验 ASR 模型）。
- `db.sql`（OCR models seed + app_config seed 均已存在）。
- ASR/polish 后端切换逻辑（仅前端换 Card 归属）。
