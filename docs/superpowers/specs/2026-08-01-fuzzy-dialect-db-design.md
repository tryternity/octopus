# 方言模糊规则 DB 化 + 配置页拆双 Tab

> **日期**：2026-08-01
> **状态**：🔜 待实现
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

seed 6 条：fei/hui + yun/yong（syllable）；n/l + f/h + r/l（initial）；hu/wu（special_hu）。

**match_type 语义**：
- `syllable`：拼音 `== from_py` → 替换成 `to_py`（整音节精确）
- `initial`：拼音 `starts_with(from_py)` → 首字母替换成 `to_py`
- `special_hu`：单字 hu→wu + huX→wX（非单字符，硬编码逻辑）

执行顺序：基础规则（始终开）→ syllable 组 → initial 组 → special_hu。一个字只归一组（flag 互斥）。

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

## 4. 不在范围
- 服务推送更新（后续）
- 用户自定义规则（后续）
- 基础规则（平翘舌+前后鼻音）DB 化（始终开无开关）
