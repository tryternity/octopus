# 模型激活语义重构设计

> 2026-07-17 · is_enabled 改表"激活"（每域仅 1），新增 is_available 表"可用"（替代原 is_enabled），删除 app_config 的 4 个激活字段

## 1. 背景与目标

### 1.1 现状问题

models 表的 `is_enabled` 字段语义混乱——它表"可用"（文件就绪/配置完整），同域可有多个=1。激活态由 `app_config` 的 4 个字段（asr_engine/polish_llm/ocr_model/translate_engine）单独指定。这导致：

1. **数据双源**：激活信息分散在 DB（is_enabled=可用集合）和 app_config（激活 spec），需要两边对齐
2. **RUNTIME_CONFIG 装太多**：ASR 的 RUNTIME_CONFIG 缓存所有 is_enabled=1 的可用模型（多个），而推理只需激活的那一个
3. **translate_engine 格式不一致**：最近改成存 DB id（`"42"`），而 asr_engine/polish_llm/ocr_model 存 spec 字符串（`"local:zipformer:xxx"`），4 个字段格式不统一

### 1.2 目标

- `is_enabled` 改表**激活**：每域仅 1 个为 1（当前选用的模型）
- 新增 `is_available` 表**可用**：文件就绪/配置完整（替代原 is_enabled 语义），同域可多个
- 删除 `app_config` 的 asr_engine/polish_llm/ocr_model/translate_engine 4 个字段
- 激活查询统一：`WHERE domain=? AND is_enabled=1 LIMIT 1`
- ASR RUNTIME_CONFIG 改为只缓存**激活的那一个** entry

## 2. 核心决策（brainstorming 已确认）

| 决策点 | 选择 |
|---|---|
| is_available 语义 | 文件就绪/配置完整（原 is_enabled 语义），同域可多个 |
| is_enabled 语义 | 当前激活，每域仅 1 个 |
| app_config 4 字段 | **删除**（asr_engine/polish_llm/ocr_model/translate_engine）|
| 激活查询 | `WHERE domain=? AND is_enabled=1 AND is_available=1 LIMIT 1` |
| 切换引擎 | `UPDATE models SET is_enabled=IF(id=?,1,0) WHERE domain=? AND is_available=1` |
| 两个核心方法 | `load_active_engine(domain)` 写缓存；`resolve_active_engine(domain)` 读缓存 |
| ResolvedEngine 通用化 | 加 domain 字段 + category 改 String，4 域共用 |
| ASR 热路径 | ACTIVE_ENGINES 缓存每域激活的那一个（推理零 DB 开销）|
| 实施范围 | 4 域一起改（ASR/LLM/OCR/Translate 结构统一）|
| 数据迁移 | 用户手工处理（开发期），只改 db.sql 新建脚本 |

## 3. DB schema 改动

### 3.1 models 表（db.sql）

```sql
CREATE TABLE IF NOT EXISTS models (
    ...
    is_available  INTEGER NOT NULL DEFAULT 0,   -- 可用：文件就绪/配置完整（原 is_enabled 语义）
    is_enabled    INTEGER NOT NULL DEFAULT 0,   -- 激活：每域仅 1 个为 1（当前选用）
    ...
);
```

- 原 `is_enabled` 的"可用"语义值迁移到 `is_available`（用户手工迁移数据）
- `is_enabled` 全部重置为 0（用户重新激活）
- seed 数据：is_available 按原 is_enabled 值填，is_enabled 全 0
- user_version 升级（v37）

### 3.2 app_config 表

删除 4 行 seed：
- `asr_engine`（category='setting'）
- `polish_llm`（category='setting'）
- `ocr_model`（category='setting'）
- `translate_engine`（category='setting'）

### 3.3 通用激活查询函数（db.rs 新增）

```rust
/// 查询指定域的激活模型（is_enabled=1 且 is_available=1），每域仅一个。
pub fn get_active_model(domain: &str) -> Result<Option<ModelRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, domain, provider, category, model_name, source, secret_key,
                    is_local, is_thinking, is_streaming, is_enabled, is_available
             FROM models WHERE domain=?1 AND is_enabled=1 AND is_available=1 LIMIT 1",
        )?;
        // ... map to ModelRow（加 is_available 字段）
    })
}

/// 切换激活模型——单语句全量刷新某域的 is_enabled（仅在可用模型中切换）。
pub fn switch_active_model(domain: &str, id: i64) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE models SET is_enabled = IF(id=?1, 1, 0) WHERE domain=?2 AND is_available=1",
            params![id, domain],
        )?;
        Ok(())
    })
}
```

## 4. 两个核心方法（统一所有路径）

整个系统收敛为**两个核心方法**，覆盖 4 个域的所有激活模型操作：

### 4.1 `load_active_engine(domain)` —— 写缓存（DB → 内存）

```rust
/// 从 DB 加载指定域的激活模型并缓存到内存。
/// 仅在启动时 + 管理页激活模型时调用。
pub fn load_active_engine(domain: &str) -> Result<ResolvedEngine> {
    // 读缓存命中则返回
    if let Some(arc) = ACTIVE_ENGINES.read().unwrap().get(domain) {
        return Ok(arc.as_ref().clone());
    }
    // 从 DB 取激活模型（is_enabled=1 AND is_available=1 LIMIT 1）
    let row = octopus_infra::db::get_active_model(domain)?
        .ok_or_else(|| anyhow!("域 {} 无激活模型", domain))?;
    let resolved = resolve_engine_from_row(domain, &row)?;
    ACTIVE_ENGINES.write().unwrap().insert(domain.to_string(), Arc::new(resolved.clone()));
    Ok(resolved)
}
```

- 缓存结构改为 `HashMap<domain, Arc<ResolvedEngine>>`（4 域各一个槽位）
- 调用时机：① 应用启动（main.rs 初始化 4 域）；② 设置页激活模型后（switch_active_model 之后调）

### 4.2 `resolve_active_engine(domain)` —— 读缓存（内存取唯一激活态）

```rust
/// 从内存缓存取指定域的唯一激活模型。
/// 各个使用方（推理 / tray / 管理页当前态 / 流式判定）都调此方法。
/// ASR 域带兜底引擎 fallback（zipformer-small-ctc），其余域无激活返回 Err。
pub fn resolve_active_engine(domain: &str) -> Result<ResolvedEngine> {
    if let Some(arc) = ACTIVE_ENGINES.read().unwrap().get(domain) {
        return Ok(arc.as_ref().clone());
    }
    // 缓存未命中（启动尚未 load / 被清）→ 走 load_active_engine 兜底
    load_active_engine(domain)
}
```

- **纯读缓存**（缓存未命中时 fallback 到 load_active_engine）
- 推理热路径零 DB 开销

### 4.3 调用方统一映射

| 场景 | 原实现 | 新实现 |
|---|---|---|
| **推理获取模型**（各引擎 transcribe / LLM polish / OCR / 翻译）| 读 cfg.asr_engine / load_llm_model(cfg.polish_llm) 等 | `resolve_active_engine(domain)` |
| **tray 显示引擎名** | `config.asr_engine` → fmt_engine_label | `resolve_active_engine("asr").name` |
| **管理页"当前使用"高亮** | build_*_options 比对 cfg.asr_engine 等 | `resolve_active_engine(domain).name` 比对 |
| **流式判定**（is_streaming_engine）| resolve_active_engine(&cfg.asr_engine) | `resolve_active_engine("asr").entry.is_streaming` |
| **设置页激活操作** | set_config asr_engine/polish_llm/... | `switch_active_model(domain, id)` + `load_active_engine(domain)` |

### 4.4 ResolvedEngine 通用化

原 `ResolvedEngine` 是 ASR 专用的（含 EngineCategory）。为支持 4 域统一，需要泛化：

```rust
pub struct ResolvedEngine {
    pub domain: String,        // 新增：asr/llm/ocr/translate
    pub name: String,          // model_name
    pub provider: String,
    pub category: String,      // 改为 String（原 EngineCategory 枚举仅 ASR 有意义）
    pub entry: ModelEntry,     // source/secret_key/is_streaming 等
}
```

- LLM/OCR/Translate 的 ResolvedEngine 的 category 就是 DB 的 category 字符串
- ASR 的 EngineCategory 枚举保留给 ASR 内部路由用（从 ResolvedEngine.category 字符串转换）

### 4.5 infra 层改动

- `ModelRow` 加 `is_available` 字段
- `get_active_model(domain)` / `switch_active_model(domain, id)`（§3.3）
- `AppConfig` 删 asr_engine/polish_llm/ocr_model/translate_engine 4 字段
- `load_llm_model(spec)` 废弃（被 `resolve_active_engine("llm")` 替代），或保留供 CLI `--model` 显式指定路径用
- `settings_commands.rs::apply_config_value` 删 4 个 case

### 4.6 切换引擎逻辑（4 域统一）

```rust
#[tauri::command]
pub fn switch_active_model(domain: String, id: i64, app: AppHandle) -> Result<(), String> {
    octopus_infra::db::switch_active_model(&domain, id).map_err(|e| e.to_string())?;
    // 重新加载该域的激活缓存
    octopus_asr_local::config::load_active_engine(&domain).map_err(|e| e.to_string())?;
    // ASR 域额外：emit config-changed 让 tray/UI 刷新
    if domain == "asr" {
        let _ = app.emit("config-changed", ());
    }
    Ok(())
}
```

原 `switch_asr_engine` / `switch_polish_llm` / `switch_ocr_model` 命令统一为此一个。

### 4.7 coordinator.rs 的 asr_engine 写回删除

`coordinator.rs:293,538` 有 `config.asr_engine = match resolve_active_engine(&rc.asr_engine)`——运行时校正写回。新方案 asr_engine 字段不存在，这两处删除（激活态已在 DB is_enabled + 内存缓存，无需写回 app_config）。

### 4.8 前端

- 4 个 Tab 的激活操作统一：`invoke("switch_active_model", {domain, id})`
- `current` 判定：后端返回的模型列表带 `is_enabled` 字段，前端直接用它标 current（不再比对 engineName）
- TranslateTab 的 `set_config({key:"translate_engine"...})` 改为 switch_active_model

## 5. 数据流（重构后）

```
应用启动：
  main.rs → load_active_engine("asr") + load_active_engine("llm")
           + load_active_engine("ocr") + load_active_engine("translate")
  → 4 域激活模型进 ACTIVE_ENGINES 内存缓存

用户在设置页激活某模型：
  → invoke("switch_active_model", {domain:"asr", id:42})
  → db::switch_active_model("asr", 42)
    UPDATE models SET is_enabled=IF(id=42,1,0) WHERE domain='asr' AND is_available=1
  → load_active_engine("asr")  // 重载该域缓存
  → emit config-changed（UI 刷新）

推理时（各使用方）：
  → resolve_active_engine("asr")  // 纯读内存缓存，零 DB 开销
  → resolve_active_engine("llm")  // 润色用
  → resolve_active_engine("ocr")  // OCR 用
  → resolve_active_engine("translate")  // 翻译用
```

## 6. 不变量

1. 推理正确性不变——激活引擎的配置内容（source/secret_key/model_name）不变，只是获取方式从"spec 查"改为"is_enabled=1 查"
2. RUNTIME_CONFIG 改为只缓存激活的一个——内存占用减小，且语义清晰
3. 切换引擎是事务性的（单 UPDATE 语句，原子）
4. 4 个域结构统一（同一个 switch_active_model + get_active_model）

## 7. 降级路径

| 场景 | 行为 |
|---|---|
| 某域无 is_enabled=1（未激活） | ASR fallback 兜底引擎（zipformer-small-ctc）；LLM/OCR/Translate 返回 None，调用方报错提示去设置页激活 |
| 激活模型 is_available=0（文件未就绪） | ASR fallback 兜底；其余报错 |
| DB 查询失败 | ASR 用缓存的 ACTIVE_ENGINE（旧值）；其余报错 |

## 8. 影响面

### 8.1 需改造的文件（按 crate）

**infra**（schema + 查询）：
- `db.sql`（models 表加 is_available + seed + user_version v37；app_config 删 4 seed）
- `db.rs`（ModelRow/ModelEntry 加 is_available；load_models_at 改 `is_enabled=1 AND is_available=1 LIMIT 1`；新增 get_active_model/switch_active_model；load_llm_model 废弃——被 resolve_active_engine("llm") 替代，仅 CLI `--model` 显式路径保留）
- `config.rs`（AppConfig 删 4 字段 + 相关 default/test）

**asr-local**（两个核心方法 + 推理路径）：
- `config.rs`（RUNTIME_CONFIG → ACTIVE_ENGINES: HashMap<domain, ResolvedEngine>；`load_active_engine(domain)` + `resolve_active_engine(domain)` 两个核心方法；ResolvedEngine 通用化加 domain 字段 + category 改 String；reload 改为按 domain 重载）
- `engine.rs`（AsrEngineManager 调用点适配 resolve_active_engine("asr")）
- 各引擎 transcribe 入口（whisper/paraformer/zipformer/qwen3/moonshine/sensevoice/firered——从 resolve_active_engine("asr").entry 取配置）

**desktop**（调用层，统一走 resolve_active_engine）：
- `config.rs`（`is_streaming_engine` 改 `resolve_active_engine("asr").entry.is_streaming`；`llm_config_ignore_mode` 改 `resolve_active_engine("llm")` 转 CompatibleLlmConfig）
- `settings_commands.rs`（get_config 的 asr_engines/llm_models/ocr_models current 判定改 resolve_active_engine + apply_config_value 删 4 case）
- `runtime_config.rs`（build_*_options current 用 resolve_active_engine(domain)；switch_* 命令统一为 switch_active_model）
- `model_commands.rs`（add/edit/remove cloud model 写 is_available）
- `action_bar_commands.rs`（翻译策略 resolve 用 resolve_active_engine("translate")）
- `translation_commands.rs`（translate_status 用 resolve_active_engine）
- `tray.rs`（`fmt_engine_label` 改读 resolve_active_engine("asr").name）
- `main.rs`（启动时 load_active_engine 4 域 + 命令注册 switch_active_model）
- `coordinator.rs`（**删除** asr_engine 写回逻辑：第 293,538 行 `config.asr_engine = match resolve_active_engine(...)`）

**cli**：
- `main.rs`（resolve_active_engine("asr") 无需 asr_engine 参数 + select_model 适配）

**server**：
- `main.rs`（resolve_active_engine("asr")）

**前端**：
- TranslateTab/AsrTab/LlmTab/OcrTab（激活操作改 switch_active_model）
- CloudModelForm（如涉及 is_enabled 写入）

### 8.2 不受影响
- 翻译引擎 trait / CloudLlmEngine（只改如何拿激活配置，不改引擎本身）
- 录音流程 / VAD / 剪贴板 / 命令面板 / 热词
- models 表其它字段（provider/category/model_name/source/secret_key 等不变）

## 9. 验证策略

1. **编译验证**：全 workspace `cargo build` 0 error 0 warning
2. **单元测试**：get_active_model / switch_active_model / load_active_engine 边界
3. **集成测试**：切换引擎后推理用新引擎；无激活时 fallback
4. **端到端**（手动）：设置页激活 ASR/LLM/OCR/Translate 模型 → 录音/润色/OCR/翻译验证
5. **回归**：现有 asr-local 125 + desktop 311 + infra 114 测试全过

## 10. 数据迁移（用户手工）

用户自行执行 SQL（开发期，不在代码里写迁移）：
```sql
-- 1. 加 is_available 列
ALTER TABLE models ADD COLUMN is_available INTEGER NOT NULL DEFAULT 0;
-- 2. 原 is_enabled 值（可用）迁移到 is_available
UPDATE models SET is_available = is_enabled;
-- 3. is_enabled 改为激活语义（全 0，用户重新激活）
UPDATE models SET is_enabled = 0;
-- 4. 删 app_config 的 4 个激活字段
DELETE FROM app_config WHERE config_key IN ('asr_engine','polish_llm','ocr_model','translate_engine');
```

db.sql 的全新库脚本和 user_version 升级由代码实现（新建库直接到新 schema）。
