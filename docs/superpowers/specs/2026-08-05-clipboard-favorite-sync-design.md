# 剪贴板收藏同步设计

- 日期：2026-08-05
- 分支：`feat/clipboard-favorite-sync`
- 类型：新功能（schema 变更 + 新增 sync 数据类型）
- 依赖：
  - hotword tombstone pattern（`crates/sync/src/hotword.rs`，2026-08-02 GC + 2026-08-05 tombstone 单向优先 fix `052c67cc`）
  - vault crypto symmetric（`crates/vault/src/crypto/symmetric.rs`，AES-256-GCM `v1:` 前缀格式）
  - octopus-sync 通用基建（git wrapper / outline / store 工具）

## 1. 背景与动机

### 1.1 现状

剪贴板历史（`clipboard_history` 表）是本机数据，无跨设备同步。`is_favorite` 标记打在历史行上，收藏列表只在本机有效。用户在 A 机收藏的常用话术 / API key / 重要转写，到 B 机看不到。

vault / hotword 已有成熟的 git sync 基建（`octopus-sync` crate），剪贴板是唯一未接入 sync 的主要数据类型。

### 1.2 设计约束（brainstorming 确认）

| 决策 | 结论 | 理由 |
|---|---|---|
| 同步范围 | **仅 favorite**，不同步全量历史 | 全量历史噪音 > 信号（80% 是一次性粘贴）；favorite 是用户主动收藏的高价值子集 |
| 同步类型 | **仅文本类**（text / voice / ocr），不含 image / file | image 是二进制 blob（git 不适合）；file 存本地路径（跨设备无意义） |
| 数据模型 | **独立 favorites 表**，只存 uuid + history_id + is_deleted | 内容真相始终在 clipboard_history；favorites 表是同步锚点 + tombstone |
| 主键 | **clipboard_history.id 改 UUID v4**（TEXT） | 跨设备全局唯一，不撞车；和 hotword/vault 同款（UUID 主键 + sync 锚点） |
| 内容流 | **sync 文件带加密内容**，pull 时 UPSERT clipboard_history | A 机收藏 → push 加密内容 → B 机 pull → 内容对齐进历史表 + favorites 表关联 |
| 加密 | **AES-256-GCM**，独立 clipboard.key（不是 vault user_vault_key，不依赖 vault 解锁） | model secret key 的 machine-key 是本机唯一不能跨设备；clipboard.key 明文存 git repo（先防君子，key 交换后续 follow-up） |
| 迁移策略 | **清表重建**（不兼容老数据） | 用户确认可以清表；避免 INTEGER→TEXT 迁移 + FTS5 rowid 适配的复杂度 |

### 1.3 非目标

- ❌ 不同步全量剪贴板历史（噪音 > 信号）
- ❌ 不同步 image / file 类型（二进制 / 路径问题）
- ❌ 不做 clipboard.key 的安全交换（后续 follow-up——非对称协商 / vault 派生 / 其他）
- ❌ 不依赖 vault 解锁（clipboard.key 独立于 user_vault_key）
- ❌ 不做老数据迁移（清表重建）

---

## 2. Schema 变更

### 2.1 clipboard_history 主键改 UUID

```sql
-- 旧：INTEGER PRIMARY KEY（毫秒戳，本机唯一）
-- 新：TEXT PRIMARY KEY（UUID v4，跨设备全局唯一）
CREATE TABLE IF NOT EXISTS clipboard_history (
    id              TEXT PRIMARY KEY,           -- UUID v4（原 INTEGER 毫秒戳）
    item_type       TEXT    NOT NULL,
    content         TEXT    NOT NULL DEFAULT '',
    ref_data        TEXT,
    meta_info       TEXT,
    is_favorite     INTEGER NOT NULL DEFAULT 0,
    is_rich         INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL,
    has_thumbnail   INTEGER NOT NULL DEFAULT 0,
    segments        TEXT,
    is_deleted      INTEGER NOT NULL DEFAULT 0
);
```

**排序变更**：原来 `ORDER BY id DESC`（毫秒戳降序 = 时间倒序）→ 改为 `ORDER BY created_at DESC, id DESC`（UUID 无时间序，显式按 created_at 排序）。

**FTS5 适配**：`clipboard_history_fts` 虚表原来用 `rowid` 关联历史表 id（整数毫秒戳）。id 改 TEXT 后，FTS5 用 SQLite 隐式 `rowid`（自增整数），trigger 改为用 `NEW.rowid`（SQLite 内部 rowid）而非 `NEW.id`。

### 2.2 clipboard_favorites 新表

```sql
CREATE TABLE IF NOT EXISTS clipboard_favorites (
    id          TEXT PRIMARY KEY,               -- UUID v4（同步锚点，跨设备稳定）
    history_id  TEXT NOT NULL,                  -- 指向 clipboard_history.id（UUID，跨设备一致）
    is_deleted  INTEGER NOT NULL DEFAULT 0,     -- 0=活跃，>0=删除时刻 epoch 秒（tombstone）
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL,
    sync_md5    TEXT,                            -- md5 内容指纹（增量同步 diff）
    UNIQUE(history_id, is_deleted),             -- 活跃同一 history 唯一；tombstone 各带时间戳不冲突
    FOREIGN KEY (history_id) REFERENCES clipboard_history(id)
);

CREATE INDEX IF NOT EXISTS idx_clip_fav_active ON clipboard_favorites(is_deleted) WHERE is_deleted = 0;
```

---

## 3. .sync/clipboard/ 目录布局

```
~/.octopus/.sync/clipboard/
├── clipboard.key              ← AES-256-GCM 32B key（hex 编码，明文存——先防君子）
├── outline.json               ← 总 outline：favorites 的 md5 + updated_ms
└── favorites/
    └── <2hex>/<uuid>.json      ← 单条 favorite 加密文件（256 桶分片，对称 vault/hotword）
```

### 3.1 clipboard.key

- 首次启用 clipboard favorite sync 时检查是否存在
- 不存在 → `OsRng` 生成 32B 随机 → hex 编码写入文件
- 存在 → 读入
- 文件格式：纯 hex 字符串（64 字符），无 JSON 包装

### 3.2 outline.json

```json
{
  "version": 1,
  "favorites": {
    "<uuid>": {
      "md5": "<md5-hex>",
      "updatedMs": 1722835200000
    }
  }
}
```

字段 camelCase（Tauri 边界 casing 规范——sync 持久化对齐 vault sync 2026-07-28 casing 统一）。

### 3.3 favorites/<2hex>/<uuid>.json

```json
{
  "version": 1,
  "id": "<uuid-v4>",
  "isDeleted": 0,
  "encryptedPayload": "v1:<base64>",
  "createdAt": "2026-08-05T10:00:00",
  "updatedAt": "2026-08-05T10:00:00"
}
```

`encryptedPayload` 用 clipboard.key AES-256-GCM 加密（`v1:` 前缀格式，复用 `octopus_vault::crypto::symmetric`）。解密后是 JSON：

```json
{
  "historyRow": {
    "id": "<uuid-v4>",
    "itemType": "text",
    "content": "实际内容",
    "refData": null,
    "metaInfo": "{\"charCount\":42}",
    "isRich": false,
    "createdAt": "2026-08-05T10:00:00",
    "segments": null
  },
  "favoriteId": "<uuid-v4>",
  "contentHash": "<md5-hex>"
}
```

**加密字段选择**：只加密 `encryptedPayload`（含内容 + 元数据），`id` / `isDeleted` / `createdAt` / `updatedAt` 明文——sync merge 需要读这些字段判断方向（tombstone 检查 / 时间戳比较），不应解密才能读。

---

## 4. 数据流

### 4.1 收藏（本机操作）

```
用户在历史列表点收藏：
  1. 生成 favorite uuid = Uuid::new_v4()
  2. INSERT clipboard_favorites (id=uuid, history_id=<当前行id>, is_deleted=0, ...)
  3. UPDATE clipboard_history SET is_favorite=1 WHERE id=<当前行id>
```

### 4.2 取消收藏（本机操作）

```
用户在收藏列表点取消收藏：
  1. UPDATE clipboard_favorites SET is_deleted=<epoch_secs>, updated_at=datetime('now')
     WHERE history_id=<行id> AND is_deleted=0
  2. UPDATE clipboard_history SET is_favorite=0 WHERE id=<行id>
```

### 4.3 sync push（导出到 .sync 文件）

```
merge_clipboard_favorites() 阶段 push：
  对每个 active favorite（is_deleted=0）:
    1. JOIN clipboard_history 取行数据
    2. 构造 payload JSON {historyRow, favoriteId, contentHash}
    3. clipboard.key 加密 → encryptedPayload
    4. 写 favorites/<2hex>/<uuid>.json
    5. 更新 outline

  对每个 tombstone favorite（is_deleted>0，未超期）:
    1. 同样写文件（is_deleted=epoch），传播删除意图
    2. outline 保留 entry（让远端知道这是 tombstone）
```

### 4.4 sync pull（从 .sync 文件导入）

```
merge_clipboard_favorites() 阶段 pull：
  对每个 outline entry (uuid):
    1. 读 favorites/<2hex>/<uuid>.json
    2. 提取 isDeleted（明文，不用解密）

    3. if isDeleted > 0（远程 tombstone，未超期）:
       → pull tombstone 到 DB：
         UPDATE clipboard_favorites SET is_deleted=<epoch>, updated_at=...
         UPDATE clipboard_history SET is_favorite=0

    4. elif DB 无此 favorite uuid:
       → 解密 encryptedPayload
       → UPSERT clipboard_history（id=historyRow.id，内容对齐）
       → INSERT clipboard_favorites（id=favoriteId, history_id=historyRow.id）

    5. elif remote_updated > local_updated:
       → 解密 + UPSERT clipboard_history + UPSERT clipboard_favorites

    6. elif local_updated > remote_updated:
       → push（但先检查 remote_is_tombstone——tombstone 单向优先 fix）
       → 如果 remote 是 tombstone，走 pull 路径（删除单向传播）

    7. else（时间戳相等）:
       → md5 比对，冲突 DB 赢
```

### 4.5 展示收藏列表

```sql
SELECT f.id AS favorite_id, h.*
FROM clipboard_favorites f
JOIN clipboard_history h ON f.history_id = h.id
WHERE f.is_deleted = 0
ORDER BY f.created_at DESC
```

### 4.6 cleanup 不影响 favorite

已有的 cleanup 逻辑（`crates/clipboard/src/cleanup.rs`）按天数 / 数量清理时，条件是 `WHERE is_favorite = 0 AND is_deleted = 0`——不清理 `is_favorite=1` 的历史行。favorite sync 拉来的历史行 `is_favorite=1`，自然不会被清。

---

## 5. merge 逻辑（套 hotword pattern + tombstone 单向优先 fix）

### 5.1 与 hotword merge 的对称性

本设计直接复用 hotword merge 的核心 pattern：

| 要素 | hotword 实现 | clipboard favorite 实现 |
|---|---|---|
| 主键 | UUID v4 | UUID v4 |
| tombstone | `is_deleted` epoch 秒 | `is_deleted` epoch 秒 |
| UNIQUE 约束 | `UNIQUE(name, is_deleted)` | `UNIQUE(history_id, is_deleted)` |
| outline | `{sets: {uuid: {md5, updated_ms}}}` | `{favorites: {uuid: {md5, updated_ms}}}` |
| 文件分片 | `<2hex>/<uuid>.json` | `<2hex>/<uuid>.json` |
| tombstone 单向优先 | `052c67cc` fix | 同款（2026-08-05 fix） |
| GC 过期硬删 | 10 天 retention + scheduler | 不做（favorite tombstone 无 GC，永久保留——数量小） |

### 5.2 tombstone 单向优先（关键不变量）

与热词 fix（`052c67cc`）+ vault fix（`7da97a01`）完全对称：

**删除是单向终态**——远程已 tombstone 时，无论本地时间戳多新都 pull tombstone，不把本地 active 写回文件覆盖远程 tombstone。

```rust
// merge 阶段 1 的 Some(db_f) 分支：
let remote_is_tombstone = read_favorite_file(uuid)
    .map(|f| f.is_deleted > 0)
    .unwrap_or(false);
if remote_is_tombstone {
    // pull tombstone 到 DB（不 push active 覆盖文件）
    pull_favorite(uuid)?;
} else if remote_updated > local_updated {
    // 正常 pull
} else if local_updated > remote_updated {
    // 正常 push
}
```

### 5.3 不做 GC

与热词（10 天 retention）不同，clipboard favorite tombstone **不做 GC 硬删**——理由：
- favorite 数量小（几十条级别），tombstone 永久保留不占空间
- 避免 GC 后跨设备「DB 无 + outline 无 → push 复活」路径（热词 GC 后仍有此风险，靠 GC 时机 + retention 缓解）
- 简化实现——少一个 scheduler task + 少一个 retention 常量

---

## 6. clipboard.key 管理

### 6.1 生成与存储

```
首次启用 clipboard favorite sync（或 enable_sync 时）：
  1. 检查 .sync/clipboard/clipboard.key 是否存在
  2. 不存在 → OsRng 生成 32B → hex 编码（64 字符）→ 写文件 0600
  3. 存在 → 读入 hex → 解码为 32B key
```

### 6.2 跨设备 key 一致性

- A 机生成 clipboard.key → commit + push
- B 机 pull → 从 git 拿到 clipboard.key → 用同一个 key 解密 favorite 内容
- **先防君子**：key 明文在 git repo（私有库守卫已强制拒绝公有库），content 加密——casual `cat` 看不到原文
- **后续 follow-up**：key 安全交换方案（非对称协商 / vault user_vault_key 派生 / 其他），不在本次 spec 范围

### 6.3 加解密 API

复用 vault crypto symmetric（不依赖 vault 解锁，只用 AES-256-GCM 算法 + `v1:` 格式）：

```rust
use octopus_vault::crypto::symmetric::SymmetricKey;

// clipboard.key (32B) → SymmetricKey
let key = SymmetricKey::from_bytes(&key_bytes);
let encrypted = key.encrypt(plaintext_json.as_bytes())?;  // → "v1:<base64>"
let decrypted = key.decrypt(&encrypted)?;                  // → Zeroizing<Vec<u8>>
```

---

## 7. crate 架构

### 7.1 新增模块

| 位置 | 职责 |
|---|---|
| `crates/sync/src/clipboard.rs` | clipboard favorite sync（outline / 文件读写 / merge），对称 `crates/sync/src/hotword.rs` |
| `crates/clipboard/src/favorite.rs` | favorite 业务逻辑（收藏 / 取消 / 列表查询 DB 层） |
| `crates/infra/src/db/clipboard_favorite.rs` | `clipboard_favorites` 表 CRUD |
| `crates/desktop/src/vault/vault_secret_access.rs` 旁 | clipboard.key 加解密 chokepoint（或放 `crates/clipboard/src/crypto.rs`） |

### 7.2 改动模块

| 位置 | 改动 |
|---|---|
| `crates/infra/src/db/transcription.rs` | id INTEGER→TEXT；INSERT 用 UUID；ORDER BY created_at |
| `crates/infra/src/db.sql` | schema 更新 + clipboard_favorites 表 |
| `crates/infra/src/db/mod.rs` | 模块声明加 clipboard_favorite |
| `crates/clipboard/src/store.rs` | insert_clipboard_item 改用 UUID |
| `crates/clipboard/src/watcher.rs` | 传 UUID 而非毫秒戳 |
| `crates/vault/src/sync/engine.rs` | sync_now 加 clipboard favorite merge（与 hotword merge 并列） |

### 7.3 sync_now 编排（engine.rs）

在现有 `sync_now` 流程中，vault merge + hotword merge 之后加 clipboard favorite merge：

```rust
// 现有：vault merge + hotword merge
let vault_report = merge_vault()?;
let hotword_report = merge_hotwords()?;
// 新增：
let clipboard_report = merge_clipboard_favorites()?;
```

---

## 8. 影响面追踪

### 8.1 clipboard_history.id INTEGER→TEXT 影响点

| 调用点 | 改动 |
|---|---|
| `crates/clipboard/src/store.rs::insert_clipboard_item` | id 从毫秒戳改 `Uuid::new_v4().to_string()` |
| `crates/clipboard/src/watcher.rs` | 传 UUID |
| `crates/infra/src/db/transcription.rs` 所有 `ORDER BY id DESC` | 改 `ORDER BY created_at DESC, id DESC` |
| `crates/infra/src/db/transcription.rs` INSERT 语句 | id 参数 TEXT |
| `crates/infra/src/db/transcription.rs` FTS5 trigger | `NEW.rowid` 替代 `NEW.id` |
| `crates/desktop/src/clipboard/clipboard_commands.rs` | 接收 / 返回的 id 类型 |
| 前端 TS interface `ClipboardHistoryItem.id` | `number` → `string` |
| `crates/desktop/src/clipboard/clipboard_window.rs` | 粘贴 / 删除命令的 id 参数 |

### 8.2 FTS5 适配

当前 FTS5 trigger（`clip_fts_ai/ad/au`）用 `NEW.id` 作为 rowid 关联。id 改 TEXT 后：

```sql
-- 旧 trigger：
CREATE TRIGGER clip_fts_ai AFTER INSERT ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(rowid, content) VALUES (NEW.id, NEW.content);
END;

-- 新 trigger（用 SQLite 内部 rowid，不用 NEW.id）：
CREATE TRIGGER clip_fts_ai AFTER INSERT ON clipboard_history BEGIN
    INSERT INTO clipboard_history_fts(rowid, content) VALUES (NEW.rowid, NEW.content);
END;
```

查询时 `clipboard_history_fts.rowid` JOIN `clipboard_history.rowid`（而非 `.id`）。

---

## 9. 测试策略

### 9.1 单元测试

| 测试 | 覆盖 |
|---|---|
| favorite 收藏 / 取消 / 列表 | DB CRUD |
| clipboard_history UUID 主键 | INSERT / 查询 / 排序 |
| FTS5 搜索 | rowid 关联正确 |
| clipboard.key 生成 / 读取 | 首次创建 + 复读 |
| encrypt / decrypt round-trip | 加解密一致性 |

### 9.2 sync merge 测试（对称 hotword 测试）

| 测试 | 覆盖 |
|---|---|
| merge_pulls_remote_favorite_to_empty_db | B 机 pull A 机 favorite |
| merge_pushes_local_only_favorite | A 机 push 新收藏 |
| merge_remote_tombstone_not_overwritten_by_local_active | 🔴 tombstone 单向优先（对称 hotword/vault fix） |
| merge_local_delete_not_resurrected_by_remote_active | 本地删 + 远程 active → 不复活 |
| merge_updated_at_remote_wins / db_wins | 时间戳比较 |
| merge_conflict_db_wins | 时间戳相等 + md5 冲突 |

---

## 10. 不变量

1. **clipboard_favorites.id（UUID v4）是跨设备唯一同步锚点**——永不重新生成，收藏 / sync / 取消全程使用同一个 uuid
2. **clipboard_favorites.history_id == clipboard_history.id**——跨设备一致（同一个 UUID），B 机 pull 时直接用 sync 文件里的 historyRow.id UPSERT
3. **tombstone 单向优先**——远程 tombstone 时，本地 active 不能覆盖（对称 hotword `052c67cc` + vault `7da97a01` fix）
4. **cleanup 不清 is_favorite=1 的行**——已有逻辑不变，favorite sync 拉来的行 is_favorite=1 自然受保护
5. **clipboard.key 在 sync 前必须存在**——不存在则首次生成 + commit + push
6. **仅 text/voice/ocr 类型可收藏同步**——image/file 类型的 is_favorite 标记仍可本机使用，但 sync 时跳过

---

## 11. 后续 follow-up（不在本次范围）

| 项 | 说明 |
|---|---|
| clipboard.key 安全交换 | 当前明文存 git（先防君子）。后续可考虑非对称协商 / vault user_vault_key 派生 / ECDH |
| image favorite 同步 | 二进制 blob 经 git 同步（需 LFS 或外部存储），与 vault attachment 同源问题 |
| favorite 分组 / 标签 | 收藏列表变长后的组织需求（参考 CopyQ tab / EcoPaste-Pro 智能分组） |
| favorite 搜索 | 复用 FTS5（clipboard_history_fts 已索引 content），favorite 列表加搜索框 |
