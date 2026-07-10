# 模型管理页面 Tab 化 + 环境变量配置 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 设置页「模型管理」改为 5 tab（常量/ASR/Text-Chat/OCR/翻译），模型下载地址支持 `{huggingface}` 等变量模板替换。

**Architecture:** DB `app_config` 表新增 category='env' 的环境变量行。前端 `ModelsPanel` 重构为 Tabs 容器，各 tab 独立组件。下载链路从旧 `download_mirror` 前缀拼接改为 `{key}` 模板替换。

**Tech Stack:** Rust + Tauri 2 + SQLite + React + Tailwind

## Global Constraints

- **GeneralPanel 模型选择不动**——新 tab 只管模型列表/toggle，不取代 GeneralPanel 的下拉选择
- **内置 3 变量不可删**——`huggingface`/`modelscope`/`github` 的值可改，key 不可改，行不可删
- **变量替换仅用于 ASR 模型下载**——LLM/OCR 的 `source`（API base URL）不做替换
- **`save_config_key` 不带 category 参数**——现有实现是 `INSERT INTO app_config (config_key, config_value)`，category 默认 NULL。env 变量也用此函数，但 key 加 `env.` 前缀（如 `env.huggingface`）做命名空间隔离

**Spec:** [`2026-07-10-models-tab-design.md`](../specs/2026-07-10-models-tab-design.md)

---

### Task 1: DB seed + env 变量 CRUD

**Files:**
- Modify: `crates/infra/src/db.sql`
- Modify: `crates/infra/src/db.rs`

**Interfaces:**
- Produces: `list_env_vars() -> Vec<(String, String)>` / `save_env_var(key, value)` / `delete_env_var(key) -> Result<bool>`（返回是否是内置变量，内置返回 false）

- [ ] **Step 1: db.sql seed 加 3 行 env 变量**

在 db.sql 的 app_config seed INSERT 末尾（`ocr_model` 行之后）加：

```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description, category) VALUES
    ('env.huggingface', 'https://hf-mirror.com', 'HuggingFace 下载镜像地址', 'env'),
    ('env.modelscope',  'https://modelscope.cn',  '魔搭社区下载镜像地址',   'env'),
    ('env.github',      'https://github.com',     'GitHub 下载地址',         'env');
```

> 注意：现有 app_config seed INSERT 没有 category 列。需确认 db.sql 的 app_config CREATE TABLE 是否已有 category 列——如果没有需要 ALTER TABLE 或在 CREATE TABLE 中补上。检查现有 schema：

```bash
grep -A10 "CREATE TABLE.*app_config" crates/infra/src/db.sql
```

- [ ] **Step 2: db.rs 加 env 变量 CRUD 函数**

在 db.rs 中加（放在 `save_config_key` / `load_config_key` 之后）：

```rust
/// 列出所有 env 变量（category='env'），返回 (key, value) 列表。
/// key 去掉 `env.` 前缀（返回裸名如 "huggingface"）。
pub fn list_env_vars() -> Result<Vec<(String, String)>> {
    ensure_db()?;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT config_key, config_value FROM app_config WHERE category = 'env' ORDER BY config_key"
        )?;
        let rows = stmt.query_map([], |r| {
            let key: String = r.get(0)?;
            let value: String = r.get(1)?;
            let bare_key = key.strip_prefix("env.").unwrap_or(&key).to_string();
            Ok((bare_key, value))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    })
}

/// 保存 env 变量（自动加 `env.` 前缀 + category='env'）。
pub fn save_env_var(key: &str, value: &str) -> Result<()> {
    ensure_db()?;
    let full_key = format!("env.{}", key);
    with_db(|conn| {
        conn.execute(
            "INSERT INTO app_config (config_key, config_value, category) VALUES (?1, ?2, 'env')
             ON CONFLICT(config_key) DO UPDATE SET config_value = excluded.config_value",
            params![full_key, value],
        )?;
        Ok(())
    })
}

/// 删除 env 变量。内置 3 个（huggingface/modelscope/github）不可删，返回 Ok(false)。
pub fn delete_env_var(key: &str) -> Result<bool> {
    const BUILTIN: &[&str] = &["huggingface", "modelscope", "github"];
    if BUILTIN.contains(&key) {
        return Ok(false);
    }
    ensure_db()?;
    let full_key = format!("env.{}", key);
    with_db(|conn| {
        conn.execute("DELETE FROM app_config WHERE config_key = ?1 AND category = 'env'", params![full_key])?;
        Ok(true)
    })
}
```

- [ ] **Step 3: 编译 + 测试**

Run: `cargo build -p octopus-infra 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 4: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(models-tab): DB seed + env 变量 CRUD（list/save/delete）"
```

---

### Task 2: 下载链路改为变量模板替换

**Files:**
- Modify: `crates/desktop/src/model_commands.rs`

**Interfaces:**
- Consumes: Task 1 的 `list_env_vars()`

- [ ] **Step 1: 加变量替换函数**

在 model_commands.rs 中加：

```rust
/// 对 URL 字符串做 `{key}` → value 模板替换（读 DB env 变量）。
fn resolve_env_template(url: &str) -> String {
    let vars = match octopus_infra::db::list_env_vars() {
        Ok(v) => v,
        Err(_) => return url.to_string(),
    };
    let mut result = url.to_string();
    for (key, value) in vars {
        let placeholder = format!("{{{}}}", key);
        result = result.replace(&placeholder, &value);
    }
    result
}
```

- [ ] **Step 2: 改造 download_model**

在 `download_model` 函数中，找到 `source_url: mirror` 那段（约 line 114-125），改为：

```rust
    // 变量模板替换：repo/source 中的 {huggingface} 等替换为实际 URL
    let resolved_repo = resolve_env_template(&repo);
    // 旧 mirror 前缀逻辑废弃——env 变量替换更灵活
```

然后在 `HfRequest` 构造中用 `resolved_repo` 替代 `repo`，`source_url: None`（不再用前缀拼接）。

- [ ] **Step 3: 编译**

Run: `cargo build -p octopus-desktop 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/model_commands.rs
git commit -m "feat(models-tab): 下载链路改为 {huggingface} 等变量模板替换"
```

---

### Task 3: Tauri 命令注册

**Files:**
- Modify: `crates/desktop/src/settings_commands.rs`
- Modify: `crates/desktop/src/main.rs`

- [ ] **Step 1: 加 3 个 Tauri 命令**

在 settings_commands.rs 中加：

```rust
#[tauri::command]
pub fn get_env_vars() -> Result<Vec<(String, String)>, String> {
    octopus_infra::db::list_env_vars().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_env_var(key: String, value: String) -> Result<(), String> {
    octopus_infra::db::save_env_var(&key, &value).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_env_var_cmd(key: String) -> Result<bool, String> {
    octopus_infra::db::delete_env_var(&key).map_err(|e| e.to_string())
}
```

- [ ] **Step 2: main.rs invoke_handler 注册**

在 invoke_handler 中加：

```rust
            settings_commands::get_env_vars,
            settings_commands::set_env_var,
            settings_commands::delete_env_var_cmd,
```

- [ ] **Step 3: 编译**

Run: `cargo build -p octopus-desktop 2>&1 | tail -5`
Expected: 编译通过

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/settings_commands.rs crates/desktop/src/main.rs
git commit -m "feat(models-tab): get/set/delete_env_var Tauri 命令注册"
```

---

### Task 4: 前端 Tab 容器 + EnvironmentTab

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ModelsPanel.tsx`
- Create: `crates/desktop/frontend/src/pages/Settings/Models/EnvironmentTab.tsx`

- [ ] **Step 1: EnvironmentTab.tsx 组件**

```tsx
// crates/desktop/frontend/src/pages/Settings/Models/EnvironmentTab.tsx
import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { Plus, Trash2, Lock } from "lucide-react";

const BUILTIN = ["huggingface", "modelscope", "github"];

export default function EnvironmentTab({ showToast }: { showToast: (msg: string) => void }) {
  const [vars, setVars] = useState<[string, string][]>([]);
  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");

  const load = useCallback(async () => {
    try {
      const data = await invoke<[string, string][]>("get_env_vars");
      setVars(data);
    } catch (e) { showToast("加载环境变量失败：" + e); }
  }, [showToast]);

  useEffect(() => { load(); }, [load]);

  const handleSave = async (key: string, value: string) => {
    try {
      await invoke("set_env_var", { key, value });
      showToast("已保存");
      load();
    } catch (e) { showToast("保存失败：" + e); }
  };

  const handleDelete = async (key: string) => {
    try {
      const ok = await invoke<boolean>("delete_env_var_cmd", { key });
      if (ok) { showToast("已删除"); load(); }
      else showToast("内置变量不可删除");
    } catch (e) { showToast("删除失败：" + e); }
  };

  const handleAdd = async () => {
    if (!newKey.trim()) return;
    try {
      await invoke("set_env_var", { key: newKey.trim(), value: newValue.trim() });
      setNewKey(""); setNewValue("");
      showToast("已添加");
      load();
    } catch (e) { showToast("添加失败：" + e); }
  };

  return (
    <div className="space-y-3">
      <p className="text-xs text-muted-foreground">
        模型下载地址中的 {"{变量名}"} 会自动替换为此处配置的值。内置变量不可删除。
      </p>
      {vars.map(([key, value]) => (
        <div key={key} className="flex items-center gap-2">
          <div className="flex items-center gap-1 w-32 shrink-0">
            <span className="text-xs font-mono font-medium">{key}</span>
            {BUILTIN.includes(key) && <Lock className="w-3 h-3 text-muted-foreground/50" />}
          </div>
          <input
            className="flex-1 px-2 py-1 text-xs rounded border border-border bg-background"
            defaultValue={value}
            onBlur={(e) => { if (e.target.value !== value) handleSave(key, e.target.value); }}
          />
          {!BUILTIN.includes(key) && (
            <button
              className="p-1 rounded hover:bg-destructive/10 text-muted-foreground hover:text-destructive"
              onClick={() => handleDelete(key)}
            >
              <Trash2 className="w-3 h-3" />
            </button>
          )}
        </div>
      ))}
      <div className="flex items-center gap-2 pt-2 border-t border-border">
        <input
          className="w-32 px-2 py-1 text-xs rounded border border-border"
          placeholder="变量名"
          value={newKey}
          onChange={(e) => setNewKey(e.target.value)}
        />
        <input
          className="flex-1 px-2 py-1 text-xs rounded border border-border"
          placeholder="https://..."
          value={newValue}
          onChange={(e) => setNewValue(e.target.value)}
        />
        <button
          className="flex items-center gap-1 px-2 py-1 text-xs rounded bg-foreground/5 hover:bg-foreground/10"
          onClick={handleAdd}
        >
          <Plus className="w-3 h-3" /> 添加
        </button>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: ModelsPanel 改为 Tab 容器**

将 ModelsPanel.tsx 重构为：

```tsx
import { useState } from "react";
import { cn } from "@/lib/utils";
import EnvironmentTab from "./Models/EnvironmentTab";
import AsrTab from "./Models/AsrTab";
import LlmTab from "./Models/LlmTab";
import OcrTab from "./Models/OcrTab";
import TranslateTab from "./Models/TranslateTab";

const TABS = ["常量", "ASR", "Text/Chat", "OCR", "翻译"] as const;
type TabName = typeof TABS[number];

export default function ModelsPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [activeTab, setActiveTab] = useState<TabName>("常量");

  return (
    <div className="flex flex-col h-full">
      <div className="flex gap-1 border-b border-border px-2">
        {TABS.map((tab) => (
          <button
            key={tab}
            className={cn(
              "px-3 py-1.5 text-xs font-medium transition-colors border-b-2 -mb-px",
              activeTab === tab
                ? "text-voice border-voice"
                : "text-muted-foreground hover:text-foreground border-transparent",
            )}
            onClick={() => setActiveTab(tab)}
          >
            {tab}
          </button>
        ))}
      </div>
      <div className="flex-1 overflow-y-auto p-3">
        {activeTab === "常量" && <EnvironmentTab showToast={showToast} />}
        {activeTab === "ASR" && <AsrTab showToast={showToast} />}
        {activeTab === "Text/Chat" && <LlmTab showToast={showToast} />}
        {activeTab === "OCR" && <OcrTab showToast={showToast} />}
        {activeTab === "翻译" && <TranslateTab />}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: tsc 检查（先创建 stub 子组件让编译通过）**

为 AsrTab / LlmTab / OcrTab / TranslateTab 创建最简 stub（Task 5-7 填充）。每个文件：

```tsx
export default function XxxTab({ showToast }: { showToast: (msg: string) => void }) {
  return <div className="text-xs text-muted-foreground">（待实现）</div>;
}
```

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 errors

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/ModelsPanel.tsx \
        crates/desktop/frontend/src/pages/Settings/Models/
git commit -m "feat(models-tab): Tab 容器 + EnvironmentTab 环境变量编辑器"
```

---

### Task 5: AsrTab（现有 ASR 模型列表提取）

**Files:**
- Fill: `crates/desktop/frontend/src/pages/Settings/Models/AsrTab.tsx`

- [ ] **Step 1: 从 ModelsPanel 提取 ASR 模型列表逻辑**

将现有 ModelsPanel.tsx 的 `models` / `progress` / `busyRepo` / `handleDownload` / `handleDelete` / 模型列表渲染全部搬到 AsrTab.tsx。

顶部加「当前使用」模型（只读 select，从 `get_config` 读 `asr_engine`）。

- [ ] **Step 2: tsc 检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 errors

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/Models/AsrTab.tsx
git commit -m "feat(models-tab): AsrTab 从 ModelsPanel 提取 ASR 模型列表"
```

---

### Task 6: LlmTab + OcrTab（模型列表 + toggle）

**Files:**
- Fill: `crates/desktop/frontend/src/pages/Settings/Models/LlmTab.tsx`
- Fill: `crates/desktop/frontend/src/pages/Settings/Models/OcrTab.tsx`

- [ ] **Step 1: LlmTab.tsx**

```tsx
// 读取 get_config → llm_models（ConfigResponse.llm_models）
// 每行：name + label + current 标记 + is_enabled toggle（调 set_model_enabled）
// 顶部「当前使用」从 llm_models.find(m => m.current)
```

用现有 `invoke("set_model_enabled", { id, enabled })` 切换启用状态。

- [ ] **Step 2: OcrTab.tsx**

同 LlmTab 但数据源为 `ocr_models`，配置字段为 `ocr_model`。

- [ ] **Step 3: tsc 检查**

Run: `cd crates/desktop/frontend && npx tsc --noEmit`
Expected: 0 errors

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/Models/LlmTab.tsx \
        crates/desktop/frontend/src/pages/Settings/Models/OcrTab.tsx
git commit -m "feat(models-tab): LlmTab + OcrTab 模型列表 + 启用 toggle"
```

---

### Task 7: TranslateTab（占位）

**Files:**
- Fill: `crates/desktop/frontend/src/pages/Settings/Models/TranslateTab.tsx`

- [ ] **Step 1: TranslateTab.tsx**

```tsx
import { Languages } from "lucide-react";

export default function TranslateTab() {
  return (
    <div className="flex flex-col items-center justify-center h-full gap-2 text-muted-foreground/50">
      <Languages className="w-8 h-8" />
      <span className="text-xs">翻译模型配置即将支持</span>
      <span className="text-[10px]">未来可接入 DeepL / Google / 百度 / 本地 Argos 等翻译引擎</span>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/Models/TranslateTab.tsx
git commit -m "feat(models-tab): TranslateTab 占位页面"
```

---

## Spec Coverage 检查

| Spec 章节 | 对应 Task |
|-----------|----------|
| §2.1 Tab 1 常量 | Task 1 + Task 4 |
| §2.2 Tab 2 ASR | Task 5 |
| §2.3 Tab 3 Text/Chat | Task 6 |
| §2.4 Tab 4 OCR | Task 6 |
| §2.5 Tab 5 翻译 | Task 7 |
| §3 变量替换机制 | Task 2 |
| §3.3 迁移 | Task 2（旧 mirror 废弃，db.sql seed 替代） |
| §4 文件变更 | Task 1-7 全覆盖 |
| §6 不变式 | Global Constraints 覆盖 |
