# 实施计划：热词存储拆分单记录 + sync 重构

> **对应 spec**：[2026-08-01-hotword-word-record-design.md](../specs/2026-08-01-hotword-word-record-design.md)
> **分支**：`bugfix/pr-0801`
> **状态**：✅ 存储拆分 + set 级 sync 完成（word 级 merge 待后续）

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

### Task 3：sync 重构——set 级元数据同步 ✅（word 级 merge 待后续）

**commit**：`ab492146`

- `HotwordSetFile` 去 words_text（set 文件只存元数据）
- `hotword_set_md5` / `hotword_set_md5_from_fields` 改 name|enabled（2 参）
- 测试改写：sample_set 2 参 + add_words/words_text_of 辅助 + set 级断言改 name
- 原 word 级断言（words_text 跨 sync 传播）标 TODO，word 级 merge 后续补
- 测试全过 108/108

**待做（后续）**：`HotwordWordFile` + `hotword_word_md5` + `merge_hotword_words`（word 级 3-way merge）+ outline 加 words 字段

### Task 4：asr-local 适配（临时方案）✅

**commit**：`ab492146`

- `reload_hotwords` 调 `list_active_words()`（带 pinyin），但暂时只取词文本（`.iter().map(|(w, _)| w.clone())`）
- corrector 逻辑不变（排序留后续）
- **待做（后续）**：HotwordIndex::from_words 接收带拼音结构，跳过 to_pinyin 现算

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
| §3 sync word 级 3-way merge | Task 3 | ⏳ set 级✅ / word 级待后续 |
| §4 HotwordIndex 适配 | Task 4 | ⏳ 临时方案✅ / 拼音优化待后续 |
| §5 迁移 v56→v57 | Task 2 | ✅ |

## 不在范围（留后续）

- **sync word 级 merge**：HotwordWordFile + hotword_word_md5 + merge_hotword_words + outline words 字段。当前 set 级元数据同步正常，但词数据不跨设备同步（两设备各自加词不合并）。
- **HotwordIndex 拼音优化**：from_words 接收带拼音结构，跳过 to_pinyin 现算（当前临时方案仍现算）。
- **correct 多命中排序**：hotword_words 已带元数据，后续 spec 做。
