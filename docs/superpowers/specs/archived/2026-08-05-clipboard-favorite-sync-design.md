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
| 数据模型 | **独立 favorites 表**（极简 4 字段：`history_id` PK + `is_deleted` + `updated_at` + `sync_md5`，无独立 id / 无 created_at / 无 FK） | 内容真相始终在 clipboard_history；favorites 表是状态标记 + 同步锚点 + tombstone（详见 §2.2 + 实现记录） |
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

**极简 4 字段**（实现简化后——原 6 字段方案见下方「设计演进」）：

```sql
CREATE TABLE IF NOT EXISTS clipboard_favorites (
    history_id  TEXT PRIMARY KEY,               -- = clipboard_history.id（一对一，无独立 id）
    is_deleted  INTEGER NOT NULL DEFAULT 0,     -- 0=活跃，>0=删除时刻 epoch 秒（tombstone）
    updated_at  TEXT NOT NULL,                  -- sync 时间戳比较用
    sync_md5    TEXT                            -- md5 内容指纹（检测 history 行编辑）
);
CREATE INDEX IF NOT EXISTS idx_clip_fav_active ON clipboard_favorites(is_deleted) WHERE is_deleted = 0;
```

**设计演进（6 → 4 字段）**：实现期间认识到 favorite 不需要独立身份——它只是「此 history 行被收藏」的状态标记 + 同步锚点。简化要点：

| 原字段 | 处置 | 理由 |
|---|---|---|
| `id TEXT PRIMARY KEY` | **删除**——history_id 直接作 PK | favorite 无独立身份，1:1 关联 history，history_id 即唯一同步锚点（与 vault/hotword 的 UUID 主键不同——它们是 1:N，clipboard favorite 是 1:1） |
| `created_at TEXT` | **删除**——用 clipboard_history.created_at | 内容真相在 history 行，favorite 只是状态标记；不重复存 |
| `UNIQUE(history_id, is_deleted)` | **删除** | history_id 已是 PK，天然唯一——复合约束冗余 |
| `FOREIGN KEY (history_id) REFERENCES clipboard_history(id)` | **删除**（**关键**） | cleanup 物理删除 history 行时不能连带删 favorite——favorite tombstone 必须存活到 sync 传播删除意图。FK 级联会导致 cleanup 提前清掉 tombstone |

**is_deleted 语义**：0=active，>0=epoch 秒 tombstone（与 hotword set/word 一致）。**永久保留，不做 GC**（详见 §5.3）。

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

文件名 `<uuid>` = history_id（同步锚点，简化 3 字段 schema 后 favorite 不再有独立 uuid）。

```json
{
  "version": 1,
  "id": "<history_id>",
  "isDeleted": 0,
  "encryptedPayload": "v1:<base64>",
  "updatedAt": "2026-08-05T10:00:00"
}
```

`encryptedPayload` 用 clipboard.key AES-256-GCM 加密（`v1:` 前缀格式）。解密后是 `FavoritePayload`：

```json
{
  "historyRow": {
    "id": "<history_id>",
    "itemType": "text",
    "content": "实际内容",
    "refData": null,
    "metaInfo": "{\"charCount\":42}",
    "isRich": false,
    "createdAt": "2026-08-05T10:00:00",
    "segments": null
  }
}
```

**实现简化**：原设计 `FavoritePayload` 还含 `favoriteId` + `contentHash` 字段，简化后**只剩 `history_row`**——history_id 即 favorite 的 id（无独立 favoriteId），contentHash 由 sync 调用方按需从 `history_row` 计算（不持久化到 payload）。`ClipboardFavoriteFile` 也无 `created_at` 字段（与 §2.2 简化一致——created_at 用 history 行自己的）。

**加密字段选择**：只加密 `encryptedPayload`（含内容 + 元数据），`id` / `isDeleted` / `updatedAt` 明文——sync merge 需要读这些字段判断方向（tombstone 检查 / 时间戳比较），不应解密才能读。

**Tombstone 空 payload**（实现期间发现）：`is_deleted > 0`（tombstone）的文件 `encryptedPayload = ""`（空字符串）——tombstone 的唯一目的是传播「此 history_id 已删除」，内容已无意义，写空省去加密开销 + 防止 history 行已物理删除时无法 export。pull 时 tombstone 不解密、不还原 history 内容（详见 §4.4）。

---

## 4. 数据流

### 4.1 收藏（本机操作）

```
用户在历史列表点收藏：
  1. INSERT clipboard_favorites (history_id=<当前行id>, is_deleted=0, updated_at=datetime('now'))
     — history_id 即 PK + 同步锚点，不生成独立 uuid
  2. UPDATE clipboard_history SET is_favorite=1 WHERE id=<当前行id>
```

### 4.2 取消收藏（本机操作）

```
用户在收藏列表点取消收藏：
  1. UPDATE clipboard_favorites SET is_deleted=<epoch_secs>, updated_at=datetime('now')
     WHERE history_id=<行id>
  2. UPDATE clipboard_history SET is_favorite=0 WHERE id=<行id>
```

**恢复收藏**（tombstone → active）：`UPDATE clipboard_favorites SET is_deleted=0, updated_at=datetime('now') WHERE history_id=<行id>`（不重新 INSERT，原 tombstone 行原地复活，updated_at 变化驱动 sync 传播）。

### 4.3 sync push（导出到 .sync 文件）

```
merge_clipboard_favorites() 阶段 push（DB → 文件）：
  对每个 active favorite（is_deleted=0）:
    1. JOIN clipboard_history 取行数据（不存在时写占位 payload，warn 日志）
    2. 构造 FavoritePayload {history_row: HistoryRowJson}
    3. clipboard.key 加密 → encryptedPayload
    4. 写 favorites/<2hex>/<history_id>.json（id=history_id）
    5. 更新 outline（md5 + updated_ms）

  对每个 tombstone favorite（is_deleted>0）:
    1. 写文件，encryptedPayload=""（空字符串）——tombstone 内容无意义，不加密
    2. outline 保留 entry（让远端知道这是 tombstone）
```

### 4.4 sync pull（从 .sync 文件导入）

```
merge_clipboard_favorites() 阶段 pull（文件 → DB）：
  对每个 outline entry (history_id):
    1. 读 favorites/<2hex>/<history_id>.json
    2. 提取 isDeleted（明文，不用解密）

    3. if isDeleted > 0（远程 tombstone）:
       a. DB active → soft_delete_favorite + history.is_favorite=0
       b. DB 无 → 直接 INSERT tombstone favorite（不 decrypt payload、不还原 history）
          —— 幂等清掉 history.is_favorite=0（history 行可能存在且 is_favorite=1）
       c. DB 已是 tombstone → skip（已是终态）

    4. elif DB 无此 favorite:
       → 解密 encryptedPayload（active 文件必有 payload，校验 history_row.id == 文件 id）
       → UPSERT clipboard_history（id=historyRow.id，内容对齐）
       → UPSERT clipboard_favorites（is_deleted=0, sync_md5=内容指纹）
       → history.is_favorite=1

    5. elif remote_updated > local_updated:
       → 解密 + UPSERT history + UPSERT favorite

    6. elif local_updated > remote_updated:
       → push（但先检查 remote_is_tombstone——tombstone 单向优先 fix）
       → 如果 remote 是 tombstone，走 pull 路径（删除单向传播）

    7. else（时间戳相等）:
       → md5 比对（DB sync_md5 vs outline md5），冲突 DB 赢
```

**Tombstone 空 payload 设计**（实现期间发现）：
- export：tombstone 写 `encrypted_payload=""`（空）——内容无意义，且 history 行可能已被 cleanup 物理删除。
- pull：tombstone + DB 无 → 直接 INSERT tombstone（不 decrypt、不还原 history）；如 history 行残留则同步清掉 `is_favorite=0`。
- 活跃 favorite 必有非空 payload；tombstone 空 payload 是「无内容删除意图」的语义化表示。

### 4.5 展示收藏列表

```sql
SELECT h.*
FROM clipboard_favorites f
JOIN clipboard_history h ON f.history_id = h.id
WHERE f.is_deleted = 0
ORDER BY f.updated_at DESC, h.created_at DESC
```

> 注：`JOIN` 后 history 行可能不存在（cleanup 已物理删除但 favorite tombstone 仍在），此时收藏列表不显示——属于预期行为（cleanup 不清 `is_favorite=1` 行，但 sync pull 拉来的 active favorite 的 history 行 `is_favorite=1`，受保护）。

### 4.6 cleanup 不影响 favorite

已有 cleanup 逻辑（`crates/clipboard/src/cleanup.rs`）按天数 / 数量清理时，条件是 `WHERE is_favorite = 0 AND is_deleted = 0`——不清理 `is_favorite=1` 的历史行。favorite sync 拉来的历史行 `is_favorite=1`，自然不会被清。

**favorite tombstone 永久保留**：即使 history 行被用户手动删除（物理 DELETE），clipboard_favorites 行（tombstone）依然存活，直到下一次 sync 把删除意图传给远端——这是 §2.2 移除 FK 的核心理由（详见「实现记录」）。

---

## 5. merge 逻辑（套 hotword pattern + tombstone 单向优先 fix）

### 5.1 与 hotword merge 的对称性

本设计直接复用 hotword merge 的核心 pattern：

| 要素 | hotword 实现 | clipboard favorite 实现 |
|---|---|---|
| 主键 | UUID v4（独立） | history_id（= clipboard_history.id，1:1，无独立 uuid） |
| tombstone | `is_deleted` epoch 秒 | `is_deleted` epoch 秒 |
| UNIQUE 约束 | `UNIQUE(name, is_deleted)` | 无（history_id PK 天然唯一） |
| FOREIGN KEY | 无 | **无**（cleanup 物理删 history 时 favorite tombstone 必须存活） |
| outline | `{sets: {uuid: {md5, updated_ms}}}` | `{favorites: {history_id: {md5, updated_ms}}}` |
| 文件分片 | `<2hex>/<uuid>.json` | `<2hex>/<history_id>.json` |
| tombstone 单向优先 | `052c67cc` fix | 同款（2026-08-05 fix） |
| 加密 | 明文 | AES-256-GCM（内联 `ClipboardKey`，非 vault `DerivedKey`） |
| tombstone payload | 明文 | `encrypted_payload=""`（空，不加密） |
| GC 过期硬删 | 10 天 retention + scheduler | **不做**（favorite tombstone 数量小，永久保留） |

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

> ⚠️ **已过时（2026-08-11 第三十六轮 P2-D）**：clipboard favorite tombstone GC 已实现——
> scheduler 每日 `purge_expired_clipboard_favorites`（30 天 retention）+ `export_all_favorites`
> 重建 .sync，对齐 hotword GC 范式。跨设备复活风险通过 merge 按年龄过滤（`is_tombstone_expired`）
> 缓解。详见全量审查 spec §40 + architecture.md「octopus-scheduler」。以下为历史决策记录。

与热词（10 天 retention）不同，clipboard favorite tombstone **不做 GC 硬删**——理由：
- favorite 数量小（几十条级别），tombstone 永久保留不占空间
- 避免 GC 后跨设备「DB 无 + outline 无 → push 复活」路径（热词 GC 后仍有此风险，靠 GC 时机 + retention 缓解）
- 简化实现——少一个 scheduler task + 少一个 retention 常量

---

## 6. clipboard.key 管理

### 6.1 生成与存储

```
首次启用 clipboard favorite sync（merge 时按需调用 load_or_create_clipboard_key）：
  1. 检查 .sync/clipboard/clipboard.key 是否存在
  2. 不存在 → OsRng 生成 32B → hex 编码（64 字符）→ 写文件 0600
  3. 存在 → 读入 hex → 解码为 32B key
  4. 文件长度/格式不符 → 返回错误（拒绝降级使用损坏 key）
```

> 注：实现期间 `ClipboardKey` 内联到 sync crate（非复用 vault `DerivedKey`），因循环依赖——详见 §6.3。

### 6.2 跨设备 key 一致性

- A 机生成 clipboard.key → commit + push
- B 机 pull → 从 git 拿到 clipboard.key → 用同一个 key 解密 favorite 内容
- **先防君子**：key 明文在 git repo（私有库守卫已强制拒绝公有库），content 加密——casual `cat` 看不到原文
- **后续 follow-up**：key 安全交换方案（非对称协商 / vault user_vault_key 派生 / 其他），不在本次 spec 范围

### 6.3 加解密 API

**实现简化——内联 `ClipboardKey`**（非复用 vault `DerivedKey`）：

原设计拟复用 `octopus_vault::crypto::symmetric::SymmetricKey`。但实际依赖方向是 `sync ← vault`（vault 已依赖 sync），sync 反向依赖 vault 会形成循环（`sync → vault → sync`）。实现期间改为在 sync crate 内联一份 `ClipboardKey`：

```rust
// crates/sync/src/clipboard.rs（内联 AES-256-GCM，byte-compatible with vault DerivedKey）
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

pub struct ClipboardKey(Zeroizing<[u8; 32]>);

impl ClipboardKey {
    pub fn from_raw(arr: [u8; 32]) -> Self { ... }
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<String> {
        // → "v1:<base64(nonce[12B]||ct||tag[16B])>"，与 vault DerivedKey 同格式
    }
    pub fn decrypt(&self, ciphertext: &str) -> Result<Zeroizing<Vec<u8>>> { ... }
}
```

**byte-compatible 保证**：同样的 32B key + 同样的 `v1:<base64(nonce||ct||tag)>` 格式，未来若 clipboard.key 迁到 vault 体系（如循环依赖解除后），已加密文件无需重新加密。`random_bytes`（OS 熵源 CSPRNG）也内联，避免依赖 `vault::crypto::util`。

**vault 私有函数不可见**：vault `symmetric::SymmetricKey` 是 `pub(crate)`——跨 crate 拿不到。与 `store::write_atomically` 同模式（sync 内联一份原子写工具，因 vault 的 private 跨 crate 拿不到）。

---

## 7. crate 架构

### 7.1 新增模块

| 位置 | 职责 |
|---|---|
| `crates/sync/src/clipboard.rs` | clipboard favorite sync（outline / 文件读写 / merge / **内联 `ClipboardKey` AES-256-GCM**），对称 `crates/sync/src/hotword.rs` |
| `crates/clipboard/src/favorite.rs` | favorite 业务逻辑（收藏 / 取消 / 列表查询 DB 层） |
| `crates/infra/src/db/clipboard_favorite.rs` | `clipboard_favorites` 表 CRUD + `HistoryRowData` 中转结构 + history UPSERT/读取辅助 |

> 注：原设计曾考虑放 `crates/desktop/src/vault/vault_secret_access.rs` 旁或 `crates/clipboard/src/crypto.rs`，最终因循环依赖而内联到 sync crate（见 §6.3）。

### 7.2 改动模块

| 位置 | 改动 |
|---|---|
| `crates/infra/resources/sql/schema.sql` | clipboard_history id TEXT(UUID) + favorites 表 + FTS5 trigger 用 NEW.rowid |
| `crates/infra/src/db/mod.rs` | 模块声明加 clipboard_favorite + `CURRENT_SCHEMA_VERSION` 58→59（破坏性 bail）|
| `crates/infra/src/db/transcription.rs` | id INTEGER→TEXT；INSERT 用 UUID；ORDER BY created_at |
| `crates/clipboard/src/store.rs` | NewClipboardItem.id i64→String；insert_clipboard_item 改用 UUID |
| `crates/clipboard/src/watcher.rs` | 传 UUID 而非毫秒戳 |
| `crates/vault/src/sync/engine.rs` | sync_now 加 clipboard favorite merge（与 hotword merge 并列） |
| `crates/desktop/src/clipboard/clipboard_commands.rs` + 前端 TS | id 类型 i64/number → String（16 处 ripple，详见实现记录） |

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

1. **clipboard_favorites.history_id 即跨设备唯一同步锚点**——直接用 `clipboard_history.id`（UUID v4）作 PK，无独立 favorite id。收藏 / sync / 取消全程用同一个 history_id
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
| recording pipeline 真 UUID | 见「实现记录 4」——当前 recording pipeline 仍用毫秒戳字符串占位，未改真 UUID（推迟） |

---

## 12. 实现记录（2026-08-05 实现期间的设计演进）

实施期间相对本 spec §1-§11 的几处偏差——已回写到对应章节，此处汇总以便快速理解整体变化。

### 12.1 favorites 表从 6 字段简化到 4 字段

原 §2.2 设计 6 字段（`id` + `history_id` + `is_deleted` + `created_at` + `updated_at` + `sync_md5` + `UNIQUE` + `FOREIGN KEY`）。实现期间认识到 favorite 是 1:1 关联 history 的状态标记，无独立身份——简化到 4 字段：

- **删除 `id`**：`history_id` 直接作 PK + 同步锚点
- **删除 `created_at`**：用 `clipboard_history.created_at`，不重复存
- **删除 `UNIQUE(history_id, is_deleted)`**：history_id 已是 PK，复合约束冗余
- **删除 `FOREIGN KEY`**（关键）：cleanup 物理删除 history 行时不能连带删 favorite——favorite tombstone 必须存活到 sync 传播删除意图

最终 schema 4 字段，详见 §2.2。

### 12.2 循环依赖解决——`ClipboardKey` 内联到 sync crate

原 §6 设计拟复用 `octopus_vault::crypto::symmetric::SymmetricKey`。但实际依赖方向是 `sync ← vault`（vault 已依赖 sync crate），sync 反向依赖 vault 会形成 `sync → vault → sync` 循环依赖。

解决：在 sync crate 内联一份 `ClipboardKey`（AES-256-GCM，`v1:` 格式，byte-compatible with vault `DerivedKey`）。`random_bytes`（OS 熵源 CSPRNG）也内联。详见 §6.3。

未来若循环依赖解除（如 sync crypto 抽到独立 crate），clipboard.key 文件无需重新加密——格式完全兼容。

### 12.3 FK 移除——tombstone 必须存活到 sync 传播

原设计 `clipboard_favorites` 有 `FOREIGN KEY (history_id) REFERENCES clipboard_history(id)`。但存在两个致命场景：

1. **cleanup 物理删 history**：cleanup 按 `is_favorite=0 AND is_deleted=0` 清理非收藏行，但用户可能先收藏后取消（favorite 变 tombstone）→ history 行被 cleanup 物理删 → FK 级联/RESTRICT 删 favorite tombstone → 删除意图丢失，第三台机当新收藏复活
2. **历史行手动删除**：用户主动删 history 行，favorite tombstone 必须保留到 sync

移除 FK 让 favorite tombstone 与 history 行的生命周期解耦——tombstone 永久保留（数量小，不做 GC，见 §5.3），靠 sync merge 在远端也变成 tombstone。

### 12.4 tombstone 空 payload 设计

原 §3.3 设计 tombstone 也写完整 encrypted payload。实现期间发现：

- **tombstone 内容无意义**：删除意图传播无需 history 内容
- **history 行可能已物理删**：cleanup 或用户主动删 history 后，export 时 JOIN clipboard_history 拿不到行——无法构建 payload

改为：export tombstone 写 `encrypted_payload=""`（空字符串）。pull 时 tombstone 不解密、不还原 history 内容，仅 INSERT/UPDATE favorite tombstone + 清掉残留 history.is_favorite。详见 §3.3 + §4.4。

### 12.5 FTS5 rowid 适配（id INTEGER→TEXT 后）

原 `clipboard_history.id` 是 INTEGER（毫秒戳），FTS5 虚表用 `content_rowid='id'` 关联 + trigger 用 `NEW.id` 作 rowid。

id 改 TEXT(UUID) 后，FTS5 不能用 TEXT 作 rowid（必须整数）。改为：

- **`content_rowid='id'` 移除**：FTS5 虚表用 SQLite 内部自增 `rowid`（隐式整数）
- **3 个 trigger 改用 `NEW.rowid` / `OLD.rowid`**（SQLite 内部 rowid，不再用 `NEW.id`）

查询时 JOIN 走 `clipboard_history_fts.rowid` ↔ `clipboard_history.rowid`。详见 §2.1 + §8.2。

### 12.6 Desktop cascade：`ClipboardItem.id` i64→String rippled to 16 处

`NewClipboardItem.id` 从 i64 改 String 后，前端 TS interface `ClipboardHistoryItem.id` 从 `number` 改 `string`，rippe 到 16 处文件（粘贴 / 删除 / 收藏 / OCR 入口 / 复制 / 编辑等所有走 id 的命令）。

### 12.7 v58→v59 迁移是 `bail!`（非自动迁移）

`clipboard_history.id` 类型变更（INTEGER→TEXT）无法自动迁移（老数据是毫秒戳，UUID 主键语义不同）。`init_schema` 的迁移分支：

```rust
58 => {
    // 破坏性变更——id 类型变更，老数据无法迁移
    anyhow::bail!("DB schema v58→v59 is a breaking change (clipboard_history.id INTEGER→TEXT UUID). \
                   Run: rm ~/.octopus/octopus.db* (then restart app to rebuild).");
}
```

老库启动直接 bail 提示清库重建（用户已确认接受）。详见 plan §「实际实现偏差」5。

### 12.8 recording pipeline 仍用毫秒戳字符串（推迟 UUID）

`crates/clipboard/src/watcher.rs` 中**录音相关**的 `insert_clipboard_item` 调用仍传 `millis_tid.to_string()`（毫秒戳字符串），未切到真 UUID。原因是录音流程与 voice id 强耦合（`insert_transcription_at_id(id)` 用毫秒戳作 session 标识，改 UUID 影响面大）。

推迟到后续 follow-up——本次仅改非录音路径（text/image/file/ocr）用 UUID。详见 §11 follow-up 表。
