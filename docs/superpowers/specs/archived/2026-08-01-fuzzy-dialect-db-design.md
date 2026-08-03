# 方言模糊规则 DB 化 + 配置页拆双 Tab

> **日期**：2026-08-01
> **状态**：✅ 已实现（commit `920ccb90` 后端 / `a32eea54` 前端双 Tab / `4675dbdc` UI 分两组）
> **背景**：方言规则从代码硬编码（FuzzyRules struct + const 表 + app_config.fuzzy_dialect 字符串）迁移到 DB 表，便于后续服务推送更新；配置页拆两个子 Tab（纠错设置 / 词典维护）。

## 1. 动机

当前痛点：规则数据散落（struct + const 表 + 硬编码 hu/wu + parse_dialect + settings 白名单，新增规则改 4 处）；配置是字符串（fuzzy_dialect 逗号 token）；页面职责混杂（全局配置 + 词典管理挤一个 Panel）。

## 2. DB 表：`fuzzy_dialect_rules`

```sql
CREATE TABLE IF NOT EXISTS fuzzy_dialect_rules (
    token TEXT PRIMARY KEY,          -- 'f/h'、'yun/yong'
    label TEXT NOT NULL,             -- 'f/h（浮 / 护）'
    from_py TEXT NOT NULL,           -- 'f'、'yun'、'hu'
    to_py TEXT NOT NULL,             -- 'h'、'yong'、'w'
    match_type TEXT NOT NULL,        -- 'syllable' | 'initial' | 'special_hu'
    enabled INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL DEFAULT 0
);
```

seed 7 条：fei/hui + yun/yong + **si/ci**（syllable）；n/l + f/h + r/l（initial）；hu/wu（special_hu）。

**si/ci 规则的「4 归 1」机制**（2026-08-01 新增）：基础规则（始终开）已把 `shi→si`、`chi→ci`（平翘舌归一），故 syllable 规则 `si→ci` 一条即可让 si/shi/chi/ci 四者全部收口到 ci——`shi`（时）经基础变 `si` 再经规则变 `ci`，`chi`（吃）经基础已变 `ci`。label 用 `si/ci（四 / 词）` 展示一对高频易混字，sort_order=3（syllable 组内 fei→yun→si）。**注意**：这是整音节精确匹配（`==`），不影响 se/ce、san/can 等其他 si/ci 开头的音节。

**match_type 语义**：
- `syllable`：拼音 `== from_py` → 替换成 `to_py`（整音节精确）
- `initial`：拼音 `starts_with(from_py)` → 首字母替换成 `to_py`
- `special_hu`：单字 hu→wu + huX→wX（非单字符，硬编码逻辑）

执行顺序：基础规则（始终开）→ syllable 组 → initial 组 → special_hu。一个字只归一组（flag 互斥）。

> **⚠️ 存量 DB 注意**：si/ci 是在 schema v56 已发布后追加的 seed（仅改 db.sql + mod.rs v55→v56 分支的 seed，**不升 schema 版本**）。故**已迁移到 v56 的存量 DB 不会自动获得第 7 条**（v55→v56 分支已执行过，`INSERT OR IGNORE` 不会重跑）。全新库从 db.sql 建出则有全部 7 条。存量 DB 需手动 `INSERT INTO fuzzy_dialect_rules VALUES ('si/ci','si/ci（四 / 词）','si','ci','syllable',0,3)` 或清库重建。

废弃 `app_config.fuzzy_dialect` 字符串——开关状态在 `fuzzy_dialect_rules.enabled`。

## 3. 代码改造

### 后端
- DB CRUD：`list_fuzzy_dialect_rules()` / `list_enabled_fuzzy_dialect_rules()` / `set_fuzzy_dialect_rule_enabled(token, enabled)` + `FuzzyDialectRule` struct
- `normalize_with_rules(py, rules: &[FuzzyDialectRule])`：废弃 FuzzyRules struct + const 表 + parse_dialect。全局缓存 `FUZZY_RULES_CACHE`
- `reload_fuzzy_dialect()`：无参，内部从 DB 读
- 新增 `list_fuzzy_dialect_rules` / `set_fuzzy_dialect_rule` Tauri 命令；删 fuzzy_dialect 字符串校验

### 前端
- HotwordPanel 内部 UnderlineTabs：「纠错设置」/「词典维护」
- TAB 1：asr_correct 总开关 + 方言规则列表（DB 读 + toggle 写 DB）
- TAB 2：原词典列表 + 词卡 + 浮窗（不变）
- 废弃 DIALECT_KEYS 常量 + dialect prop

### 迁移（schema v55→v56）
- 建 fuzzy_dialect_rules 表 + seed
- 读旧 app_config.fuzzy_dialect 字符串 → 解析 token → UPDATE enabled=1（保留开关）
- 删 app_config.fuzzy_dialect 行

## 4. 实现注记（实际落地与设计差异 / 补充）

### 4.1 init_schema 改 while 循环迁移链（额外重构）
迁移 v56 时把 `init_schema` 从「单分支 `if v == 54`」重构为 **while 循环迁移链**：`let mut cur = v; while cur < CURRENT_SCHEMA_VERSION { match cur { 54 => ..., 55 => ..., _ => bail } cur += 1; PRAGMA user_version = cur }`。每个分支升 1 版本，v54→v55→v56... 串行，未来加 v57 只需加一个 `match` 分支，无需再改外层结构。`_ =>` 兜底 bail 54 以下旧库（不支持表结构自动迁移）。

### 4.2 seed 在两处（db.sql + mod.rs 迁移分支）
`fuzzy_dialect_rules` 表的 CREATE + seed 7 条**同时存在于** ① `crates/infra/src/db.sql`（全新库 `v==0` 由 `execute_batch` 一次性建出）；② `crates/infra/src/db/mod.rs` 的 `55 =>` 迁移分支（存量库 v55→v56 升级用）。两处内容必须一致（token / label / from_py / to_py / match_type / enabled=0 / sort_order）。改规则时两处都要改。

### 4.3 旧 fuzzy_dialect 字符串迁移（保留状态）
v55→v56 迁移分支读旧 `app_config.fuzzy_dialect`（逗号分隔 token，如 `"f/h,r/l"`）→ `split(',')` → `UPDATE fuzzy_dialect_rules SET enabled=1 WHERE token=?`（逐个 token）。**不删 app_config.fuzzy_dialect 行**——留作历史，不再读。日志记录迁移的旧值或「无旧配置」。

### 4.4 normalize_with_rules 分组遍历 + matched flag 互斥（与数组顺序无关）
`normalize_with_rules(py, rules)` 按 match_type **分三个独立 for 循环**遍历：syllable 组 → initial 组 → special_hu 组，每组的循环间用 `matched` flag 互斥（syllable 命中则跳过 initial/special_hu）。**正确性与 rules 数组的传入顺序无关**——分组遍历保证 syllable 总在 initial 前评估，即使调用方把 initial 规则排在数组前面，`fei` 仍走 fei/hui（syllable）而非被 f/h（initial）抢成 hei。

DB 层 `list_fuzzy_dialect_rules` / `list_enabled_fuzzy_dialect_rules` 的 SQL `ORDER BY match_type, sort_order` **仅为输出确定性**（UI 展示顺序稳定、log 可复现），不参与归一化正确性。回归测试 `syllable_beats_initial_regardless_of_array_order` 锁定此行为（sorted + shuffled 两组 rules 断言结果一致）。

### 4.5 前端规则按 match_type 分两组展示
TAB 1「纠错设置」方言规则 toggle 按 `matchType` 分两个视觉组：**声母模糊**（initial：n/l、f/h、r/l）+ **整音节模糊**（syllable：fei/hui、yun/yong）。special_hu（hu/wu）归到声母模糊组（视觉上与声母类规则同类）。乐观更新：toggle 即调 `set_fuzzy_dialect_rule(token, enabled)` 写 DB，失败回滚 UI 状态。

### 4.6 FUZZY_RULES_CACHE 全局缓存
`crates/asr-local/src/text/hotword.rs` 的 `FUZZY_RULES_CACHE: OnceLock<RwLock<Vec<FuzzyDialectRule>>>` 持 enabled 规则缓存，`reload_fuzzy_dialect()`（无参，内部 `list_enabled_fuzzy_dialect_rules` 读 DB）调 `set_fuzzy_rules_cache` 更新。`normalize_fuzzy_pinyin(py)` 读 cache 调 `normalize_with_rules`。启动 setup 先 `reload_fuzzy_dialect`（设 cache）再 `reload_hotwords`（建索引）——顺序不能反（索引 key 由 normalize 生成，规则变 key 必变）。

## 5. 不在范围
- 服务推送更新（后续）——本次 DB 化就是为此铺路
- 用户自定义规则（后续，UI 需新增编辑入口）
- 基础规则（平翘舌 + 前后鼻音）DB 化（始终开无开关，代码内联）
