# 实施计划：方言模糊规则 DB 化 + 配置页拆双 Tab

> **对应 spec**：[2026-08-01-fuzzy-dialect-db-design.md](../specs/2026-08-01-fuzzy-dialect-db-design.md)
> **分支**：`bugfix/pr-0801`
> **状态**：✅ 已完成（3 个 commit）

## 任务分解 + 实施记录

### Task 1：后端 DB 化 + schema v55→v56 ✅

**commit**：`920ccb90 refactor(hotword): 方言规则 DB 化（后端）+ schema v55→v56`

文件：
- `crates/infra/src/db.sql` — 新增 `fuzzy_dialect_rules` 表 CREATE + seed 7 条（全新库；si/ci 后续追加）
- `crates/infra/src/db/mod.rs` — `CURRENT_SCHEMA_VERSION` v55→v56；`init_schema` 改 **while 循环迁移链**（`while cur < CURRENT { match cur {...} }`），新增 `55 =>` 分支建表 + seed + 旧 `app_config.fuzzy_dialect` 字符串开关迁移到表 `enabled`
- `crates/infra/src/db/hotword.rs` — 新增 `FuzzyDialectRule` struct + `FUZZY_RULE_COLS` + `list_fuzzy_dialect_rules` / `list_enabled_fuzzy_dialect_rules` / `set_fuzzy_dialect_rule_enabled`
- `crates/asr-local/src/text/hotword.rs` — 废弃 `FuzzyRules` struct + const 表 + `parse_dialect`；新增 `FUZZY_RULES_CACHE: OnceLock<RwLock<Vec<FuzzyDialectRule>>>` + `set_fuzzy_rules_cache` + `normalize_with_rules(py, rules)`（按 match_type 分组：基础→syllable→initial→special_hu）；`normalize_fuzzy_pinyin` 改读 cache
- `crates/asr-local/src/text/corrector.rs` — `reload_fuzzy_dialect` 改无参（内部 `list_enabled_fuzzy_dialect_rules` 读 DB → `set_fuzzy_rules_cache`），重建索引
- `crates/infra/src/config.rs` — 删 `fuzzy_dialect` 字段校验
- `crates/desktop/src/commands/hotword_commands.rs` — 新增 `list_fuzzy_dialect_rules` / `set_fuzzy_dialect_rule` Tauri 命令（写后调 `reload_fuzzy_dialect`）
- `crates/desktop/src/commands/settings_commands.rs` — 删 `fuzzy_dialect` 字符串校验
- `crates/desktop/src/core/invoke_handler.rs` — 注册 2 个新命令
- `crates/desktop/src/core/setup.rs` — 启动 setup 先 `reload_fuzzy_dialect`（设 cache）再 `reload_hotwords`（顺序不能反）

**match_type 三组语义落地**：`syllable`（`py == from_py` 整音节精确）、`initial`（`py.starts_with(from_py)` 声母前缀）、`special_hu`（hu→wu + huX→wX 硬编码）。rules 须按 `(match_type, sort_order)` 排序，syllable 在 initial 前避免 fei 被 f/h 抢。

**验证**：`cargo build -p octopus-infra -p octopus-asr-local -p octopus-desktop` → 0 error；corrector 测试 `normalize_with_rules` 方言组改用 `rule()` 辅助注入 DB struct。

### Task 2：前端 HotwordPanel UnderlineTabs 拆双 Tab ✅

**commit**：`a32eea54 refactor(hotword): 前端 UnderlineTabs 拆分——纠错设置 / 词典维护`

文件：
- `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx` — 引入 `UnderlineTabs`，内部拆两个子 Tab：
  - **TAB 1「纠错设置」**：`asr_correct` 总开关 + 方言规则 toggles（`list_fuzzy_dialect_rules` 读 DB + `set_fuzzy_dialect_rule` 乐观更新写 DB，失败回滚）
  - **TAB 2「词典维护」**：原词典列表 + 词卡 + 浮窗（逻辑不变，仅搬到 TAB 2 容器）
- 废弃 `DIALECT_KEYS` 常量 + dialect prop
- `crates/desktop/frontend/src/locales/zh-CN.yaml` — 新增 `tabCorrect: 纠错设置` / `tabManage: 词典维护`

### Task 3：方言规则 UI 按 matchType 分两组 ✅

**commit**：`4675dbdc fix(ui): 方言规则 UI 分两组——声母模糊 / 整音节模糊`

文件：
- `crates/desktop/frontend/src/pages/Settings/HotwordPanel.tsx` — TAB 1 方言规则按 `matchType` 分两个视觉组展示：**声母模糊**（initial：n/l、f/h、r/l + special_hu：hu/wu）+ **整音节模糊**（syllable：fei/hui、yun/yong）

## Spec Coverage

| spec section | 对应 Task | 状态 |
|---|---|---|
| §2 DB 表 fuzzy_dialect_rules | Task 1 | ✅ |
| §3 后端代码改造 | Task 1 | ✅ |
| §3 前端 HotwordPanel 双 Tab | Task 2 | ✅ |
| §3 迁移 v55→v56 | Task 1 | ✅ |
| §4.1 init_schema while 循环 | Task 1（额外重构） | ✅ |
| §4.5 前端分两组展示 | Task 3 | ✅ |

## 验证记录

- 后端编译：`cargo build` 相关 crate 0 error 0 warning
- 真实 DB 迁移：v55（`fuzzy_dialect="f/h,r/l"`）→ 启动自动迁移 v56，旧开关状态保留到 `fuzzy_dialect_rules.enabled`
- 分支已与 main 同步（`845fdf5d`）
