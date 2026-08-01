# 热词存储拆分单记录 + sync 重构

> **日期**：2026-08-01
> **状态**：✅ 已实现（存储拆分 + set 级 sync；word 级 merge 待后续）
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
    sync_md5    TEXT,
    UNIQUE(set_id, word)
);
CREATE INDEX IF NOT EXISTS idx_hotword_words_set ON hotword_words(set_id);
```

**业务键** = `(set_id, word)` 复合唯一约束。同词跨 set 是独立记录（各自 UUID / is_deleted / updated_at）。

**`hotword_sets` 变更**：DROP `words_text` 列（迁移后，已查清无索引/触发器/视图引用）。`hotword_sets` 只保留元数据：`id / name / enabled / created_at / updated_at / sync_md5`。

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

### 文件布局

```
~/.octopus/.sync/hotword/
├── outline.json                    ← 词级 outline（uuid → {md5, updated_ms}）
├── sets/<2hex>/<set-uuid>.json     ← set 元数据（name/enabled，不再含 words_text）
└── words/<2hex>/<word-uuid>.json   ← 每词一个文件（新）
```

复用 `shard_dir`（UUID 前 2 hex，256 桶），加 path traversal 校验（对齐 vault，当前 hotword 缺）。

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
    is_deleted: bool,
    created_at: String,
    updated_at: String,
}
```

### md5 指纹（长度前缀分隔防 `|` 碰撞）

```
{set_id_len}|{set_id}|{word_len}|{word}|{pinyin_len}|{pinyin}|{is_deleted}
```

词文本可能含任意 Unicode（含 `|`），用长度前缀分隔（vault 靠密文 base64 不含 `|`，热词明文必须防碰撞）。**不含 created_at/updated_at**（时间戳跨设备必然不同）。

### merge 逻辑（对称 vault merge_vault）

每条 word 记录：
- outline 有 + DB 无 → pull（.sync → DB）
- DB 有 + outline 无 → push（DB → .sync）
- 都有 → 比 updated_ms：remote > local → pull；local > remote → push；相等 → md5 比对，不同 → DB 赢（conflict），相同 → 跳过

set 级 merge 保留（set 元数据 name/enabled 仍需同步）。merge 后从 DB 重建 outline。

### 软删

`remove_word` = `UPDATE hotword_words SET is_deleted=1, updated_at=now`。文件不删，走标准 merge 路径（is_deleted 参与 md5）。`list_words_in_set` / `list_active_words` 过滤 `is_deleted=0`。

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
