# 模型激活语义重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** is_enabled 改表"激活"（每域仅 1），新增 is_available 表"可用"，删除 app_config 的 4 个激活字段，4 域统一为 `load_active_engine(domain)` + `resolve_active_engine(domain)` 两个核心方法。

**Architecture:** RUNTIME_CONFIG 从"缓存 ASR 可用模型集合"改为"缓存每域激活的那一个 ResolvedEngine"；激活查询 `WHERE domain=? AND is_enabled=1 AND is_available=1 LIMIT 1`；切换 `UPDATE SET is_enabled=IF(id=?,1,0) WHERE domain=? AND is_available=1`。

**Spec:** [2026-07-17-model-activation-refactor-design.md](../specs/2026-07-17-model-activation-refactor-design.md)

## Global Constraints

- `is_enabled` = 激活（每域仅 1 个=1）；`is_available` = 可用（文件就绪/配置完整，同域可多个）
- 删除 AppConfig 的 asr_engine/polish_llm/ocr_model/translate_engine 4 字段
- 两个核心方法：`load_active_engine(domain)` 写缓存；`resolve_active_engine(domain)` 读缓存
- ResolvedEngine 通用化：加 domain 字段 + category 改 String
- ACTIVE_ENGINES: `HashMap<String, Arc<ResolvedEngine>>`（4 域各一个槽）
- 数据迁移用户手工（只改 db.sql 新建脚本 + user_version）
- 推理正确性不变（引擎配置内容不变，只改获取方式）

---

## Task 1: infra 层 schema + 查询函数

**Files:**
- Modify: `crates/infra/src/db.sql`（models 表加 is_available + seed + user_version v37；app_config 删 4 seed）
- Modify: `crates/infra/src/db.rs`（ModelRow/ModelEntry 加 is_available；load_models_at 改 LIMIT 1 + AND is_available=1；新增 get_active_model/switch_active_model）
- Modify: `crates/infra/src/config.rs`（AppConfig 删 asr_engine/polish_llm/ocr_model/translate_engine + default/test）

**Interfaces:**
- Produces: `get_active_model(domain) -> Result<Option<ModelRow>>`、`switch_active_model(domain, id) -> Result<()>`、`ModelRow.is_available`、`ModelEntry.is_available`

- [x] **Step 1: db.sql models 表加 is_available 列**

在 `is_enabled` 行后加 `is_available`，调整 seed（原 is_enabled 值表示可用→迁到 is_available，is_enabled 全 0）。user_version 升 v37。app_config seed 删 asr_engine/polish_llm/ocr_model/translate_engine 4 行。

- [x] **Step 2: db.rs ModelRow + ModelEntry 加 is_available**

ModelRow 和 ModelEntry 各加 `pub is_available: bool` 字段。所有构造点和 SELECT 语句补该列。

- [x] **Step 3: db.rs 新增 get_active_model + switch_active_model**

```rust
pub fn get_active_model(domain: &str) -> Result<Option<ModelRow>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, domain, provider, category, model_name, source, secret_key,
                    is_local, is_thinking, is_streaming, is_enabled, is_available
             FROM models WHERE domain=?1 AND is_enabled=1 AND is_available=1 LIMIT 1",
        )?;
        stmt.query_row(params![domain], |r| Ok(ModelRow { ... })).optional()
    })
}

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

- [x] **Step 4: load_models_at 改为只取激活的**

SQL 加 `AND is_available=1 LIMIT 1`（原 `WHERE domain='asr' AND is_enabled=1` 改为新语义）。

- [x] **Step 5: config.rs AppConfig 删 4 字段**

删除 asr_engine/polish_llm/ocr_model/translate_engine 字段 + default 函数 + 相关 test。注意 default_polish_llm/default_ocr_model 函数删除。

- [x] **Step 6: 编译 + 测试**

Run: `cargo build -p octopus-infra && cargo test -p octopus-infra --lib`
Expected: 编译会有大量下游报错（desktop/cli/asr-local 读这 4 字段）——**本 Task 只要求 infra 自身编译通过**（infra 不依赖下游）。infra 测试全过。

- [x] **Step 7: Commit**

---

## Task 2: asr-local 两个核心方法 + ResolvedEngine 通用化

**Files:**
- Modify: `crates/asr-local/src/config.rs`（RUNTIME_CONFIG → ACTIVE_ENGINES；load_active_engine/resolve_active_engine 带 domain；ResolvedEngine 通用化）

**Interfaces:**
- Consumes: Task 1 的 get_active_model
- Produces: `load_active_engine(domain) -> Result<ResolvedEngine>`、`resolve_active_engine(domain) -> Result<ResolvedEngine>`、通用化 `ResolvedEngine { domain, name, provider, category: String, entry }`

- [x] **Step 1: ResolvedEngine 通用化**

```rust
pub struct ResolvedEngine {
    pub domain: String,        // "asr"/"llm"/"ocr"/"translate"
    pub name: String,
    pub provider: String,
    pub category: String,      // DB category 字符串（ASR 内部按需转 EngineCategory）
    pub entry: ModelEntry,
}
```

加 `as_engine_category()` helper（ASR 专用，category 字符串→EngineCategory）。

- [x] **Step 2: ACTIVE_ENGINES 替换 RUNTIME_CONFIG**

```rust
static ACTIVE_ENGINES: RwLock<HashMap<String, Arc<ResolvedEngine>>> = RwLock::new(HashMap::new());
```

- [x] **Step 3: load_active_engine(domain)**

从 DB get_active_model(domain) → 构造 ResolvedEngine → 写 ACTIVE_ENGINES。缓存命中直接返回。

- [x] **Step 4: resolve_active_engine(domain)**

读 ACTIVE_ENGINES。缓存未命中 fallback 到 load_active_engine。ASR 域无激活时 fallback 兜底引擎（zipformer-small-ctc），其余域返回 Err。

- [x] **Step 5: load_config 的处理（关键决策）**

`load_config()`（返回 AsrConfig 集合）当前服务于两个用途：
- (a) 推理取激活引擎配置——被 resolve_active_engine / 各引擎 transcribe 用
- (b) CLI `--model xxx` / server 显式指定引擎——按 name 查任意引擎

**决策**：
- (a) 由新的 `resolve_active_engine("asr")` 取代——各引擎 transcribe 的 entry 从 ResolvedEngine.entry 取（AsrEngineManager::switch_model 已缓存实例，transcribe 不再重复 load_config）
- (b) 保留 `load_config()` 但改为查**全量可用模型**（`is_available=1`，不过滤 is_enabled）——供 CLI 显式指定路径用。或新增 `get_model_by_name(domain, name)` 直查 DB

**最简方案**：保留 load_config 不动其内部逻辑（仍查 is_enabled=1），但语义改为"激活引擎配置"。各引擎 transcribe 实际只被 AsrEngineManager::switch_model 调（它传激活的 name），所以 load_config 只含激活的那一个也能命中。CLI 显式路径如需查非激活引擎，用 get_model_by_name 新函数。

实现时先保留 load_config 原样，观察是否编译通过（各引擎调用链是否都经 AsrEngineManager 缓存实例）。如果报错再调整。

`reload_models_config` 改名为 `reload_active_engine(domain)`——清 ACTIVE_ENGINES 该域槽 + 重新 load。

- [x] **Step 6: 编译 asr-local**

Run: `cargo build -p octopus-asr-local`
Expected: 0 error（下游 desktop/cli 报错预期，Task 3+ 修）。

- [x] **Step 7: 测试 + Commit**

Run: `cargo test -p octopus-asr-local --lib`

---

## Task 3: desktop 调用层（config.rs / coordinator / tray / settings / runtime_config）

**Files:**
- Modify: `crates/desktop/src/config.rs`（is_streaming_engine / llm_config 改 resolve_active_engine）
- Modify: `crates/desktop/src/coordinator.rs`（删 asr_engine 写回 293/538；begin_recording 改 resolve_active_engine）
- Modify: `crates/desktop/src/tray.rs`（fmt_engine_label 改 resolve_active_engine("asr")）
- Modify: `crates/desktop/src/settings_commands.rs`（apply_config_value 删 4 case；build_*_options current 用 is_enabled）
- Modify: `crates/desktop/src/runtime_config.rs`（switch_* 统一 switch_active_model；build_*_options current 改）
- Modify: `crates/desktop/src/main.rs`（启动 load_active_engine 4 域 + switch_model 用 resolve_active_engine）

- [x] **Step 1: config.rs is_streaming_engine + llm_config 改造**

```rust
pub fn is_streaming_engine() -> bool {  // 删 cfg 参数
    match octopus_asr_local::config::resolve_active_engine("asr") {
        Ok(r) => r.entry.is_streaming && r.category != "Fun-ASR",  // category 字符串比较
        Err(_) => false,
    }
}

pub fn llm_config_ignore_mode() -> Option<CompatibleLlmConfig> {  // 删 cfg 参数
    match octopus_asr_local::config::resolve_active_engine("llm") {
        Ok(r) => Some(CompatibleLlmConfig { provider: r.provider, model: r.name, base_url: r.entry.source, secret_key: r.entry.secret_key, ... }),
        Err(_) => None,
    }
}
```

- [x] **Step 2: coordinator.rs 删 asr_engine 写回**

第 293-296、537-541 行 `config.asr_engine = match resolve_active_engine(...)` 整块删除。下游 begin_recording 改为不依赖 config.asr_engine（用 resolve_active_engine("asr") 按需取）。

- [x] **Step 3: tray.rs fmt_engine_label 改**

`fmt_engine_label(spec)` 改为 `fmt_engine_label()` 无参，内部 `resolve_active_engine("asr").name`。

- [x] **Step 4: settings_commands.rs apply_config_value 删 4 case + current 判定**

删 asr_engine/polish_llm/ocr_model/translate_engine 的 set_config case。build_*_options 的 current 判定改为比对 DB 行 is_enabled 字段。

- [x] **Step 5: runtime_config.rs switch_* 统一**

switch_asr_engine/switch_polish_llm 改为内部调 `switch_active_model(domain, id)` + `load_active_engine(domain)`。或新增统一 `switch_active_model` Tauri 命令。

- [x] **Step 6: main.rs 启动 + switch_model**

启动时 `load_active_engine("asr/llm/ocr/translate")`。`em.switch_model(&active_model)` 的 active_model 改为 `resolve_active_engine("asr").name`。

- [x] **Step 7: 编译 + 测试**

Run: `cargo build -p octopus-desktop --features embedded && cargo test -p octopus-desktop --bin octopus-desktop`

- [x] **Step 8: Commit**

---

## Task 4: 翻译/OCR 使用路径 + action_bar_commands

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`（翻译策略 resolve 用 resolve_active_engine("translate")）
- Modify: `crates/desktop/src/translation_commands.rs`（translate_status 用 resolve_active_engine）
- Modify: `crates/desktop/src/model_commands.rs`（add/edit/remove cloud model 写 is_available）

- [x] **Step 1: action_bar_commands resolve_translate_strategy 改**

从 `config.translate_engine.parse::<i64>()` + get_model_by_id 改为 `resolve_active_engine("translate")`。

- [x] **Step 2: translation_commands translate_status 改**

从读 config.polish_llm 改为 resolve_active_engine。

- [x] **Step 3: model_commands add/edit cloud model 写 is_available**

insert_cloud_model/update_cloud_model 的 is_available 写入（原 is_enabled 语义）。is_enabled 新增默认 0（不自动激活）。

- [x] **Step 4: 编译 + 测试 + Commit**

---

## Task 5: cli + server 适配

**Files:**
- Modify: `crates/cli/src/main.rs`（resolve_active_engine("asr") 无需 asr_engine 参数；select_model 适配）
- Modify: `crates/server/src/main.rs`（resolve_active_engine("asr")）

- [x] **Step 1: cli main.rs**

resolve_active_engine(&app_cfg.asr_engine) → resolve_active_engine("asr")。select_model 的引擎列表用 list_engines_from_db（已直查 DB）。

- [x] **Step 2: server main.rs**

同 cli。

- [x] **Step 3: 编译 + Commit**

---

## Task 6: 前端统一激活操作

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/AsrTab.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/LlmTab.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/OcrTab.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/Models/TranslateTab.tsx`

- [x] **Step 1: 4 Tab 激活操作统一为 switch_active_model**

AsrTab: `invoke("switch_asr_engine", {modelName})` → `invoke("switch_active_model", {domain:"asr", id})`
LlmTab: `invoke("switch_polish_llm", ...)` → `invoke("switch_active_model", {domain:"llm", id})`
OcrTab: `invoke("set_config", {key:"ocr_model"...})` → `invoke("switch_active_model", {domain:"ocr", id})`
TranslateTab: `invoke("set_config", {key:"translate_engine"...})` → `invoke("switch_active_model", {domain:"translate", id})`

- [x] **Step 2: current 判定改用后端返回的 is_enabled 字段**

- [x] **Step 3: tsc + vite build + Commit**

---

## Task 7: 文档同步 + 全量验证

- [x] architecture.md 同步（两层模型语义 + 两个核心方法 + 删 4 字段）
- [x] 全量 `cargo build --release` + 全 crate test + tsc + vite
- [x] Commit

---

## Task 8: 单测补全 + code review 反馈修复（实施后追加）

> Task 7 完成后，补充 spec §3.3/§6/§7 不变量单测（tests-after，非严格 TDD）+ code review 反馈。

**Files:**
- Modify: `crates/infra/src/db.rs`（_at 拆分 + 10+2 测试 + v37 自动迁移）
- Modify: `crates/asr-local/src/config.rs`（2 测试：云 provider + as_engine_category）
- Modify: `crates/desktop/src/runtime_config.rs`（switch_active_model 并发不变量注释）
- Modify: `crates/desktop/src/settings_commands.rs`（删 set_config 死代码分支 + 移除 engine_manager 参数）

- [x] **Step 1: 单测补全**（commit `6abbbe55`）
  - infra 10 个测试：get_active_model（4 个）+ switch_active_model（3 个）+ get_asr_model_by_spec（5 个，对 _at 拆分版）
  - asr-local 2 个测试：resolve_category 云 provider 路由 + as_engine_category 集成
- [x] **Step 2: code review Issue #1 修复**（v37 迁移自动化）
  - 原 v37 迁移仅 ALTER 加列，旧库 is_enabled=1 多行（旧「可用」语义）不清理 → 用户重新激活后破坏「每域仅 1」不变量
  - 改为自动完成 spec §10 手工 SQL：UPDATE is_available=is_enabled + UPDATE is_enabled=0 + DELETE app_config 4 字段
  - 回归测试 `migration_v36_to_v37_migrates_is_enabled_semantics_and_clears_activation`
- [x] **Step 3: code review Issue #7 修复**（id=-1 单测）
  - 新增 `switch_active_model_with_id_neg1_clears_domain`，锁定 LlmTab.tsx 的 -1 契约
- [x] **Step 4: code review Minor #2/#3/#5 修复**
  - #2 switch_active_model 加并发不变量注释（DB 是真相源，reload 读回）
  - #3 删 settings_commands set_config unreachable asr_engine 分支 + 未用 engine_manager 参数
  - #5 list_cloud_models_by_domain_at 注释修正（Task 1 后 insert_cloud_model 写 is_enabled=0）
- [x] **Step 5: Commit + 文档同步**（commit `240bee9a`）
  - spec §10 改为「v37 自动迁移」+ plan 验证结果更新（infra 126 测试）

**延后项（Minor，后续 follow-up）**：
- code review Issue #4：`set_model_enabled` 改名 `set_model_available`（需动 4 处调用点）

---

## Task 9: build_*_options current 判定改用 DB is_enabled（用户反馈 bug 修复）

> 用户反馈：aliyun:aliyun:deepseek-v4-flash 激活时，deepseek:deepseek:deepseek-v4-flash 也被标「已激活」。
> 根因：Task 3-6 改造时 build_llm_options/build_asr_options 仍接收外部 current spec 字符串按 name 匹配，
> 同 name 不同 provider 都匹配。正确做法：DB 行自带 is_enabled（每行唯一），直接用它标 current。

**Files:**
- Modify: `crates/infra/src/db.rs`（AsrEngineRow / OcrModelInfo / ModelDetailRow 补 is_enabled + provider/category）
- Modify: `crates/asr-local/src/config.rs`（EngineInfo 补全字段：id/source/secret_key/is_streaming/is_thinking/is_enabled）
- Modify: `crates/desktop/src/runtime_config.rs`（3 个 build_*_options 去 current 参数，用 DB is_enabled 标 current）
- Modify: `crates/desktop/src/settings_commands.rs`（get_config 调用点去 current 参数）

- [x] **Step 1: infra 数据结构补字段**
  - AsrEngineRow 补 id/source/secret_key/is_streaming/is_thinking/is_enabled（SQL 同步）
  - OcrModelInfo 补 is_enabled（SQL 同步）
  - ModelDetailRow 补 provider/category/is_enabled（SQL 同步）
- [x] **Step 2: asr-local EngineInfo 补全字段**
  - 原 EngineInfo 仅 5 字段（name/provider/category/description/is_local），补全到 11 字段
    （含 id/source/secret_key/is_streaming/is_thinking/is_enabled）
  - list_engines_from_db 从 AsrEngineRow 填充全字段
- [x] **Step 3: runtime_config 3 个 build_*_options 重写**
  - build_llm_options(llms)：去 current 参数，current = m.is_enabled
  - build_asr_options(engines)：去 current_effective 参数，current = e.is_enabled
    （build_asr_options 重回纯函数——不再调 list_asr_model_details，字段从 EngineInfo 取）
  - build_ocr_options(ocrs)：去 current 参数，current = m.is_enabled
  - 兜底 ASR current 判定：DB 无任何 is_enabled=1 时 fallback 视为当前（与 resolve_active_engine 对称）
- [x] **Step 4: 调用点适配**
  - list_asr_engines / list_llm_models 命令：不再传 current_raw
  - settings_commands get_config：不再传 asr_current/llm_current/ocr_current
- [x] **Step 5: 重写单测**
  - mk_engine / mk_llm helper 减少 11 字段字面量样板
  - build_llm_options_is_enabled_precise_current（同名不同 provider 回归测试，锁定 bug 修复）
  - build_asr_options_uses_is_enabled_not_name_match（同名不同 provider 回归测试）
  - build_ocr_options_uses_is_enabled_for_current
  - 删过时的 build_*_options_current_in_spec_format（3-part spec 匹配逻辑已废弃）
- [x] **Step 6: 验证 + Commit**

**验证**：infra 126 / asr-local 123 / desktop 306 / cli 4 测试全过；release build 0 error 0 warning。

---

## Self-Review 注意事项

1. **load_config 双用途处理**（Task 2 Step 5 关键）：各引擎 transcribe（whisper/paraformer 等）直接 `load_config().asr.{section}.get(name)` 取配置。RUNTIME_CONFIG 改后只缓存激活的那一个——但 transcribe 实际只被 AsrEngineManager::switch_model 调（它已缓存引擎实例，name=激活的），所以单条缓存能命中。CLI `--model xxx` 显式指定路径需保留按 name 查任意引擎的能力（新增 get_model_by_name 或 load_config 查 is_available=1 全量）。**实现时先保留 load_config 原样观察编译**。
2. **SharedRuntimeConfig 清理**（Task 3）：switch_asr_engine/switch_polish_llm 写 SharedRuntimeConfig.asr_engine/polish_llm——删字段后这些写入逻辑要清。SharedRuntimeConfig 是 `Arc<RwLock<AppConfig>>`，AppConfig 删字段后自动联动。
3. **编译顺序**：Task 1 完成后下游全报错（删了 4 字段）。**建议 Task 1-3 连续完成**（infra → asr-local → desktop 核心调用层），中间不 commit 到 main。Task 4-6 修剩余调用点。最后 Task 7 全量验证。
4. **ResolvedEngine.category 从枚举改 String**：ASR 内部大量用 EngineCategory 枚举 match（engine.rs switch_model、streaming 判定等）。ResolvedEngine.category 改 String 后这些地方要加 `as_engine_category()` 转换。保留 EngineCategory 枚举不变，只改 ResolvedEngine 存储格式。

---

## 实施记录（Task 2-7 实际偏差 + 新增决策）

> 本节为 2026-07-17 实施完成后回写，反映实际实现。

### 新增辅助函数 / 字段（spec 之外）

1. **infra ModelRow 补 language + description**（Task 2a，commit `2c651e84`）：
   load_active_engine 从 ModelRow 构造 ModelEntry 需全字段，原 ModelRow 缺 language/description。
   决策：扩展 ModelRow（非用户初始 spec，但最简——4 域统一从 ModelRow 完整构造）。

2. **ResolvedEngine 补 is_thinking 字段**（commit `bde37096`）：
   LLM domain 的 is_thinking 字段 ModelEntry 不含（仅 ModelRow 有），故提升到 ResolvedEngine
   顶层。`llm_config_ignore_mode` 用 `resolved.is_thinking` 构造 CompatibleLlmConfig。

3. **ACTIVE_ENGINES 用 LazyLock 而非 const**（commit `7b2462c3`）：
   `HashMap::new()` 非 const，`RwLock::new(HashMap::new())` 不能直接 `static`。改用
   `std::sync::LazyLock<RwLock<HashMap<...>>>`。

4. **infra 新增 get_asr_model_by_spec + asr-local 新增 resolve_engine_any**（commit `aad7bd98`）：
   CLI `--model` 多模型路径修复——load_config（仅激活）找不到非激活引擎时，engine.rs 的
   load_engine_into_cache fallback 到 resolve_engine_any（查 DB 任意可用 ASR）。
   原计划只提到「新增 get_model_by_name」，实际新增两个对称函数（category_any + engine_any）。

5. **asr-local 新增 resolve_engine_category_any**（commit `aad7bd98`）：
   CLI 多模型场景判定流式类型用，查 DB 所有可用 ASR（不限激活）。

### Task 3 偏差

1. **switch_asr_engine / switch_polish_llm 保留为 wrapper**（非删除）：
   spec §4.6 说「原命令统一为此一个」，但实际保留旧命令作 wrapper（按 name 查 DB id → 调
   switch_active_model）。原因：前端 AsrTab/LlmTab 仍可能调旧命令，wrapper 保证向后兼容。
   Task 6 前端迁移到 switch_active_model 后，旧命令仅遗留（可在后续清理）。

2. **switch_polish_llm「不选择模型」用 id=-1**：
   LLM 域允许无激活（polish_mode=Disabled 时）。空 model_name → switch_active_model("llm", -1)，
   SQL `IIF(id=-1,1,0)` 全域 is_enabled=0（取消激活）。reload_active_engine 会 warn（无激活），
   但 switch_active_model 命令仍返回 Ok。

3. **preheat_local_engine 去掉 spec 参数**：
   改为内部 resolve_active_engine("asr")。spec 参数废弃。

### Task 4 实际并入 Task 3

action_bar_commands / translation_commands 的改造与 desktop 调用层同 crate，实际在 Task 3
一起完成。Task 4 仅剩 model_commands 的 DownloadableModel.is_available 字段分离（commit `99e4484f`）。

### Task 6 偏差

1. **TranslateTab cloud 模型仍用 engineName 比对 current**：
   list_translate_cloud_models 命令返回的 TranslateCloudModel 无 is_enabled 字段（非 LlmOption
   结构）。cloud 模型的 current 判定仍用 translate_status 返回的 engineName 比对 model_name。
   本地 translate 模型已改用 is_enabled（DB 行带此字段）。

### 验证结果（Task 7）

- cargo build --release（server + cli + asr-local + desktop embedded,cloud）0 error 0 warning
- infra 126 测试（+12 含 v37 迁移回归）/ desktop 307 测试全过
- asr-local 123 单测全过；5 个 real_model 测试失败（需用户手工迁移 DB，spec §10）
- tsc --noEmit 0 error；vite build 成功

### DB 迁移（v37 自动迁移，code review Issue #1 修复）

**最终实现**：v36→v37 迁移在 `init_schema` 中自动完成（对齐原计划的手工 SQL），用户无需手工干预。
原计划「用户手工迁移」改为自动——code review Issue #1 指出若用户仅升级 app 不执行手工 SQL，
旧库多 is_enabled=1 行（旧「可用」语义）会破坏「每域仅 1 个激活」不变量。

迁移块在 `crates/infra/src/db.rs` v37 段：补 is_available 列 + `UPDATE is_available=is_enabled`
+ `UPDATE is_enabled=0` + 删 app_config 4 个废弃字段。

回归测试 `migration_v36_to_v37_migrates_is_enabled_semantics_and_clears_activation`（infra db tests）
+ `switch_active_model_with_id_neg1_clears_domain`（id=-1 LLM「不选择模型」路径）。

迁移后用户在设置页重新激活所需模型（switch_active_model）。
