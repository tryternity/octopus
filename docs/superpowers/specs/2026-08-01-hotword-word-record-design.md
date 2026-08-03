# 热词存储拆分单记录 + sync 重构

> **日期**：2026-08-01
> **状态**：✅ 已实现（存储拆分 + set 级 sync + word 级 3-way merge，两级 outline 层级）
> **背景**：`hotword_sets.words_text`（空格分隔大文本）拆成 `hotword_words` 表（每词一条记录），sync 从 set 级降到 word 级 3-way merge（对齐 vault）。驱动：单词级软删（is_deleted）、按 updated_at 增量同步、确定性 UUID 跨设备合并。

## 1. 动机

当前痛点（研究实证）：
- **单词操作是「读-改-写全量」**：add/remove word 要 SELECT words_text → 拼接/过滤 → normalize → UPDATE，无「单词」实体，无单词级 updated_at / is_deleted / sync_md5。
- **md5 指纹粒度是 set 级**：改一词整个 set md5 变，sync 无法定位哪个词变了。
- **删除是硬删**：`delete_hotword_set` 直接 DELETE，无法做软删传播（vault 有 is_deleted，热词无）。
- **多命中不确定**：`list_active_hotword_words` 的 `HashSet.into_iter()` 顺序随机，correct 多命中取哪个取决于哈希运气（排序留后续 spec，但拆分是前提）。

## 2. 数据模型

### 新表 `hotword_words`（schema v57）

```sql
CREATE TABLE IF NOT EXISTS hotword_words (
    id          TEXT PRIMARY KEY,         -- hotword_word_uuid(set_id, word) 确定性
    set_id      TEXT NOT NULL,            -- 逻辑 FK hotword_sets.id
    word        TEXT NOT NULL,            -- 词文本
    pinyin      TEXT NOT NULL DEFAULT '', -- 原始拼音空格分隔 "ba zhao yu"
    is_deleted  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(set_id, word)
);
CREATE INDEX IF NOT EXISTS idx_hotword_words_set ON hotword_words(set_id);
```

**业务键** = `(set_id, word)` 复合唯一约束。同词跨 set 是独立记录（各自 UUID / is_deleted / updated_at）。

**`hotword_sets` 变更**：DROP `words_text` 列（迁移后，已查清无索引/触发器/视图引用）。`hotword_sets` 只保留元数据：`id / name / enabled / is_deleted / created_at / updated_at / sync_md5`（v58 加 `is_deleted` epoch 秒 + `UNIQUE(name, is_deleted)` 复合约束）。

**`hotword_hits` 不变**：全局表 `word → hit_count`，不绑 set。correct 排序留后续 spec。

### UUID：确定性 v5

```rust
pub const HOTWORD_NAMESPACE: Uuid = Uuid::from_u128(0x...固定值);
pub fn hotword_word_uuid(set_id: &str, word: &str) -> String {
    Uuid::new_v5(&HOTWORD_NAMESPACE, format!("{set_id}/{word}").as_bytes()).to_string()
}
```

- SHA1-based 确定性：两设备独立加同词到同词典 → 同 UUID → 按 updated_at 天然合并。
- 原生输出标准带连字符 UUID 格式，复用 `shard_dir`（filter hex take 2）。
- `id` 既做主键也做 sync 文件名，跨设备一致。

### 拼音：存原始不存归一化

- 存 `to_pinyin().plain()`（如「八爪鱼」→ `ba zhao yu`），**不存** `normalize_fuzzy_pinyin` 后结果。
- 理由：方言规则可配置可变，存归一化拼音规则变了 DB 过期。归一化交给运行时 `HotwordIndex::from_words` 生成 key。
- infra 已依赖 `pinyin = "0.10"`，迁移代码直接算（`word_plain_pinyins`），不需调 asr-local。

## 3. sync 模型：word 级 3-way merge

### 文件布局（两级 outline 层级——2026-08-01 实现版）

```
~/.octopus/.sync/hotword/
├── outline.json                 ← 总 outline：只描述词典状态
│     { version, hotwordVersion, sets: { <setUuid>: {md5, updatedMs} } }
└── <set-uuid>/                  ← 每个词典一个目录（目录名 = 词典 ID）
    ├── meta.json                ← 词典元数据（name/enabled/createdAt/updatedAt）
    ├── outline.json             ← 本词典的词状态
    │     { words: { <wordUuid>: {md5, updatedMs} } }
    └── <2hex>/<word-uuid>.json  ← 词文件（按词 UUID 前2位分桶）
```

**为什么两级而非扁平**：① 3 万词条拆成 N 个 3 千项的词典 outline，git diff 只碰改动词典；
② 删词典 = `rm -r <set-id>/` 原子完整；③ 语义干净——总 outline 管词典，词状态归属各自词典。
**词文件名用 UUID**（=v5(set_id,word)，软删/改拼音不变），内容 MD5 做 outline 变化指纹。
（原设计是扁平 `sets/ + words/` 共用一个 outline，实现时改为两级——见 plan 实施记录。）

复用 `shard_dir`（UUID 前 2 hex，256 桶），`validate_hotword_uuid` 校验 set 目录名 + word 文件名
（对齐 vault path traversal 防护）。`write_atomically` 原子写（对齐 vault 持久化保证）。

### HotwordWordFile 格式

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HotwordWordFile {
    version: u32,
    id: String,
    set_id: String,
    word: String,
    pinyin: String,
    /// 0=活跃，>0=删除时刻 epoch 秒（tombstone）。统一语义（GC 2026-08-02，原 bool 0/1）。
    is_deleted: i64,
    created_at: String,
    updated_at: String,
}
```

### md5 指纹（身份指纹，不含状态字段）

**set 级 md5** = `md5(id|name)`——身份指纹，不含 enabled/is_deleted/时间戳。
set name 可改（rename），md5 变化触发 outline diff 判断改名。状态变化（删除/启用）靠 updated_at 时间戳比较。

**word 级无 sync_md5**（2026-08-02 去掉）——word id = `uuid_v5(namespace, "{set_id}/{word}")` 已是确定性业务主键。
word 不可改名、拼音是附属品（`pinyin = f(word)`），id 已唯一标识记录，sync_md5 完全冗余。
DB 列已删除（db.sql 建表 SQL 不含 sync_md5），代码层不读写。
注：outline 增量 export 仍用 `hotword_word_md5(set_id, word)` 现算词指纹做文件 diff（写到词典内 outline.json），但 merge 阶段时间戳相等时跳过不比较。

### merge 逻辑（两阶段，对称 vault merge_vault）

**阶段 1：set 层 merge**（词典元数据，遍历总 outline.sets）
- 总 outline 有 + DB 无 → pull（读 meta.json → upsert DB）
- 都有 → 比 updated_ms：remote > local → pull；local > remote → push；相等 → md5 比对（判断改名），不同 → DB 赢
- DB 有 + outline 无 → push（写 meta.json）

**阶段 2：word 层 merge**（每个词典的词数据，遍历 DB 词典 → 读词典内 outline.words）
- 词典 outline 有 + DB 无 → pull（读词文件 → upsert_hotword_word）
- 都有 → 比 updated_ms：remote > local → pull（软删传播：is_deleted pull 后 DB 也变软删）；local > remote → push；**相等 → 跳过**（word 不可变，id=f(set_id,word) 确定，时间戳相等 = 无变化）
- DB 有 + 词典 outline 无 → push（写词文件）

merge 完后从 DB 最新状态重建所有文件 + outline（`export_all_hotwords`，DB 是单一真相源）。

**删除后不立即 export**：`delete_hotword_set` 只更新 DB + reload corrector 索引，
不调 `export_all_hotwords`（全量重建太慢，删除大词典时卡顿）。
同步是 scheduler 定时器的事——下次 merge 时 DB 的 updated_at 更新 → DB 赢 → push 软删态到 .sync。


### 软删

`remove_word_from_set` = `UPDATE hotword_words SET is_deleted=now_secs, updated_at=now`（is_deleted 存删除时刻 epoch 秒，v58 统一语义）。
文件不删（UUID 不含 is_deleted，文件名不变），走标准 merge 路径。
`list_words_in_set` / `list_active_words` 过滤 `is_deleted=0`。

## 4. HotwordIndex 适配

- `from_words` 改接收带原始拼音的结构（word + pinyin），不再现算 `char_fuzzy_pinyin` 的 `to_pinyin()` 部分——但**仍需 `normalize_fuzzy_pinyin` 归一化生成 key**（方言规则运行时生效）。
- `reload_hotwords` 调 `list_active_words()`（带 pinyin），传给 from_words。
- corrector 的 find_candidates / correct_greedy / drain_hits **不变**（排序留后续）。

## 5. 迁移 v56→v57

`init_schema` while 循环加 `56 => { ... }` 分支：
1. CREATE hotword_words 表 + idx
2. `SELECT id, words_text FROM hotword_sets`（旧表仍有 words_text）→ 每词 `hotword_word_uuid` + `word_plain_pinyins(word).join(" ")` → INSERT OR IGNORE
3. `ALTER TABLE hotword_sets DROP COLUMN words_text`
4. 日志记录迁移词数

**不可逆**：DROP words_text 后无法回退。迁移前建议备份 `~/.octopus/octopus.db`。

## 6. 不在范围（留后续）

- **correct 多命中排序**：hotword_words 已带元数据，后续 spec 做（hit_count JOIN 排序候选）。
- **拼音热路径缓存**：correct 内 per-char 拼音缓存，性能优化，后续。
- **单词级 hit_count**：hotword_hits 仍全局表。

## 7. 风险

1. **迁移不可逆**：DROP words_text。迁移代码需充分测试（建 v56 测试库 + 数据 → 迁移 → 验证）。
2. **sync breaking change**：旧 `.sync/hotword/sets/` 文件格式变（set 文件去 words_text）+ 新增 `words/` 目录。旧客户端不兼容。
3. **拼音迁移期**：迁移用 `word_plain_pinyins`（原始），运行时 `normalize_fuzzy_pinyin`（归一化）不同层——确认 from_words 正确处理「DB 存原始、内存归一化」。
