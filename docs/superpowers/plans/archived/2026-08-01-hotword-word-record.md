# 实施计划：热词存储拆分单记录 + sync 重构

> **对应 spec**：[2026-08-01-hotword-word-record-design.md](../specs/2026-08-01-hotword-word-record-design.md)
> **分支**：`bugfix/pr-0801`（存储拆分 + set 级 sync）+ `feature/hotword-word-merge`（word 级 merge）
> **状态**：✅ 存储拆分 + set 级 sync + word 级 3-way merge 全部完成（word 级 merge 详见 [2026-08-01-hotword-word-level-merge.md](2026-08-01-hotword-word-level-merge.md)）

## 任务分解 + 实施记录

### Task 1：infra 加 uuid v5 + 拼音工具 + 新表 ✅

**commit**：`40ac795f`

- `crates/infra/Cargo.toml`：加 `uuid = { version = "1", features = ["v5"] }`
- `crates/infra/src/hotword_text.rs`：`HOTWORD_NAMESPACE` + `hotword_word_uuid(set_id, word)`（v5 确定性）+ `word_plain_pinyins(word)`（原始拼音）
- `crates/infra/src/db.sql`：`hotword_words` 表（id/set_id/word/pinyin/is_deleted/timestamps/sync_md5 + UNIQUE(set_id,word) + idx）+ `hotword_sets` 去 words_text
- `crates/infra/src/db/mod.rs`：`CURRENT_SCHEMA_VERSION` 56→57

### Task 2：HotwordWord CRUD + 迁移 v56→v57 ✅

**commit**：`40ac795f`

- `HotwordWord` struct + CRUD：list_words_in_set / list_active_words(带拼音) / add_word_to_set(幂等+恢复软删) / add_words_to_set / remove_word_from_set(软删) / set_words_in_set(覆盖diff) / upsert_hotword_word / list_all_hotword_words / get_hotword_word / update_hotword_word_sync_md5
- `HotwordSet` 去 words_text 列
- 迁移 v56→v57：建表 + words_text split 成词记录 + DROP 列（幂等：列不存在时跳过）
- 容量校验改按 hotword_words 行数
- 测试全过 168/168

### Task 3：sync 重构——set 级元数据同步 ✅（word 级 merge 已由后续 plan 完成）

**commit**：`ab492146`

- `HotwordSetFile` 去 words_text（set 文件只存元数据）
- `hotword_set_md5` / `hotword_set_md5_from_fields` 改 name|enabled（2 参）
- 测试改写：sample_set 2 参 + add_words/words_text_of 辅助 + set 级断言改 name
- 原 word 级断言（words_text 跨 sync 传播）标 TODO，word 级 merge 后续补
- 测试全过 108/108

**后续已完成**：word 级 3-way merge 由 [2026-08-01-hotword-word-level-merge.md](2026-08-01-hotword-word-level-merge.md) 实现（commit `96560238`）——两级 outline 层级 + `HotwordWordFile` + `hotword_word_md5` + `merge_hotword_words` + word sync_md5 DB 层填充。`HotwordSetFile` 进一步演化为 `HotwordSetMeta`（两级结构 `<set-id>/meta.json`）。

### Task 4：asr-local 适配（临时方案，已被后续 commit 升级）✅

**commit**：`ab492146`

- `reload_hotwords` 调 `list_active_words()`（带 pinyin），但暂时只取词文本（`.iter().map(|(w, _)| w.clone())`）
- corrector 逻辑不变（排序留后续）
- **后续已升级**：commit `af517dcc`（[pinyin-and-ranking spec](../specs/2026-08-01-hotword-pinyin-and-ranking-design.md)）——`from_words` 改接 `(word, pinyin, hit_count)` 三元组，跳过 `to_pinyin()` 现算；`reload_hotwords` 直接传三元组（不再丢弃拼音）；多命中按 hit_count 排序。

### Task 5：desktop 命令 + 前端适配 ✅

**commit**：`ab492146`

- `list_words_in_set` / `list_word_counts` 新命令
- import_hotwords: set_hotword_set_words → set_words_in_set
- export_hotwords: set.words_text → list_words_in_set join
- 前端 HotwordPanel: HotwordSet 接口去 wordsText + selectedWords/wordCounts state

### Task 6：文档 ✅

本 plan + spec + architecture.md。

## Spec Coverage

| spec section | 对应 Task | 状态 |
|---|---|---|
| §2 数据模型 hotword_words 表 | Task 1-2 | ✅ |
| §2 UUID v5 确定性 | Task 1 | ✅ |
| §2 拼音存原始 | Task 1-2 | ✅ |
| §3 sync word 级 3-way merge | Task 3 + [word-level-merge plan](2026-08-01-hotword-word-level-merge.md) | ✅（两级 outline 层级，2026-08-02 完成） |
| §4 HotwordIndex 适配 | Task 4 + [pinyin-and-ranking spec](../specs/2026-08-01-hotword-pinyin-and-ranking-design.md) | ✅（commit `af517dcc`，from_words 跳过 to_pinyin 现算） |
| §5 迁移 v56→v57 | Task 2 | ✅ |

## 不在范围（留后续）

- ~~**HotwordIndex 拼音优化**~~：**已解决**（commit `af517dcc`，[pinyin-and-ranking spec](../specs/2026-08-01-hotword-pinyin-and-ranking-design.md)）——`from_words` 接收 `(word, pinyin, hit_count)` 三元组，跳过 `to_pinyin()` 现算，只做必要的 `normalize_fuzzy_pinyin`（方言规则运行时生效，不能预存归一化 key）。
- ~~**correct 多命中排序**~~：**已解决**（同 commit `af517dcc`）——hit_count JOIN 进 list_active_words，多命中按 hit_count 降序排序。
- ~~**set 级删除复活问题**~~：**已解决**（2026-08-02，[set 软删 spec](../specs/archived/2026-08-02-hotword-set-soft-delete.md)）——set 级 `is_deleted` 存时间戳 + `UNIQUE(name,is_deleted)` + tombstone 经 merge 传播。词级软删（`hotword_words.is_deleted`）此前已解决（本次 word 级 merge）。
