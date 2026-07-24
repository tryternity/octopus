# 热词同步覆盖 Bug 分析（待定方案）

> **日期**：2026-07-25
> **状态**：📋 待定方案（用户选择先记录，后续自行决策）
> **症状**：新增的热词，进行 git 同步后消失了

---

## 1. 根因链

`sync_now`（`crates/vault/src/sync/engine.rs:588`）的执行顺序：

```
1. git fetch
2. git merge --ff-only（或 rebase 兜底）
3. pull（文件 → DB）    ← bug 在这
4. push（DB → 文件）
5. git add + commit + push
```

**bug 核心**：步骤 3 的 `pull_hotwords_from_files`（`crates/sync/src/hotword.rs:356`）用「merge 后工作区的 outline.md5」对比「DB 的 sync_md5」，只要不匹配就用**文件版本（旧）覆盖 DB（新）**，没有方向判断（不区分「本地改了」还是「远端改了」）。

关键代码 `hotword.rs:366-367`：
```rust
let needs_update = !db_ids.contains(uuid.as_str())
    || hotword_md5_mismatch_v2(uuid, &entry.md5, &db_sets);
// mismatch = true → 读文件 → upsert_hotword_set 全字段覆盖 DB
```

`upsert_hotword_set_at`（`db.rs:3058-3080`）是 `ON CONFLICT(id) DO UPDATE SET words_text=excluded.words_text, ...`——**全字段覆盖，无合并**。

## 2. 触发场景（单设备即可触发，无需多设备）

**用户实测场景**：单设备备份用，在「通用」集里加词，同步后该词消失。

单设备时序：
1. 上次 sync：「通用」集（含词A）已 commit + push，远端 outline 的「通用」md5 = md5(A)
2. 本地加词B → DB「通用」sync_md5 = md5(A+B)（`refill_sync_md5` 回填）
3. 触发 sync_now（手动或定时）：
   - merge：远端无新 commit → "Already up to date" → 工作区文件不变（还是只有A的版本）
   - **pull**：读 outline（md5(A)）对比 DB（md5(A+B)）→ **mismatch=true** → 读文件「通用」（只有A）→ **upsert 覆盖 DB** → **词B丢失**
   - push：DB 已被覆盖成只有A → 写只有A的文件 → commit + push → 词B永久消失

## 3. 为什么 vault 没暴露

vault 的 `pull_from_files`（`engine.rs:785-803`）cipher/folder 用**完全相同的 md5-mismatch-then-overwrite 逻辑**（`cipher_md5_mismatch` `engine.rs:890`）。理论上 vault 也有此 bug，但未暴露，推测原因：
- vault 修改频率低（密码不常改），改后通常立即手动 sync 验证
- 立即手动 sync 时，远端是本地刚 push 的状态，pull 时 md5 匹配（刚 push 的），不触发覆盖
- 热词有**每小时定时 sync**（`main.rs:614`），用户加词后 1 小时内若没手动 sync，定时触发时远端仍是旧的 → 覆盖

**本质**：任何「本地改了 → 间隔一段时间 → sync（此时远端是旧版本）」都会触发，与设备数无关。

## 4. 加剧因素：「通用」集固定 UUID

`db.rs:716-717`：v45→v46 迁移时，所有「通用」热词集分配固定 UUID `00000000-0000-0000-0000-000000000001`。

- 多设备：两台机器的「通用」集同 UUID → 必然冲突 → last-write-wins
- 单设备：同 UUID 不是直接原因（单设备无 UUID 冲突），但「通用」集是默认集、高频修改对象，更容易暴露 pull 覆盖 bug

新建热词集用 `Uuid::new_v4()`（随机），单设备场景下不冲突，但**仍受 pull 覆盖 bug 影响**（只要本地改了没及时 push 到远端，pull 就会覆盖）。

## 5. 候选修复方案

### 方案 A：pull 跳过覆盖（最小修复）

pull 时，对 DB 已有条目，只有 `sync_md5 == outline.md5`（本地未改）才允许 pull 覆盖；不匹配时跳过（保留本地），让 push 阶段导出本地版本。

- ✅ 单设备完美修复（本地新数据不被旧版本覆盖）
- ✅ 改动小（pull 函数加一个条件判断）
- ❌ 多设备：双方都改时，后 push 者覆盖先 push 者（仍 last-write-wins，但至少不丢本地新数据）
- ❌ 多设备：远端真有更新但本地也改了时，远端更新拉不进来（需二次 sync 或手动处理）

### 方案 B：热词合并并集（热词专用，推荐）

pull 时检测同名热词集双方都改了 → `words_text` 合并为并集（normalize 去重）；`name`/`enabled` 冲突取 `updated_at` 较新者。

- ✅ 语义安全（用户的词只增不减，符合热词「集合」本质）
- ✅ 多设备也不丢数据
- ❌ 实现复杂（需读双方 words_text 解析、并集、重新 normalize、算 md5）
- ❌ 只适用于热词，vault cipher 不能用此方案（密码条目不能并集）

### 方案 C：分步（先 A 后 B）

先上方案 A 止血（快速修复单设备数据丢失），后续再实现方案 B 优化多设备体验。

## 6. 待决策点

- [ ] 选择哪个方案（A / B / C）
- [ ] vault cipher/folder 的同类 bug 是否一并修（vault 用方案 A 较安全，密码不能并集）
- [ ] 「通用」集固定 UUID 是否改为随机（多设备隔离），或保留固定但改用合并策略

## 7. 相关代码位置速查

| 位置 | 作用 |
|---|---|
| `crates/vault/src/sync/engine.rs:588` | `sync_now` 主流程（merge→pull→push 顺序） |
| `crates/vault/src/sync/engine.rs:642` | 热词 pull 调用点（失败只 warn 不阻断） |
| `crates/sync/src/hotword.rs:356-398` | `pull_hotwords_from_files`（bug 主体，mismatch→覆盖） |
| `crates/sync/src/hotword.rs:402-407` | `hotword_md5_mismatch_v2`（只比 md5，无方向） |
| `crates/sync/src/hotword.rs:237-304` | `incremental_export_hotwords`（push 阶段，DB→文件） |
| `crates/infra/src/db.rs:3058-3080` | `upsert_hotword_set_at`（全字段覆盖，无合并） |
| `crates/vault/src/sync/engine.rs:785-803` | vault cipher pull（同样 bug，未暴露） |
| `crates/vault/src/sync/engine.rs:890-894` | `cipher_md5_mismatch`（与热词同逻辑） |
| `crates/infra/src/db.rs:716-717` | 「通用」集固定 UUID（v46 迁移） |
| `crates/desktop/src/hotword_commands.rs:22-32` | `refill_sync_md5`（写后回填 md5） |
