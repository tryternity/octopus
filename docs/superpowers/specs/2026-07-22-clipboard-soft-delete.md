# 剪贴板软删除与回收站设计（Clipboard Soft Delete & Trash）

> **日期**：2026-07-22
> **状态**：已实现（cargo check 0 error 0 warning；tsc 0 error；clipboard 17 test pass；infra 158 test pass）
> **关联**：DB schema v46→v47；架构文档见 `docs/architecture.md` §「octopus-clipboard」

---

## 0. 目标与范围

### 0.1 核心目标

为剪贴板文本类条目引入「删除进回收站」的软删机制：

1. **文本类条目（text/voice/ocr/file）删除时不立即物理删**——写 `deleted_at` 时间戳进回收站，用户可在回收站里还原或永久删除。
2. **图片（image）仍立即物理删**——受 `image_data` 引用计数约束（软删行还在 → refcount 不归零 → blob 泄漏）。
3. **热词挖掘来源不中断**——软删的核心目的就是保留热词来源，`list_recent_text` 故意不过滤 `deleted_at`。
4. **回收站 tab 仅出现在设置页 ClipboardPanel**（浮窗不暴露）。

### 0.2 范围

| 包含 | 不包含 |
|---|---|
| DB schema v47 加 `deleted_at` 列 + 索引 | 自动过期回收站（永久删的 TTL，P2） |
| 软删 / 还原 / 永久删 / 清空回收站 5 个新命令 | 回收站容量上限 |
| 默认删除入口改软删语义（前端调用不变） | 回收站搜索（FTS 仍可命中软删行） |
| 清理任务（cleanup）按 item_type 分流 | 跨设备同步软删态（P2） |

---

## 1. 不变量

### INV-C1：热词来源不断（第一优先级）

`list_recent_text`（`crates/infra/src/db.rs`）**故意不过滤 `deleted_at`**。软删的核心目的就是保留热词来源：用户把文本删进回收站后，行还在 → 仍被热词挖掘读到。只有「永久删除」（`DELETE FROM`）才会让行真正消失、挖不到。

```sql
SELECT content FROM clipboard_history
WHERE item_type IN ('voice','text','ocr') AND content IS NOT NULL AND content != ''
-- 故意不过滤 deleted_at（INV-C1：软删内容仍是热词来源）
ORDER BY id DESC LIMIT ?1
```

### INV-C2：FTS 自动保留

FTS5 触发器 `clip_fts_ad` 只绑 `AFTER DELETE`。软删走 `UPDATE deleted_at`，不触发任何 FTS trigger → 索引项保留。软删内容仍可被搜索命中（回收站里可搜），且无需在软删/还原时维护索引。

### INV-C3：图片物理删

`image_data` 表靠 `clipboard_history.ref_data` 引用计数（`delete_image_if_unreferenced` 检查是否还有其他行引用同一 hash）。软删行还在 → refcount 不归零 → blob 永不回收 → 泄漏。因此 **图片（item_type='image'）一律物理 DELETE**。

### INV-C4：回收站隔离

`build_where` 中除 `"trash"` 外的所有 filter 都追加 `AND deleted_at IS NULL`；`"trash"` 反向过滤 `deleted_at IS NOT NULL`。确保软删内容只在回收站 tab 返回。

| # | 不变量 | 落地点 |
|---|---|---|
| INV-C1 | 热词挖掘不过滤 `deleted_at` | `db.rs::list_recent_text` SQL 内联注释 |
| INV-C2 | 软删不触发 FTS trigger，索引自动保留 | `db.sql` clip_fts_ad 仅 AFTER DELETE |
| INV-C3 | 图片永远物理删（image_data refcount 约束） | `store.rs` 所有删除入口 is_image_item 分流 |
| INV-C4 | 软删行只在 filter="trash" 返回 | `store.rs::build_where` |

---

## 2. 数据模型变更

### 2.1 Schema v46 → v47

`clipboard_history` 表新增一列 + 一个索引（`crates/infra/src/db.sql`）：

```sql
deleted_at TEXT DEFAULT NULL          -- 软删时间戳（v47）。NULL=活跃；非空=已进回收站。
CREATE INDEX IF NOT EXISTS idx_clip_deleted ON clipboard_history(deleted_at);
```

### 2.2 迁移（`crates/infra/src/db.rs::init_schema`）

迁移分三条路径（按 DB 版本）：

```text
// 路径 A：v >= 47 → 已最新，直接返回
if v >= 47 { return Ok(()) }

// 路径 B：v == 46 → 最常见老用户场景（v46 是上个稳定版）
// 补 deleted_at 列后直接升 v47，跳过不需要的 v44/v45/v46 迁移段
if v == 46 {
    1. 检查 clipboard_history 表存在（测试库可能只有 hotword_sets）
    2. 检查 deleted_at 列不存在
    3. ALTER TABLE clipboard_history ADD COLUMN deleted_at TEXT
    4. PRAGMA user_version = 47; return
}

// 路径 C：v <= 45 → 走完整 v44→v45→v46→v47 迁移链
// v44/v45/v46 段跑完后 fall through 到 v47 段（同路径 B 逻辑）
```

- **路径 B 是关键修复**（2026-07-22）：初版迁移代码把 `if v >= 46 { return }` 挡在前面，v46 的 DB 永远到不了 v47 迁移段。改为 `v >= 47` + v46 快速路径。
- 全新库 db.sql 已含 `deleted_at` 列，直接设 `PRAGMA user_version = 47`。

### 2.3 ClipboardItem struct（`crates/clipboard/src/model.rs`）

```rust
/// 软删时间戳（v47）。None=活跃；Some=已进回收站。图片始终 None（不软删）。
#[serde(skip_serializing_if = "Option::is_none")]
pub deleted_at: Option<String>,
```

---

## 3. 删除语义矩阵

| item_type | 单条删除 | 批量删除 | 清空 | 自动清理 | 回收站还原 | 永久删 |
|---|---|---|---|---|---|---|
| **text/voice/ocr/file** | 软删 | 软删 | 软删 | 软删 | 清除 deleted_at | 物理 DELETE |
| **image** | 物理删 | 物理删 | 物理删 | 物理删 | N/A | 物理删 |

**清空语义细节**（`clear_history` / `clear_history_by_filter`）：
- `keep_favorite=true` 时跳过收藏项。
- 先物理删图片（`DELETE ... WHERE item_type='image'` + cleanup_unreferenced_images），再软删文本。
- `clear_history_by_filter(filter="trash")` 走特殊分支 = 永久删回收站（物理 DELETE）。

---

## 4. 命令清单

### 新增 5 个命令

| 命令 | 签名 | 语义 |
|---|---|---|
| `restore_clipboard_item` | `(id, app) -> ()` | 还原单条 |
| `restore_clipboard_items` | `(ids, app) -> usize` | 批量还原 |
| `permanent_delete_clipboard_item` | `(id, app) -> ()` | 永久删单条 |
| `permanent_delete_clipboard_items` | `(ids, app) -> usize` | 批量永久删 |
| `empty_clipboard_trash` | `(app) -> usize` | 清空回收站 |

### 既有命令语义变更（签名不变，前端调用零改动）

| 命令 | 旧语义 | 新语义 |
|---|---|---|
| `delete_clipboard_item` | 物理 DELETE | 图片物理删 / 其他软删 |
| `delete_clipboard_items` | 物理 DELETE | 同上批量 |
| `clear_clipboard_history` | 物理 DELETE | 图片物理删 / 文本软删 |
| `clear_clipboard_history_by_filter` | 物理 DELETE | 同上；filter="trash" 时 = 永久删 |

---

## 5. 前端交互（ClipboardPanel.tsx）

- **回收站 tab**：FILTER_GROUPS 状态组末尾新增 `{ value: "trash" }`
- **行内操作**：回收站模式下每行右侧显示还原(RotateCcw) + 永久删除(Trash2) 按钮
- **顶部操作**：回收站模式 header 显示「全部清空」（destructive-ghost → empty_clipboard_trash）
- **空状态**：回收站为空显示「回收站为空」
- **i18n**：8 个新 key（filterTrash / restore / permanentDelete / emptyTrash / trashEmpty / restored / permanentlyDeleted / trashEmptied）

---

## 6. 回收站自动清理（TTL + 容量双条件，octopus-scheduler）

回收站内容由通用调度 crate `octopus-scheduler` 每 10 分钟检查一次（CPU 空闲时），满足**任一**条件永久删除：

1. **TTL 超期**：`deleted_at` 超过 **3 天**
2. **容量超限**：回收站总条数超过 **500 条**，删最老的（`deleted_at ASC`）超出部分

### 架构

```
crates/infra/src/cpu.rs       ← global_cpu_usage() + is_cpu_idle()（sysinfo 封装）
crates/scheduler/             ← octopus-scheduler：每 10 分钟 tick + CPU 空闲检测
crates/clipboard/src/store.rs ← purge_trash(conn, ttl_days, max_items)
crates/desktop/src/main.rs    ← setup 创建 Scheduler + 注册 trash_purge 任务
```

- **infra/cpu.rs**：`global_cpu_usage() -> f32` + `is_cpu_idle(threshold) -> bool`。内部 `OnceLock<Mutex<System>>` 持久化实例（sysinfo CPU 差分需跨 tick）。SystemStatusSampler 保持自己的 System（它需要 refresh_processes/memory 等更多 API）。
- **octopus-scheduler**：通用调度框架。后台线程每 10 分钟醒一次 → 查 `infra::cpu::is_cpu_idle(30.0)` → CPU < 30% 才执行所有到期任务。不知道业务逻辑——任务由 `register_task(name, interval, run: Box<dyn Fn()>)` 注册。
- **purge_trash**：先删 TTL 超期（`DELETE WHERE deleted_at < datetime('now','-3 days')`），再查剩余条数，超 500 则删最老的（`DELETE WHERE id IN (SELECT id ... ORDER BY deleted_at ASC LIMIT excess)`）。物理 DELETE 触发 FTS trigger 自动清索引。
- **TTL = 3 天 / max_items = 500**：写死常量，不可配。
- **CPU 阈值 = 30%**：Scheduler 默认值，写死。
- 与 clipboard 自有的 cleanup 线程（每小时按 age/count 清活跃项）互补：cleanup 管「太多/太老」，scheduler 管「回收站里待太久」。

---

## 7. 已知限制

- 浮窗不暴露回收站（浮窗删除仍走 delete_clipboard_item 软删，但需去设置页还原）
- FTS 对软删行可见（回收站 tab 走 FTS 搜索能命中软删行——期望行为）
