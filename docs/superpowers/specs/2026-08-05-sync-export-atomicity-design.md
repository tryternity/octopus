# Sync Export 原子性设计——消除 remove_dir_all 残破窗口 + merge 冗余写

- 日期：2026-08-05
- 类型：架构改进（sync 层数据流重构）
- 触发：第二十三轮全代码审查 P2-sync1/sync2（= 第 21 轮 P2-s1/s2），衔接近期 `SyncEntity` trait 统一重构（[spec](./2026-08-05-sync-entity-trait-unification-design.md)）
- 状态：**✅ 已实施（2026-08-10）**——见 [实施 plan](../plans/2026-08-10-sync-export-atomicity.md)。方案 B（先写后清孤儿）+ 方案 C（删冗余 push 写）落地，覆盖 clipboard + hotword + vault。回归测试 5 个。

---

## 1. 背景与动机

`SyncEntity` trait 统一后，3 模块（vault cipher/folder + hotword set/word + clipboard favorite）的 merge 主循环同构（[trait spec §1](./2026-08-05-sync-entity-trait-unification-design.md)）。但 `export_all`（trait 方法，merge 末尾调）仍有两个遗留问题。

### 1.1 P2-sync1 · export_all 非原子「清空 + 重建」

**现状**：`export_all` 流程为 `remove_dir_all(实体目录) → create_dir_all → 逐文件 write_atomically → write_outline`。

- `clipboard::export_all_favorites`（`sync/src/clipboard.rs:436-443`）：`remove_dir_all(favorites/)` → `create_dir_all(favorites/)` → 逐 favorite 写
- `hotword::export_all_hotwords_with`（`sync/src/hotword.rs:426-437`）：`for entry in read_dir(hotword/) { remove_dir_all(各 set 目录) }` → 逐 set 重建

**问题**：`remove_dir_all` 成功后、新文件写完前崩溃（断电 / SIGKILL / panic 未被 catch）→ 目录被删空但新内容未写入 → **残破工作区**。git 会捕获到删除（`.sync` 是 git repo），commit/push 后传播到对端 → 对端也丢失文件。

**风险窗口**：实测 `remove_dir_all` 到 `create_dir_all` 之间是**微秒级**（两个 syscall）。触发需恰在此窗口崩溃。概率低但非零，且后果是**数据丢失**（不可恢复——文件已删，DB 重启后 export 会重建，但对端已 pull 残破状态）。

### 1.2 P2-sync2 · merge push 写被 export_all 全量覆盖（冗余 IO）

**现状**：`merge_three_way`（`sync/src/pipeline.rs:141-211`）流程：
1. 阶段 1：outline 驱动逐 key 判定（pull / push / conflict / skip）
2. 阶段 2：DB-only 行（outline 无）→ push
3. 阶段 3（:209）：`E::export_all()?` 全量重建所有文件 + outline

**问题**：push（阶段 1/2）调 `write_file` 写单文件（`pipeline.rs:189/205/240-251`），但阶段 3 `export_all` 会 `remove_dir_all + 全量重写`——**push 写的文件被 export_all 完全覆盖**。1000 收藏 = 1000 次无效原子写。

**性质**：纯性能问题（冗余 IO），**不影响正确性**——export_all 保证最终一致（DB 是真相源，全量重建覆盖一切中间状态）。但每次 merge 多 1000 次磁盘写 + fsync。

---

## 2. 方案对比

### 方案 A · tmp 目录 + POSIX rename 原子替换

**思路**：写全量到 `实体目录.new/` → rename 原子替换 `实体目录/`。

```
1. mkdir favorites.new/
2. 逐 favorite write_atomically 到 favorites.new/<2hex>/<uuid>.json
3. rename(favorites, favorites.old)        // POSIX 原子
4. rename(favorites.new, favorites)        // POSIX 原子（覆盖）
5. remove_dir_all(favorites.old)
6. write_atomically(outline.json)          // 已原子
```

**崩溃恢复**：下次 export 启动时检测残留：
- `favorites.new` 存在 → 上次 step 1-2 未完成 → 删 `favorites.new` 重做
- `favorites.old` 存在 → 上次 step 3-4 成功但 step 5 未完成 → 删 `favorites.old`（favorites 已是新内容）

**优点**：目录替换语义清晰；崩溃恢复路径明确。

**缺点**：
- **hotword 双层目录复杂**：hotword 是 `hotword/<set-id>/{meta.json, outline.json, <2hex>/<word>.json}`，每个 set 独立子目录。需对每个 set 子目录做 tmp+rename，或对整个 `hotword/` 做（但 `hotword/` 还含 `outline.json` 总索引）。
- **Windows 兼容**：POSIX `rename` 原子覆盖目标目录，但 Windows `MoveFileEx` 对已存在目录失败。octopus 当前 macOS only，但代码不应假设。
- **磁盘空间翻倍**：写 tmp 期间新旧目录并存，大 vault（数千 cipher）临时占双倍空间。
- **rename 目录在跨挂载点失效**：若 `.sync` 和 tmp 在不同卷（理论可能，如 `.sync` 是符号链接到他处），rename 失败。需保证 tmp 与目标同目录（`.sync/clipboard/` 内）。

### 方案 B · 先写后清孤儿（零窗口）

**思路**：不 remove_dir_all，改为「先全量写新文件 → 扫目录删非 DB 孤儿文件」。

```
1. 收集 DB 全部 keys（active + tombstone）→ keep_set
2. 逐 DB 行 write_atomically 到 对应路径（覆盖旧文件）
3. write_atomically(outline.json)
4. 遍历 实体目录/ 所有 .json 文件 → 不在 keep_set 的删除（孤儿清理）
```

**优点**：
- **零残破窗口**：任何时刻崩溃，目录里要么是旧文件（写未开始）、要么新文件已写入（旧文件可能残留，下次 export 清）。**永不存在「目录被删空」状态**。
- **无 tmp 目录 + rename 复杂度**：纯文件级操作，复用 `write_atomically`（已原子）。
- **无磁盘翻倍**：不并存两份。

**缺点**：
- **孤儿遍历成本**：需扫整个分片子目录树（`<2hex>/<uuid>.json`）。clipboard 分片 256 桶，hotword 每 set 分片 256 桶 × N sets。但 export 本就是全量操作（O(N) 写），多一次 O(N) 扫描不改变量级。
- **删除孤儿非原子**：step 4 删孤儿文件逐个删，中途崩溃留下部分孤儿——但孤儿不影响正确性（下次 export 会清），且 merge pull 按 outline 走（outline 只含 DB keys，孤儿不在 outline 不会被 pull）。
- **实现需遍历分片结构**：clipboard 单层分片，hotword 双层（set 目录 + set 内分片）。

### 方案 C · 保留现状 + 删冗余 push 写（仅修 P2-sync2）

**思路**：不动 export_all（P2-sync1 留后续），只删 merge 阶段 1/2 的 `write_file` 调用——因为 export_all 会覆盖。

```rust
// pipeline.rs push_or_skip：不调 write_file，只记 report
fn push_or_skip<E: SyncEntity>(row: &E::Row, key: &str, report: &mut MergeReport) -> bool {
    // 第二十三轮 P2-sync2：write_file 被 export_all 覆盖，省略避免冗余 IO
    report.pushed += 1;
    true
}
```

**优点**：极简，3 行改动，消除 1000 收藏 = 1000 次无效写。

**缺点**：
- **不修 P2-sync1**（残破窗口仍在）。
- **语义损失**：push 的 `write_file` 是「DB→文件」语义的单点体现。删掉后，文件状态**完全**依赖 export_all。若未来有人改 export_all 为增量（不全量重建），push 语义就丢了——需在 export_all 加不变量注释「必须全量重建，push 依赖此」。
- **部分失败容错降低**：当前若 export_all 中途失败，push 已写的文件存活（至少 push 的那些一致）。删 push 后，export_all 中途失败 → 部分文件新部分旧（但 outline 是最后写的 write_atomically，outline 仍旧 → 下次 merge 重算，自愈）。

---

## 3. 推荐方案

**推荐方案 B（先写后清孤儿）**，理由：

1. **零残破窗口**——P2-sync1 是数据丢失风险，方案 B 彻底消除。方案 A 有 step 3-4 间窗口（虽 POSIX 原子，但两步 rename 之间崩溃仍有理论残留状态需恢复检测）。
2. **复杂度可控**——方案 B 的孤儿清理复用 outline（export 后 outline 含全部 active keys，扫目录文件不在 outline 即孤儿）。分片遍历是 O(N)，与 export 的 O(N) 写同量级。
3. **无双倍磁盘**——方案 A 在 tmp 写期间新旧并存，大 vault 临时占双倍。
4. **跨平台安全**——无 rename 目录的 Windows/跨卷问题。

**P2-sync2 的处理**：采用方案 C 的思路（删冗余 push 写），但**作为方案 B 的一部分**——方案 B 的 step 2「逐 DB 行 write_atomically」已经等价于 export_all 的全量重建，merge 阶段 1/2 的 push 写完全冗余，一并删除。

即：**方案 B + C 合并** = export_all 改「先写后清孤儿」+ merge pipeline 删 push 的 write_file。

---

## 4. 实施设计（方案 B + C）

### 4.1 trait 新增方法：`cleanup_orphan_files`

`SyncEntity` trait 新增一个方法，供 export_all 末尾调用清理孤儿：

```rust
pub trait SyncEntity {
    // ... 现有方法 ...

    /// 清理 `.sync` 目录中不在 `keep_keys` 集合的孤儿文件。
    ///
    /// 方案 B 的 step 4：export 写完所有 DB 行 + outline 后，扫实体目录删除
    /// 不在 keep_keys 的 .json 文件（DB 已删但文件残留的孤儿）。
    ///
    /// keep_keys 含 active + 未超期 tombstone 的 key（与 export 写入范围一致）。
    /// 实现者负责各自的目录结构遍历（clipboard 单层分片 / hotword 双层 set+分片）。
    fn cleanup_orphan_files(keep_keys: &std::collections::HashSet<String>) -> Result<()>;
}
```

### 4.2 clipboard 实现

`clipboard::export_all_favorites` 改造：

```rust
pub fn export_all_favorites() -> Result<ClipboardOutline> {
    let key = load_or_create_clipboard_key()?;
    let favs = octopus_infra::db::list_all_favorites()?;
    let clip_dir = clipboard_dir()?;
    std::fs::create_dir_all(&clip_dir)?;  // 不 remove_dir_all

    let mut entries: BTreeMap<String, OutlineEntry> = BTreeMap::new();
    let mut keep_keys: HashSet<String> = HashSet::new();

    for fav in &favs {
        // ... 现有 write_favorite_file + entries.insert 逻辑不变 ...
        keep_keys.insert(fav.history_id.clone());
    }

    // 清孤儿：扫 favorites/<2hex>/*.json，删不在 keep_keys 的
    cleanup_orphan_favorite_files(&keep_keys)?;

    let outline = ClipboardOutline { version: 1, favorites: entries };
    write_clipboard_outline(&outline)?;
    Ok(outline)
}

fn cleanup_orphan_favorite_files(keep_keys: &HashSet<String>) -> Result<()> {
    let fav_dir = favorites_dir()?;
    if !fav_dir.is_dir() { return Ok(()); }
    // 遍历 <2hex> 分片子目录
    for shard_entry in std::fs::read_dir(&fav_dir)? {
        let shard_path = shard_entry?.path();
        if !shard_path.is_dir() { continue; }
        for file_entry in std::fs::read_dir(&shard_path)? {
            let file_path = file_entry?.path();
            // 提取 uuid（文件名去 .json）——不在 keep_keys 即孤儿
            if let Some(stem) = file_path.file_stem().and_then(|s| s.to_str()) {
                if !keep_keys.contains(stem) {
                    let _ = std::fs::remove_file(&file_path);  // 孤儿删除失败不阻断
                }
            }
        }
    }
    Ok(())
}
```

### 4.3 hotword 实现

hotword 双层结构（set 目录 + set 内分片）需两级清理：

```rust
pub fn export_all_hotwords_with(sets: &[HotwordSet], words: &[HotwordWord]) -> Result<HotwordOutline> {
    let dir = hotword_dir();
    std::fs::create_dir_all(&dir)?;  // 不 remove_dir_all 各 set 目录

    // ... 现有逐 set 写 meta + word + set outline 逻辑不变 ...

    // 清孤儿：set 级（不在 keep_set_ids 的 set 目录）+ word 级（set 内不在 keep_word_ids 的词文件）
    let keep_set_ids: HashSet<String> = sets.iter()
        .filter(|s| !is_tombstone_expired(s.is_deleted, now_secs))
        .map(|s| s.id.clone()).collect();
    cleanup_orphan_hotword_files(&keep_set_ids, &words)?;

    write_hotword_outline(&outline)?;
    Ok(outline)
}

fn cleanup_orphan_hotword_files(keep_set_ids: &HashSet<String>, words: &[HotwordWord]) -> Result<()> {
    let dir = hotword_dir();
    if !dir.is_dir() { return Ok(()); }
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if !path.is_dir() { continue; }
        let set_id = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if !keep_set_ids.contains(&set_id) {
            // 整个 set 目录是孤儿（DB 已删或超期 tombstone）——删目录
            let _ = std::fs::remove_dir_all(&path);
        } else {
            // set 存活——清 set 内孤儿词文件
            let keep_word_ids: HashSet<String> = words.iter()
                .filter(|w| w.set_id == set_id && !is_tombstone_expired(w.is_deleted, now_secs))
                .map(|w| w.id.clone()).collect();
            // 遍历 set_dir/<2hex>/*.json
            for shard_entry in std::fs::read_dir(&path)? {
                let shard_path = shard_entry?.path();
                if shard_path.is_dir() {
                    for file_entry in std::fs::read_dir(&shard_path)? {
                        let file_path = file_entry?.path();
                        if let Some(stem) = file_path.file_stem().and_then(|s| s.to_str()) {
                            if !keep_word_ids.contains(stem) {
                                let _ = std::fs::remove_file(&file_path);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
```

### 4.4 P2-sync2 · merge pipeline 删冗余 push 写

`pipeline.rs` 的 `push_or_skip` 不再调 `write_file`：

```rust
fn push_or_skip<E: SyncEntity>(_row: &E::Row, _key: &str, report: &mut MergeReport) -> bool {
    // 第二十三轮 P2-sync2：write_file 被 export_all 全量覆盖（方案 B 的 step 2 已写
    // 所有 DB 行）。push 只记 report 计数（pulled/pushed/conflicts/skipped），实际
    // 文件写入统一由 export_all 在 merge 末尾完成。1000 收藏省 1000 次无效写。
    report.pushed += 1;
    true
}
```

**不变量**（必须注释强调）：`export_all` **必须全量重建**（写所有 DB 行 + 清孤儿）。若未来改为增量，需恢复 push 的 write_file。

---

## 5. 影响面

### 5.1 改动文件

| 文件 | 改动 |
|---|---|
| `sync/src/pipeline.rs` | `SyncEntity` trait 加 `cleanup_orphan_files` 方法；`push_or_skip` 删 write_file |
| `sync/src/clipboard.rs` | `export_all_favorites` 去 remove_dir_all + 加 cleanup_orphan；新增 `cleanup_orphan_favorite_files` |
| `sync/src/hotword.rs` | `export_all_hotwords_with` 去 remove_dir_all + 加 cleanup_orphan；新增 `cleanup_orphan_hotword_files` |
| `vault/src/sync/store.rs` | **vault 同型**（已核实 :529/:532 `export_all_to_files` 也有 remove_dir_all ciphers/folders）。但 vault 未迁移到 `SyncEntity` trait（仍用独立 `merge_vault` + `export_all_to_files`），方案 B 的 trait 方法对 vault 不直接生效。vault 的修复需单独改 `export_all_to_files`（同型改法：去 remove_dir_all + 加孤儿清理），或等 vault 迁移到 trait 后统一受益。 |

**vault 处理决策**：建议 vault 单独改 `export_all_to_files`（非 trait 路径），因为：
1. vault 迁移到 trait 是另一个大重构（[trait spec](./2026-08-05-sync-entity-trait-unification-design.md) 阶段性，vault 未完成）
2. P2-sync1 对 vault 同样是数据丢失风险
3. 改法同型（先写后清孤儿），不依赖 trait

### 5.2 不变量

1. **export_all 全量重建不变量**：必须写所有 DB 行（active + 未超期 tombstone）+ 清孤儿。删 push 写后，文件状态完全依赖此。
2. **outline 最后写**：outline.json 用 write_atomically（原子），在 cleanup 之后写。任何时候崩溃，outline 要么旧（自愈）、要么新（与文件一致）。
3. **孤儿不影响 merge pull**：pull 按 outline 走（outline 只含 DB keys），孤儿文件不在 outline 不会被 pull。

### 5.3 测试

- **P2-sync1 回归**：export 后，手动在目录塞一个孤儿 .json → 再 export → 断言孤儿被清。
- **P2-sync1 残破窗口**（难直接测崩溃）：验证 export 后目录非空（`favorites/` 存在且含文件）。
- **P2-sync2 回归**：merge 阶段 push 后立即查文件 mtime——验证文件未被 push 单独写（只被 export_all 写一次）。或验证 push_or_skip 不调 write_file（trait mock 注入计数）。
- **现有 merge 测试全过**：vault/hotword/clipboard 的 merge 集成测试不变。

---

## 6. 风险与取舍

1. **vault 已核实同型**（2026-08-05）：`vault/src/sync/store.rs:516-532` 的 `export_all_to_files` 也有 `remove_dir_all(ciphers_dir)` + `remove_dir_all(folders_dir)`。vault 也有 P2-sync1 问题。但 vault 未迁移到 `SyncEntity` trait（仍用 `merge_vault` + 独立 export），方案 B 的 trait 方法不直接覆盖 vault——需单独改 `export_all_to_files`（§5.1 vault 处理决策）。
2. **孤儿清理的 now_secs 一致性**：cleanup 用 `is_tombstone_expired` 判定时需用与 export 相同的 now。若 export 和 cleanup 之间时间跨越 GC 边界（极端），可能误判。应传同一 now。
3. **方案 B 的 step 4 删孤儿中途崩溃**：留部分孤儿——但不影响正确性（下次 export 清），可接受。
4. **大 vault 的孤儿遍历性能**：数千文件的 `read_dir` 递归是 O(N)，与 export 写 O(N) 同量级，不改变整体复杂度。

---

## 7. 后续

本 spec 是**设计阶段**，实施需：
1. 核实 vault export 是否同型（§6.1）
2. 写实施 plan（`docs/superpowers/plans/2026-08-05-sync-export-atomicity.md`）
3. TDD：先写孤儿清理 + 残破窗口回归测试
4. 实施 + 全量测试（sync 153 测试 + desktop 525 测试全过）
