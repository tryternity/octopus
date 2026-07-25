# 热词同步覆盖 Bug 分析（待定方案）

> **日期**：2026-07-25
> **状态**：✅ 已实现（方案：merge UpToDate 时跳过 pull；用户确认 last-write-wins 可接受）
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

## 5. 修复方案（✅ 已实现）

### 选定方案：merge UpToDate 时跳过 pull

经分析，最精准的修复点不在 `pull_hotwords_from_files` 函数本身（pull 在 FastForwarded 路径仍需要正常拉取远端更新），而在 `sync_now` 的流程控制：**当 git merge 表明远端无新 commit（UpToDate）时，跳过 pull 阶段**。

理由：UpToDate 意味着工作区文件是上次 sync 的旧状态，此时本地 DB 若有新改动（加词/删词），pull 必然检测到 md5 mismatch 并用旧文件覆盖 → 数据丢失。跳过 pull 后，push 阶段正常导出本地新数据到文件 → commit + push → 远端拿到最新。

**实现**（`crates/sync/src/git.rs` + `crates/vault/src/sync/engine.rs`）：
1. `MergeFfResult` enum 新增 `UpToDate` 变体；`git_merge_ff` 通过对比 merge 前后 HEAD SHA 区分 `UpToDate`（hash 不变）vs `FastForwarded`（hash 变了）——`git merge --ff-only` 在 "Already up to date" 时也 exit 0，无法靠退出码区分。
2. `sync_now` 在 `UpToDate` / `NoUpstream`（首次推送）时跳过 vault pull + 热词 pull；`FastForwarded` / rebase 正常 pull（远端有更新）。
3. push 阶段不受影响（DB→文件，导出本地最新）。

**覆盖场景**：
- ✅ 单设备加词 → sync → 词还在（UpToDate 跳过 pull，push 导出新词）
- ✅ 单设备删词（改 words_text）→ sync → 删除保留（同上）
- ✅ 多设备 A push → B sync → B 拿到 A 的更新（FastForwarded 正常 pull）
- ⚠️ 多设备同时改：last-write-wins（用户已确认可接受）

### 未采纳方案（备选）

- **方案 A（pull 函数加方向判断）**：pull 时对 DB 已有条目只 `sync_md5 == outline.md5` 才覆盖。更细粒度，但实现复杂（需「上次同步 md5 快照」），且 FastForwarded 路径仍需覆盖。当前方案在流程层解决更简洁。
- **方案 B（words_text 并集合并）**：词级合并。用户确认 last-write-wins 可接受，不需要。

## 6. 已知限制（用户接受，留作后续）

- [ ] **删整个集的复活**：`delete_hotword_set` 是硬删（`DELETE FROM`，无 `deleted_at` 列）。A 删集 → push 删文件 → B pull 不删 DB → B push 又写回 → A pull 复活。需引入 `deleted_at` tombstone（仿 vault_ciphers）才能正确传播。用户主要删「集里的词」（改 words_text，已修复），删整个集低频，留作后续。
- [ ] **多设备同时改 last-write-wins 丢失**：用户接受，不做冲突合并。
- [ ] **vault cipher/folder 同类 pull 覆盖**：vault pull 用相同 md5-mismatch-overwrite 逻辑，本次修复（UpToDate 跳过 pull）对 vault 同样生效，vault 也受益。

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
