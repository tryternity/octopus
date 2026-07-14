# 云端模型管理 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 移除所有云端模型 seed，改为用户自行添加/编辑/删除 LLM 润色模型和云端 ASR 模型。通过 app_config 表存储各 provider 的参考模型列表。

**Architecture:** DB v31 删除云端模型 seed + 新增 app_config 参考列表。新增 DB CRUD 函数 + Tauri 命令。前端新增 CloudModelForm 弹窗组件 + 各 Tab 云端 section 加「添加/编辑/删除」按钮。

**Tech Stack:** Rust, SQLite, Tauri 2, React/TypeScript

**Spec:** `docs/superpowers/specs/2026-07-14-cloud-model-management-design.md`

## Global Constraints

- 移除 DB seed 中所有 `domain='asr' AND is_local=0` 和 `domain='llm'` 行
- 参考模型列表存 `app_config` 表：`category='asr_cloud_model'`（ASR）和 `category='llm_provider'`（LLM base_url）
- ASR provider 固定配置（source 端点/字段语义）写在 spec §2.3 表中，前端按 provider+category 自动填 source
- `models` 表物理增删改（不软删）
- 本地模型 seed（`is_local=1`）不动
- 现有各 provider 协议代码不变

---

### Task 1: DB v31 — 删除云端模型 seed + 新增参考列表 seed

**Files:**
- Modify: `crates/infra/src/db.sql`（删除云端 ASR + LLM INSERT，加 app_config 参考列表 seed）
- Modify: `crates/infra/src/db.rs`（v30→v31 迁移）

**Interfaces:**
- Produces: DB v31，`models` 表无云端模型，`app_config` 表含 `asr_cloud_model` + `llm_provider` 参考数据

- [ ] **Step 1: db.sql 删除云端 ASR seed**

删除 db.sql 中所有 `domain='asr' AND is_local=0` 的 INSERT 语句块（aliyun/bytedance/tencent/baidu 云端 ASR）。

- [ ] **Step 2: db.sql 删除 LLM seed**

删除 db.sql 中 `domain='llm'` 的 INSERT 语句块。

- [ ] **Step 3: db.sql 加 app_config 参考列表 seed**

在 env 变量 seed 之后加：

```sql
-- ── 云端 ASR 参考模型列表（category='asr_cloud_model'）──
INSERT OR IGNORE INTO app_config (config_key, config_value, description, category) VALUES
('aliyun:Fun-ASR', 'fun-asr-realtime;fun-asr-realtime-2026-02-28;fun-asr-realtime-2025-11-07;fun-asr-flash-8k-realtime;fun-asr-flash-8k-realtime-2026-01-28', '阿里云 FunASR 实时模型列表', 'asr_cloud_model'),
('aliyun:Paraformer', 'paraformer-realtime-v1;paraformer-realtime-v2;paraformer-realtime-8k-v1;paraformer-realtime-8k-v2', '阿里云 Paraformer 实时模型列表', 'asr_cloud_model'),
('aliyun:Qwen-ASR', 'qwen3-asr-flash-realtime;qwen3-asr-flash-realtime-2026-02-10;qwen3-asr-flash-realtime-2025-10-27', '阿里云 Qwen3-ASR Realtime 模型列表', 'asr_cloud_model'),
('bytedance:Doubao-ASR', 'doubao-asr-1.0-streaming', '火山引擎豆包 ASR 1.0', 'asr_cloud_model'),
('bytedance:Doubao-ASR-2.0', 'doubao-asr-2.0-streaming;seedasr-2.0-streaming', '火山引擎豆包 ASR 2.0', 'asr_cloud_model'),
('tencent:Tencent-ASR', '16k_zh;16k_zh_large;16k_zh-PY;16k_zh-TW;16k_yue;16k_zh_dialect;16k_wuu-SH', '腾讯云实时语音识别中文引擎', 'asr_cloud_model'),
('tencent:Tencent-ASR-Multi', '16k_zh_en;16k_multi_lang;16k_en;16k_en_large', '腾讯云实时语音识别多语种引擎', 'asr_cloud_model'),
('baidu:Baidu-ASR', '15372;15376;1537', '百度实时语音识别中文模型（dev_pid）', 'asr_cloud_model'),
('baidu:Baidu-ASR-EN', '17372;1737', '百度实时语音识别英文模型（dev_pid）', 'asr_cloud_model');

-- ── LLM provider 预设 base_url（category='llm_provider'）──
INSERT OR IGNORE INTO app_config (config_key, config_value, description, category) VALUES
('deepseek', 'https://api.deepseek.com/', 'DeepSeek API', 'llm_provider'),
('aliyun', 'https://dashscope.aliyuncs.com/compatible-mode/v1', '阿里云 DashScope', 'llm_provider'),
('bigmodel', 'https://open.bigmodel.cn/api/paas/v4', '智谱 BigModel', 'llm_provider'),
('openai', 'https://api.openai.com/v1', 'OpenAI', 'llm_provider'),
('ollama', 'http://localhost:11434/v1', 'Ollama 本地', 'llm_provider');
```

- [ ] **Step 4: db.rs v30→v31 迁移**

```rust
// v30→v31：删除云端模型 seed + 新增参考列表
{
    // 删除旧云端模型（用户改为自建）
    conn.execute("DELETE FROM models WHERE domain='asr' AND is_local=0", [])?;
    conn.execute("DELETE FROM models WHERE domain='llm'", [])?;
    // 重跑 INIT_SQL 确保参考列表 seed 已建（幂等）
    conn.execute_batch(INIT_SQL).ok();
    conn.execute("PRAGMA user_version = 31", [])?;
    log::info!("schema upgraded to v31 (cloud models: remove seed, add presets)");
}
```

更新 `if v >= 30` → `if v >= 31`，全新库 `PRAGMA user_version = 31`。

- [ ] **Step 5: 更新测试断言 v30→v31**

所有 `assert_eq!(v, 30)` → `assert_eq!(v, 31)`。

- [ ] **Step 6: 运行测试**

Run: `cargo test -p octopus-infra`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat: DB v31 — remove cloud model seed, add reference presets"
```

---

### Task 2: DB CRUD 函数 + Tauri 命令

**Files:**
- Modify: `crates/infra/src/db.rs`（新增 CRUD 函数）
- Modify: `crates/desktop/src/model_commands.rs`（新增 Tauri 命令）
- Modify: `crates/desktop/src/main.rs`（注册命令）

**Interfaces:**
- Produces: `insert_cloud_model` / `update_cloud_model` / `delete_cloud_model` / `list_asr_cloud_presets` / `list_llm_provider_presets`

- [ ] **Step 1: DB CRUD 函数**

在 `db.rs` 中新增：

```rust
/// 新增云端模型到 models 表。
pub fn insert_cloud_model(
    domain: &str, provider: &str, category: &str,
    model_name: &str, source: &str, secret_key: &str,
    is_streaming: bool, is_thinking: bool,
) -> Result<i64>

/// 更新云端模型（按 id）。
pub fn update_cloud_model(
    id: i64, provider: &str, category: &str,
    model_name: &str, source: &str, secret_key: &str,
    is_streaming: bool, is_thinking: bool,
) -> Result<()>

/// 删除云端模型（物理删除，按 id）。
pub fn delete_cloud_model(id: i64) -> Result<()>

/// 读取 ASR 云端参考模型列表。
/// 返回 Vec<(provider, category, "model1;model2;...")>
pub fn list_asr_cloud_presets() -> Result<Vec<(String, String, String)>>

/// 读取 LLM provider 预设 base_url。
/// 返回 Vec<(provider, base_url)>
pub fn list_llm_provider_presets() -> Result<Vec<(String, String)>>
```

- [ ] **Step 2: Tauri 命令**

在 `model_commands.rs` 中新增对应 `#[tauri::command]` 包装。

返回类型用 Serialize struct：

```rust
#[derive(Serialize)]
pub struct AsrCloudPreset {
    pub provider: String,
    pub category: String,
    pub models: Vec<String>,  // 分号拆分后的列表
}

#[derive(Serialize)]
pub struct LlmProviderPreset {
    pub provider: String,
    pub base_url: String,
}
```

- [ ] **Step 3: 注册命令**

在 `main.rs` invoke_handler 中注册 5 个新命令。

- [ ] **Step 4: 编译 + 测试**

Run: `cargo build -p octopus-desktop --features embedded && cargo test -p octopus-infra`
Expected: PASS

- [ ] **Step 5: Commit**

---

### Task 3: 前端 CloudModelForm 组件

**Files:**
- Create: `crates/desktop/frontend/src/pages/Settings/Models/CloudModelForm.tsx`

**Interfaces:**
- Produces: `CloudModelForm` 组件（Modal 弹窗），props: `{ domain: "asr" | "llm", onSaved: () => void, onCancel: () => void, editModel?: CloudModel | null }`

- [ ] **Step 1: 创建 CloudModelForm.tsx**

组件功能：
- `domain="llm"` 时：provider 下拉（从 `list_llm_provider_presets` 读）→ base_url 自动填 → model_name input → api_key password → is_stream/is_thinking checkbox
- `domain="asr"` 时：provider 下拉 → category 下拉（按 provider 过滤 `list_asr_cloud_presets`）→ source 只读自动填 → model_name datalist（参考项+自由输入）→ api_key input（label 按 provider 变化）
- 编辑模式：预填已有值
- 保存：调 `insert_cloud_model` 或 `update_cloud_model`

ASR provider 固定配置（source + secret_key label）硬编码在前端：

```typescript
const ASR_PROVIDER_CONFIG: Record<string, Record<string, { source: string; keyLabel: string }>> = {
  aliyun: {
    "Fun-ASR": { source: "wss://dashscope.aliyuncs.com/api-ws/v1/inference", keyLabel: "DashScope API Key" },
    "Paraformer": { source: "wss://dashscope.aliyuncs.com/api-ws/v1/inference", keyLabel: "DashScope API Key" },
    "Qwen-ASR": { source: "wss://dashscope.aliyuncs.com/api-ws/v1/realtime", keyLabel: "DashScope API Key" },
  },
  bytedance: {
    "Doubao-ASR": { source: "volc.bigasr.sauc.duration", keyLabel: "火山引擎 API Key" },
    "Doubao-ASR-2.0": { source: "volc.seedasr.sauc.duration", keyLabel: "火山引擎 API Key" },
  },
  tencent: {
    "Tencent-ASR": { source: "{appid}:{secretid}", keyLabel: "腾讯云 SecretKey" },
    "Tencent-ASR-Multi": { source: "{appid}:{secretid}", keyLabel: "腾讯云 SecretKey" },
  },
  baidu: {
    "Baidu-ASR": { source: "{appid}", keyLabel: "百度 API Key (appkey)" },
    "Baidu-ASR-EN": { source: "{appid}", keyLabel: "百度 API Key (appkey)" },
  },
};
```

- [ ] **Step 2: 前端构建**

Run: `cd crates/desktop/frontend && npm run build`
Expected: PASS

- [ ] **Step 3: Commit**

---

### Task 4: 前端 LlmTab/AsrTab 集成

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/LlmTab.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/AsrTab.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/ModelRow.tsx`（加编辑/删除按钮回调）

- [ ] **Step 1: LlmTab 云端 section 加「添加」按钮 + CloudModelForm**

```tsx
// 云端 section 标题栏右侧加「+ 添加模型」按钮
// 点击 → setShowForm(true) → <CloudModelForm domain="llm" ... />
// 每个云端模型行加「编辑」「删除」按钮
```

- [ ] **Step 2: AsrTab 云端 section 同理**

```tsx
// 云端 section 标题栏右侧加「+ 添加模型」按钮
// 点击 → setShowForm(true) → <CloudModelForm domain="asr" ... />
// 每个云端模型行加「编辑」「删除」按钮
```

- [ ] **Step 3: ModelRow 扩展——云端模型显示编辑/删除按钮**

ModelRow 已有 `onDelete`，新增 `onEdit` 回调。云端模型（`is_local=false`）显示编辑/删除，不显示下载/校验。

- [ ] **Step 4: 前端构建**

Run: `cd crates/desktop/frontend && npm run build`
Expected: PASS

- [ ] **Step 5: Commit**

---

### Task 5: 删除当前激活模型时的回退处理

**Files:**
- Modify: `crates/desktop/src/model_commands.rs`（delete_cloud_model 检查是否为当前引擎）

- [ ] **Step 1: delete_cloud_model 检查当前激活**

删除前检查：如果被删的模型是当前 `asr_engine` / `polish_llm`，需要回退。

```rust
pub async fn delete_cloud_model(id: i64, rc: State<'_, SharedRuntimeConfig>) -> Result<(), String> {
    // 查被删模型的 spec
    let model = ...; // 从 DB 按 id 查
    let spec = format!("{}:{}:{}", model.provider, model.category, model.model_name);
    // 如果是当前引擎，回退
    let current_asr = rc.read().asr_engine.clone();
    if current_asr.contains(&model.model_name) {
        // 回退到兜底引擎
        *rc.write() 的 asr_engine = "local:zipformer:zipformer-small-ctc";
    }
    let current_llm = rc.read().polish_llm.clone();
    if current_llm.contains(&model.model_name) {
        *rc.write() 的 polish_llm = "";
    }
    // 物理删除
    octopus_infra::db::delete_cloud_model(id).map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 2: 编译 + 测试**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: PASS

- [ ] **Step 3: Commit**

---

### Task 6: 全量编译 + 测试 + 文档同步

**Files:**
- Modify: `docs/architecture.md`
- Modify: `docs/superpowers/specs/2026-07-14-cloud-model-management-design.md`（偏差记录）

- [ ] **Step 1: 全量编译**

Run: `cargo build --release -p octopus-server -p octopus-cli -p octopus-desktop --features embedded`
Expected: PASS

- [ ] **Step 2: 全量测试**

Run: `cargo test -p octopus-infra -p octopus-translation`
Expected: PASS

- [ ] **Step 3: 更新 architecture.md**

更新模型管理章节，描述云端模型用户自管理（无 seed、参考列表存 app_config、CRUD 命令）。

- [ ] **Step 4: Commit**
