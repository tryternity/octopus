# 热词 tombstone GC——10 天后硬删 + 手动清空

> **日期**：2026-08-02
> **状态**：✅ 已实现
> **背景**：set 级软删（v58）+ word 级软删（v57）后，tombstone（is_deleted>0）长期堆积在 DB + .sync。本 spec 加 GC：超期 tombstone 自动硬删 + 手动清空回收站。
> **关联**：`2026-08-02-hotword-set-soft-delete.md` §6 留后续项「tombstone GC」。

## 1. 动机

软删后行不删（tombstone 经 sync 传播删除意图），但长期堆积：
- DB：`hotword_sets` is_deleted>0 + `hotword_words` is_deleted>0 的行永远在
- .sync：tombstone 词典目录 `<set-id>/meta.json`（is_deleted>0）+ 词文件 + outline entry 永远在
- git 仓库膨胀 + sync diff 噪音

vault 有 `empty_trash`（手动清空回收站）但**无自动 GC**。热词补齐两层：自动 10 天 GC + 手动清空。

## 2. 前置：统一 is_deleted 语义（word bool→i64 秒）

**现状**：set 级 is_deleted 是 i64 epoch 秒（0=活跃，>0=删除时刻）；word 级 is_deleted 是 bool（0/1）。不对称——word 无删除时刻，无法按年龄 GC。

**统一**：word 级 is_deleted 也改 i64 epoch 秒（0=活跃，>0=删除时刻）。判断逻辑删除统一为 `is_deleted > 0`。

- **无 schema 迁移**：DB 列已是 `INTEGER NOT NULL DEFAULT 0`，存 0/1 还是秒都是整数
- set 现状秒不变；word 从 0/1 升级为秒（0=活跃语义不变）
- `HotwordWord.is_deleted: bool` → `i64`；`HotwordWordFile.is_deleted: bool` → `i64`（version 1→2）
- `hotword_word_md5_from_fields` 第 4 参 `bool` → `i64`
- `remove_word_from_set_at`：`SET is_deleted=1` → `SET is_deleted=<now_secs>`
- 所有 `WHERE is_deleted=0`（过滤活跃）不变

### 旧文件兼容

version 1 word 文件（is_deleted=0/1）反序列化成 i64：is_deleted=1 → 值 1（epoch 1 秒 = 1970 年，超期，GC 清掉——合理）；is_deleted=0 → 0（活跃，不变）。

## 3. GC 方向：merge 按年龄过滤（跨设备自洽，无锁）

### 为什么不用 vault 式持锁 purge

vault `empty_trash` 持 sync lock purge DB + 清 .sync。但跨设备：A purge 后 .sync 无 tombstone → B 机 DB 还有（没到 GC 阈值）→ B export 又写回 → A pull → **复活**。单设备自洽，跨设备不自洽。

### merge 按年龄过滤（本方案）

- **purge**：本地 DB 硬删 is_deleted>0 且 `now - is_deleted > RETENTION` 的 tombstone
- **export**：超期 tombstone 不写文件 + outline 不含 + 删已存在的 .sync 目录
- **merge pull**：读 meta.json/word file 拿 is_deleted → 超期 skip（不 upsert，不复活）

**跨设备自洽证明**：A 机 GC 后 export 不含超期 tombstone；B 机 pull 时即使旧 outline 有该 tombstone，merge 读 meta.json 检查年龄超期也 skip；B 机下次 GC 也清掉 → 收敛。各机 GC 时机不同也自洽（都按 10 天阈值）。

## 4. retention + 常量

- `HOTWORD_TOMBSTONE_RETENTION_SECS = 10 * 86400`（硬编码 10 天）
- 放 infra（db + sync 都引用）

## 5. GC 范围

统一处理 set + word 两层 tombstone（is_deleted 统一后，两层都按年龄 GC）：
- set tombstone（is_deleted>0）：超期硬删 set + 其词记录 + 其 hotword_hits（孤儿）
- word tombstone（活跃词典里 is_deleted>0 的词）：超期硬删词记录

## 6. 触发

- **自动**：scheduler 每日（interval 86400）调 `purge_expired_hotword_tombstones` + export 重建
- **手动**：前端「清空回收站」按钮调 `purge_all_hotword_tombstones`（不限年龄）

## 7. 不在范围（留后续）

- **retention 可配**：当前硬编码 10 天。未来加 config（对齐 clipboard_max_age_days）。
- **回收站 UI（查看/恢复/单条删）**：当前只「清空全部」。未来做回收站面板展示 + 恢复 + 单条永久删。

## 8. 风险

1. **merge 年龄过滤的性能**：每个 tombstone pull 需读 meta.json/file 拿 is_deleted。但 tombstone 数量少（用户删的词典数 × 10 天积累），开销可忽略。
2. **word 旧文件 is_deleted=1 被 GC**：version 1 文件 is_deleted=1 反序列化为 epoch 1 秒（1970），超期被 GC。语义正确（它确实是已删除的），但用户若手动恢复过旧文件可能意外。实际无此场景（恢复走 DB add，不走文件）。
