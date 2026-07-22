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
- 激活查询统一：`WHERE domain=? AND is_enabled=1 AND is_available=1 LIMIT 1`
- ASR RUNTIME_CONFIG 改为只缓存**激活的那一个** entry

## 2. 核心决策（brainstorming 已确认）

| 决策点 | 选择 |
|---|---|
| is_available 语义 | 文件就绪/配置完整（原 is_enabled 语义），同域可多个 |
| is_enabled 语义 | 当前激活，每域仅 1 个 |
| app_config 4 字段 | **删除**（asr_engine/polish_llm/ocr_model/translate_engine）|
| 激活查询 | `WHERE domain=? AND is_enabled=1 AND is_available=1 LIMIT 1` |
| 切换引擎 | `UPDATE models SET is_enabled=IIF(id=?,1,0) WHERE domain=? AND is_available=1` |
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
            "UPDATE models SET is_enabled = IIF(id=?1, 1, 0) WHERE domain=?2 AND is_available=1",
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
// ACTIVE_ENGINES 用 LazyLock（HashMap::new 非 const，不能直接 static）
static ACTIVE_ENGINES: std::sync::LazyLock<RwLock<HashMap<String, Arc<ResolvedEngine>>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// 从 DB 加载指定域的激活模型并缓存到内存。
/// 缓存命中直接返回（不强制重读 DB）；ASR 域无激活 fallback 兜底引擎；其余域返回 Err。
pub fn load_active_engine(domain: &str) -> Result<ResolvedEngine> {
    if let Some(arc) = ACTIVE_ENGINES.read().unwrap().get(domain) {
        return Ok(arc.as_ref().clone());
    }
    crate::db::ensure_db()?;
    match octopus_infra::db::get_active_model(domain)? {
        Some(row) => {
            let resolved = resolved_engine_from_row(&row);
            ACTIVE_ENGINES.write().unwrap().insert(domain.to_string(), Arc::new(resolved.clone()));
            Ok(resolved)
        }
        None => {
            if domain == "asr" {
                let resolved = fallback_resolved_engine(); // zipformer-small 兜底
                ACTIVE_ENGINES.write().unwrap().insert(domain.to_string(), Arc::new(resolved.clone()));
                Ok(resolved)
            } else {
                anyhow::bail!("域 '{}' 无激活模型，请在设置页激活", domain)
            }
        }
    }
}

/// 重载指定域的激活缓存（清槽 + 重 load）。switch_active_model 写 DB 后调。
/// ASR 域同步 reload_models_config（引擎实例化路径用 load_config）保持两缓存一致。
pub fn reload_active_engine(domain: &str) -> Result<ResolvedEngine> {
    ACTIVE_ENGINES.write().unwrap().remove(domain);
    if domain == "asr" { reload_models_config(); }
    load_active_engine(domain)
}
```

- 缓存结构 `LazyLock<RwLock<HashMap<domain, Arc<ResolvedEngine>>>>`（4 域各一个槽位）
- `LazyLock` 而非 const：`HashMap::new()` 非 const 函数，不能直接 `static`
- 调用时机：① 应用启动（main.rs 初始化 4 域）；② 设置页激活模型后（switch_active_model 之后 reload_active_engine）
- ASR 域 fallback：`fallback_resolved_engine()` 优先 `load_config` 缓存的 zipformer-small，否则硬构造（`DEFAULT_ASR_MODEL_DIR`）

### 4.2 `resolve_active_engine(domain)` —— 读缓存（内存取唯一激活态）

```rust
/// 从内存缓存取指定域的唯一激活模型。
/// 各个使用方（推理 / tray / 管理页当前态 / 流式判定）都调此方法。
/// 纯读缓存——缓存未命中 fallback 到 load_active_engine（含 ASR 兜底）。
pub fn resolve_active_engine(domain: &str) -> Result<ResolvedEngine> {
    if let Some(arc) = ACTIVE_ENGINES.read().unwrap().get(domain) {
        return Ok(arc.as_ref().clone());
    }
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
| **管理页"当前使用"高亮** | build_*_options 比对 cfg.asr_engine 等 | build_*_options 直接用 DB 行 `is_enabled` 字段（每行自带激活态，不再外部传 current 字符串匹配 name——同名不同 provider 不串扰） |
| **流式判定**（is_streaming_engine）| resolve_active_engine(&cfg.asr_engine) | `resolve_active_engine("asr").entry.is_streaming` |
| **设置页激活操作** | set_config asr_engine/polish_llm/... | `switch_active_model(domain, id)` + `load_active_engine(domain)` |

### 4.4 ResolvedEngine 通用化

原 `ResolvedEngine` 是 ASR 专用的（含 EngineCategory）。为支持 4 域统一，需要泛化：

```rust
pub struct ResolvedEngine {
    pub domain: String,        // 新增：asr/llm/ocr/translate
    pub name: String,          // model_name
    pub provider: String,      // 新增：DB models.provider（local/aliyun/deepseek 等）
    pub category: String,      // 改为 String（原 EngineCategory 枚举仅 ASR 有意义）
    pub is_thinking: bool,     // 新增（Task 2）：DB models.is_thinking（LLM reasoning 模型标记）
                               // ModelEntry 不含此字段，故提升到 ResolvedEngine 顶层
    pub entry: ModelEntry,     // source/secret_key/language/is_streaming 等
}

impl ResolvedEngine {
    /// category 字符串 → EngineCategory（ASR 内部路由用）。
    /// 仅 ASR domain 有意义；LLM/OCR/Translate domain 返回 None。
    pub fn as_engine_category(&self) -> Option<EngineCategory> {
        resolve_category(&self.provider, &self.category)
    }
}
```

- LLM/OCR/Translate 的 ResolvedEngine 的 category 就是 DB 的 category 字符串
- ASR 的 EngineCategory 枚举保留给 ASR 内部路由用（从 ResolvedEngine.category 字符串转换）
- `is_thinking` 提升原因：`CompatibleLlmConfig`（LLM polish 用）需此字段，但 `ModelEntry` 不含——直接从 `ModelRow.is_thinking` 提升到 ResolvedEngine 顶层，避免 LLM 特化逻辑污染 ModelEntry

### 4.5 infra 层改动

- `ModelRow` 加 `is_available` 字段 + `language`/`description` 字段（Task 2a，供 load_active_engine 完整构造 ModelEntry）
- `get_active_model(domain)` / `switch_active_model(domain, id)`（§3.3）—— 两者均拆出 `_at` 裸连接版本供测试
- `get_asr_model_by_spec(provider, category, name)`（Task 5 新增）—— CLI `--model` 多模型路径用，查 ASR 域任意可用模型（不限激活）
- `AppConfig` 删 asr_engine/polish_llm/ocr_model/translate_engine 4 字段
- `load_llm_model(spec)` 保留（被 `resolve_active_engine("llm")` 替代，但保留供向后兼容 / 显式 spec 查询）
- `settings_commands.rs::apply_config_value` 删 4 个 case（Task 3）+ 删 set_config 的 `asr_engine` preheat 死代码分支（code review Issue #3）

### 4.6 切换引擎逻辑（4 域统一）

```rust
#[tauri::command]
pub fn switch_active_model(
    domain: String,
    id: i64,
    app_handle: tauri::AppHandle,
    engine_manager: State<'_, std::sync::Arc<octopus_asr_local::engine::AsrEngineManager>>,
) -> Result<(), String> {
    octopus_infra::db::switch_active_model(&domain, id).map_err(|e| e.to_string())?;
    // 重载该域激活缓存（reload_active_engine 清槽 + 重 load）。
    // 并发不变量（code review Issue #2）：DB 是真相源，reload 从 DB 读回（而非入参 id）。
    if let Err(e) = octopus_asr_local::config::reload_active_engine(&domain) {
        log::warn!("switch_active_model: reload_active_engine('{}') 失败：{}", domain, e);
    }
    // ASR 域额外：刷新 tray + 后台预热 + emit 事件
    if domain == "asr" {
        // ... tray::update_tray_engine_label + preheat_local_engine + emit config-changed
    }
    Ok(())
}
```

**id=-1 特殊语义（LLM「不选择模型」）**：前端 LlmTab.tsx 传 `id=-1` 取消激活。SQL
`UPDATE ... SET is_enabled=IIF(id=-1,1,0) WHERE domain='llm' AND is_available=1` ——
SQLite AUTOINCREMENT 不产生负 id，故 IIF 无匹配行，该域所有 is_available=1 行 is_enabled=0
（全清空）。回归测试 `switch_active_model_with_id_neg1_clears_domain`。

**原 switch_asr_engine / switch_polish_llm 保留为 wrapper**（Task 6 前端已迁移到
switch_active_model，wrapper 仅遗留兼容）：按 name 查 DB 取 id → 调 switch_active_model。
后续可在 follow-up 清理。

### 4.7 coordinator.rs 的 asr_engine 写回删除

`coordinator.rs:293,538` 有 `config.asr_engine = match resolve_active_engine(&rc.asr_engine)`——运行时校正写回。新方案 asr_engine 字段不存在，这两处删除（激活态已在 DB is_enabled + 内存缓存，无需写回 app_config）。

### 4.8 前端

- 4 个 Tab 的激活操作统一：`invoke("switch_active_model", {domain, id})`
- `current` 判定：后端返回的模型列表带 `is_enabled` 字段，前端直接用它标 current（不再比对 engineName）
- TranslateTab 的 `set_config({key:"translate_engine"...})` 改为 switch_active_model
- LlmTab「不选择模型」传 `id=-1`（取消激活，详见 §4.6）

## 5. 数据流（重构后）

```
应用启动：
  main.rs → load_active_engine("asr") + load_active_engine("llm")
           + load_active_engine("ocr") + load_active_engine("translate")
  → 4 域激活模型进 ACTIVE_ENGINES 内存缓存

用户在设置页激活某模型：
  → invoke("switch_active_model", {domain:"asr", id:42})
  → db::switch_active_model("asr", 42)
    UPDATE models SET is_enabled=IIF(id=42,1,0) WHERE domain='asr' AND is_available=1
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
| 某域无 is_enabled=1（未激活） | ASR fallback 兜底引擎（zipformer-small）；LLM/OCR/Translate 返回 None，调用方报错提示去设置页激活 |
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
- `settings_commands.rs`（get_config 的 asr_engines/llm_models/ocr_models current 判定：build_*_options 内部直接读 DB 行 is_enabled，不再外部传 current 字符串 + apply_config_value 删 4 case）
- `runtime_config.rs`（build_*_options 去掉 current 参数，用 DB 行 is_enabled 标 current；switch_* 命令统一为 switch_active_model）
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

## 10. 数据迁移（v37 自动迁移，code review Issue #1 修复）

**最终实现**：v36→v37 迁移在 `init_schema` 中**自动完成**（对齐原计划的手工 SQL），用户无需手工干预。
迁移块（`crates/infra/src/db.rs` v37 段）：

```sql
-- 1. 旧库补 is_available 列（db.sql 新库已含，仅 ALTER 给 v36 旧库）
ALTER TABLE models ADD COLUMN is_available INTEGER NOT NULL DEFAULT 0;
-- 2. 原 is_enabled 值（旧「可用」语义）迁移到 is_available
UPDATE models SET is_available = is_enabled;
-- 3. is_enabled 重置为 0（新语义=激活，用户重新激活才设；保证「每域仅 1」不变量不被旧库多 is_enabled=1 破坏）
UPDATE models SET is_enabled = 0;
-- 4. 删 app_config 的 4 个废弃激活字段
DELETE FROM app_config WHERE config_key IN ('asr_engine','polish_llm','ocr_model','translate_engine');
```

**为什么必须自动迁移**（code review Issue #1）：原计划「用户手工迁移」有隐患——若用户仅升级 app 不执行手工 SQL，
旧库 is_enabled=1 的多行（旧「可用」语义）会保留，用户重新激活新模型时 `switch_active_model` 的
`UPDATE ... WHERE is_available=1` 因旧库 is_available=0 而影响 0 行，旧 is_enabled=1 不被清空。
后续用户把某模型 is_available 置 1 时，该域出现多 is_enabled=1 AND is_available=1 行，违反 §6.1
核心不变量。自动迁移消除此隐患。

回归测试：`migration_v36_to_v37_migrates_is_enabled_semantics_and_clears_activation`（infra db tests）。

**原计划（已废弃，保留作历史参考）**：用户自行执行 SQL（开发期，不在代码里写迁移）。

db.sql 的全新库脚本和 user_version 升级由代码实现（新建库直接到新 schema）。
