# 热词 set 级软删 + tombstone sync

> **日期**：2026-08-02
> **状态**：✅ 已实现
> **背景**：word 级软删已解决（commit `96560238`，词级 `is_deleted` + word merge tombstone），但 set 级仍是硬删——`delete_hotword_set` 直接 `DELETE FROM`，跨设备删除会复活（A 删集 → push 删文件 → B merge 不删 DB → B push 又写回 → A merge 复活）。本 spec 给 set 级加软删 + tombstone 传播，与 word 级对称。
> **关联**：取代 `2026-08-01-hotword-sync-merge-model.md` §5 的 `[ ] 热词 set 级删除复活` task。

## 1. 动机

当前 set 级删除复活问题（spec `2026-08-01-hotword-sync-merge-model` §5 已记录）：

```
设备 A：delete_hotword_set(id) → 硬删 DB 行 + 词记录
设备 A：sync → export_all_hotwords 重建（词典目录消失）→ push
设备 B：sync → merge_hotwords
  - 阶段 1：B 的 DB 有该集 → 但 A 的 outline 无 → 「DB 有 + outline 无 → push」B 又写回文件
  - 或：B 的 DB 无该集 → A 的 outline 无 → 都没有，不传播删除
设备 B：sync → push → A pull → A 的词典复活（从 B 的文件）
```

vault 用 `is_deleted` 软删（cipher + folder 都有），tombstone 文件在 `.sync/` 里传播删除意图，merge 据此软删对端。热词 word 级已对齐，set 级需补。

## 2. 数据模型：is_deleted 存时间戳（非 bool）

### 为什么不用 bool（0/1）

`hotword_sets.name` 有 `UNIQUE` 约束（用户可读明文名，防重名）。软删后若 name 不变，行还在占着 name → 用户重建同名词典被 UNIQUE 拒绝。

### 方案：is_deleted 存删除时刻的 epoch 秒

```sql
-- name + is_deleted 复合唯一约束
UNIQUE(name, is_deleted)
```

- **未删的（is_deleted=0）**：name + 0 唯一 → 同名只能有一个活跃词典（用户预期不变）
- **删了的（is_deleted=时间戳秒）**：name + 不同时间戳 → 可有多个同名 tombstone（各自删除时刻不同，天然不冲突）
- **重建同名**：新行 is_deleted=0，与旧 tombstone（is_deleted=时间戳）的 (name, is_deleted) 组合不同 → 不冲突

**优势 vs 其他方案**：
- 比 `name 加后缀`（`__deleted__:name`）干净——name 字段不被污染，tombstone 文件 name 保持原值
- 比 `去 UNIQUE`（对齐 vault folder）保持用户预期——同名活跃词典仍被拒
- 比 `ON CONFLICT(name) 复活`语义正确——重建是新建空词典，不是复活旧数据
- 时间戳额外提供删除时刻元数据（未来回收站 UI 可展示「X 天前删除」）

### `hotword_sets` schema 变更（v57→v58）

```sql
CREATE TABLE IF NOT EXISTS hotword_sets (
    id          TEXT    PRIMARY KEY,
    name        TEXT    NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    sync_md5    TEXT,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    is_deleted  INTEGER NOT NULL DEFAULT 0,      -- 0=活跃，>0=删除时刻 epoch 秒
    UNIQUE(name, is_deleted)                      -- 活跃同名唯一；tombstone 各自带时间戳不冲突
);
```

注意：SQLite 不支持 ALTER TABLE 改约束，迁移用「建新表 + 复制 + DROP + RENAME」（详见 plan）。

### 与 vault `is_deleted`（bool 0/1）的差异

vault cipher/folder 的 is_deleted 是 bool（0/1），靠 name 不 UNIQUE 回避冲突。热词 set 的 is_deleted 是时间戳（>0），靠 `UNIQUE(name, is_deleted)` 让 tombstone 不冲突。两层语义统一（>0 表删除），存储类型不同（vault bool / 热词时间戳）——各自场景的最优解。

## 3. 软删语义

### `delete_hotword_set` 改为软删

软删：`UPDATE hotword_sets SET is_deleted=now_secs, updated_at=now WHERE id=? AND is_deleted=0`。
**词级级联软删**：`UPDATE hotword_words SET is_deleted=1 WHERE set_id=? AND is_deleted=0`（保持与原硬删行为一致——删词典=清空其词）。即使不级联，`list_active_words` JOIN 加 `AND s.is_deleted=0` 也能屏蔽——但级联让语义干净。

### 读取过滤

| 函数 | 变更 |
|---|---|
| `list_hotword_sets_at` | 加 `WHERE is_deleted=0`（前端列表只看活跃） |
| `list_active_words_at` | JOIN 加 `AND s.is_deleted=0`（corrector 并集屏蔽软删词典的词） |
| `get_hotword_set_at` | **不加过滤**——sync merge 需读 tombstone 行（按 id 查，含软删）；命令层按需判断 |
| 新增 `list_all_hotword_sets()` | 不过滤 is_deleted——sync export 用（mirror `list_all_hotword_words`） |

### name UNIQUE 重建场景

用户删「项目A」→ 重建「项目A」：
- 旧行：`(id_A, name='项目A', is_deleted=1800000000)`
- 新行：`(id_B, name='项目A', is_deleted=0)`
- `UNIQUE(name, is_deleted)`：('项目A', 1800000000) ≠ ('项目A', 0) → 不冲突 ✓
- `list_hotword_sets`（WHERE is_deleted=0）只返回新行 ✓

## 4. tombstone sync 传播

### set md5 含 is_deleted

`hotword_set_md5_from_fields` 输入从 `(name, enabled)` 改为 `(name, enabled, is_deleted)`。软删后 is_deleted 变化 → md5 变化 → outline diff 检测到 → merge 传播。

### `HotwordSetMeta` 加 is_deleted 字段（version 1→2）

```rust
pub struct HotwordSetMeta {
    pub version: u32,        // 2（加 is_deleted）
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub is_deleted: i64,     // 新增：0=活跃，>0=删除时刻 epoch 秒
    pub created_at: String,
    pub updated_at: String,
}
```

`version: 2` + 旧文件（version 1，无 is_deleted）反序列化时 `#[serde(default)]` is_deleted=0（兼容降级）。`HotwordSet` struct（infra）也加 is_deleted 字段。

### merge 传播路径

`merge_hotwords` 阶段 1 set 层 merge 已有的 3-way 逻辑天然支持 tombstone：

- **A 软删集 + push**：A 的 outline 里该集 md5 变（含 is_deleted=时间戳）+ 词典目录 meta.json 写 is_deleted=时间戳
- **B pull**：B 读到 md5 变化 → pull_set 读 meta.json（is_deleted=时间戳）→ `upsert_hotword_set` 写入 is_deleted → B 的 DB 该集变软删
- **B 的 corrector**：`list_active_words` JOIN `s.is_deleted=0` 屏蔽 → 词不再纳入

关键：`upsert_hotword_set_at` 的 ON CONFLICT(id) 要 SET is_deleted=excluded.is_deleted（当前不 set，需加）。

### 不复活保证

「DB 有 + outline 无 → push」分支（merge 阶段 1 末尾）不再发生——因为软删集**仍在 DB**（is_deleted>0），`list_all_hotword_sets` 仍返回它，export 仍写它的 tombstone meta.json，outline 仍有 entry。删除意图经 md5 + is_deleted 字段传播，不靠「文件消失」。

## 5. 迁移 v57→v58

SQLite 不支持 ALTER TABLE 改 UNIQUE 约束。迁移用建表复制法：

```sql
-- 1. 建新表（含 is_deleted + UNIQUE(name,is_deleted)）
CREATE TABLE hotword_sets_new (...含 is_deleted + UNIQUE(name,is_deleted));
-- 2. 复制数据（现有行 is_deleted=0）
INSERT INTO hotword_sets_new SELECT id, name, enabled, sync_md5, created_at, updated_at, 0 FROM hotword_sets;
-- 3. 换表
DROP TABLE hotword_sets;
ALTER TABLE hotword_sets_new RENAME TO hotword_sets;
```

**不可逆**：DROP TABLE 旧表（但复制了数据，无数据丢失）。迁移前建议备份 `~/.octopus/octopus.db`。

## 6. 不在范围（留后续）

- **回收站 UI**：is_deleted>0 的词典当前只是 tombstone（不显示），未来可做回收站面板展示 + 恢复 + 永久删。
- **tombstone GC**：软删词典长期堆积。未来加「30 天后硬删」或「用户手动清空回收站」。
- **correct 多命中排序**：与 set 软删无关，单独后续。

## 7. 风险

1. **迁移不可逆**：DROP TABLE + RENAME。需充分测试（建 v57 测试库 + 数据 → 迁移 → 验证 UNIQUE + 数据完整）。
2. **is_deleted 时间戳跨设备不一致**：A 删集时刻 T1，B pull 后 is_deleted=T1（A 的时刻）。两设备 is_deleted 值一致（md5 一致，sync 正常）。T1 是 A 的本地时刻——语义 OK（tombstone 栀记 + 删除时刻元数据），不用于 merge 方向判定（updated_at 管）。
3. **name UNIQUE 复合约束的迁移期**：迁移瞬间若有同名（不可能——v57 UNIQUE(name) 已防），复制会失败。v57 保证无同名，安全。
