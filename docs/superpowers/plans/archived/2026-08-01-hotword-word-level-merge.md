# 热词 word 级 sync merge 实施计划

> **日期**：2026-08-01
> **spec**：`docs/superpowers/specs/2026-08-01-hotword-word-record-design.md` §3
> **分支**：`feature/hotword-word-merge`（worktree `.worktrees/hotword-word-merge`，从 main）
> **状态**：🔄 实施中
> **目标**：两设备各自加的词跨设备合并（word 级 3-way merge，对称 vault cipher merge）

## 背景与现状

热词存储 v57 已从 `hotword_sets.words_text` 拆成 `hotword_words` 表（每词一条）。set 级 sync（name/enabled 元数据）已完成，但**词数据本身不跨设备合并**——两设备往同一词典加词，sync 后词不合并。

### 已有可复用基础设施（直接复用，不重写）
- **`hotword_words` 表 + CRUD**：`crates/infra/src/db/hotword.rs` 的 `HotwordWord` struct / `list_all_hotword_words` / `upsert_hotword_word` / `update_hotword_word_sync_md5` 全 `pub`
- **确定性 UUID**：`octopus_infra::hotword_text::hotword_word_uuid(set_id, word)`（v5 SHA1，跨设备一致）+ `word_plain_pinyins(word)`
- **sync 通用工具**：`store::shard_dir` / `md5_hex` / `iso_to_unix_ms` / `sync_root`；`OutlineEntry`（`md5` + `updated_ms`）复用
- **vault merge 范本**：`crates/vault/src/sync/engine.rs::merge_vault`（1027-1307 行）cipher/folder 3-way merge
- **vault 原子写 + path traversal 范本**：`crates/vault/src/sync/store.rs`（`validate_uuid` + `write_atomically` + `*_file_path` 三件套）—— `write_atomically` 当前 vault private，需在 sync store 加一份

---

## 目录结构（用户确认——两级 outline 层级）

```
~/.octopus/.sync/hotword/
├── outline.json              ← 总 outline：只描述词典状态
│     { version, hotwordVersion, sets: { <setUuid>: {md5, updatedMs} } }
└── <set-uuid>/               ← 每个词典一个目录（目录名 = 词典 ID，validate 校验）
    ├── meta.json             ← 词典元数据（name/enabled/createdAt/updatedAt）
    ├── outline.json          ← 本词典的词状态
    │     { words: { <wordUuid>: {md5, updatedMs} } }
    └── <2hex>/<word-uuid>.json   ← 词文件（按词 UUID 前2位分桶；UUID 不含 is_deleted → 软删不改文件名）
```

**为什么层级优于扁平**：① 3 万词条拆成 10 个 3 千项的词典 outline，git diff 只碰改动词典；② 删词典 = `rm -r <set-id>/` 原子完整；③ 语义干净——总 outline 管词典，词状态归属各自词典。

**词文件名用 UUID**（=v5(set_id,word)，软删/改拼音不变），内容 MD5 做 outline 变化指纹（对齐 vault cipher 文件名=cipher-uuid）。

---

## 关键设计决策（锁定）

- **D1 两级 outline**：`HotwordOutline{version, hotword_version, sets}`（总，只管词典）+ `HotwordSetOutline{version, hotword_version, words}`（词典内，只管词）。`OutlineEntry`（md5+updatedMs）复用。vault `Outline` 不动。
- **D2 meta.json**：词典元数据从扁平 `sets/<2hex>/<uuid>.json` 改成 `<set-uuid>/meta.json`（语义清晰——meta 是词典属性）。
- **D3 word md5**：`{set_id_len}|{set_id}|{word_len}|{word}|{pinyin_len}|{pinyin}|{is_deleted}`（长度前缀防 `|` 碰撞，不含时间戳）。
- **D4 sync_md5 填充**：DB 层写入时填（对齐 vault `cipher.rs:82`）。desktop `refill_sync_md5` 临时方案**移除**——set sync_md5 回归纯元数据 `name|enabled`，word 有自己的 sync_md5。
- **D5 path traversal + 原子写**：`validate_hotword_uuid`（拒绝 `/` `\` `..` `\0` 空）覆盖 set 目录名 + word 文件名；`write_atomically` 加到 `crates/sync/src/store.rs`（搬 vault 实现）。
- **D6 infra 加 md-5**：infra 加 `md-5 = "0.10"` 外部依赖（不违反「无项目内依赖」规则），`hotword_word_md5_from_fields` 放 `hotword_text.rs`（纯函数），sync + db 都调。
- **D7 API**：`export_all_hotwords()`/`incremental_export_hotwords()`/`merge_hotwords()` 都无参，内部自己 `list_hotword_sets()` + `list_all_hotword_words()`（对齐 `merge_hotwords()` 已无参 + `push_hotwords_to_files()` 无参）。enable_sync 调用点同步去掉参数。

---

## 任务分解（TDD：先写失败测试再实现）

### Task 1：两级 outline 结构 + 读写（`crates/sync/src/hotword.rs`）
- `HotwordOutline`（总）+ `HotwordSetOutline`（词典内），删 `use Outline`，保留 `use OutlineEntry`。
- `read/write_hotword_outline`（总）+ `read/write_hotword_set_outline(set_id)`（词典内）。
- `export_all_hotwords`/`incremental_export_hotwords`/`merge_hotwords`（set 级部分）全切新结构，`outline.ciphers`→`outline.sets`。
- 现有测试 `outline.ciphers`→`outline.sets` 全量改。

### Task 2：vault engine 测试适配（让 workspace 能编译）
3 处 `outline.ciphers`→`outline.sets`；顺带修 `set_hotword_set_words`（v57 已移除，main 上 vault test target 已 broken）→ `add_words_to_set`。

### Task 3：路径 + 原子写 + meta.json 迁移（`store.rs` + `hotword.rs`）
- `store::write_atomically(path, content)`（pub(crate)，含 fsync+rename+目录 fsync）。
- `validate_hotword_uuid`；`hotword_set_dir(set_id)`/`hotword_meta_file_path(set_id)`/`hotword_set_outline_path(set_id)`/`hotword_word_file_path(set_id, word_uuid)`。
- `HotwordSetFile`→`HotwordSetMeta`，路径 `sets/<2hex>/<uuid>.json`→`<set-uuid>/meta.json`，set 文件读写改原子写。
- TDD：path traversal 拒绝 / meta 往返 / set 目录创建。

### Task 4：`HotwordWordFile` + md5 + DB sync_md5 填充
- infra `hotword_text.rs`：加 `hotword_word_md5_from_fields`（infra 加 md-5 依赖）。
- sync `hotword.rs`：`HotwordWordFile`(camelCase) + from/to + `hotword_word_md5(&HotwordWord)`；`read/write/delete_hotword_word_file`（原子写）。
- infra `db/hotword.rs`：`add_word_to_set_at`/`remove_word_from_set_at`/`set_words_in_set_at` 写入时填 word sync_md5。
- TDD：md5 确定性/防 `|` 碰撞/软删 md5 变/文件往返。

### Task 5：export 加 word 部分 + 词典内 outline 重建
- `export_all_hotwords()`：遍历词典，每词典写 meta.json + 词文件 + 本词典 outline.json；清空重建目录（防 stale）。
- `incremental_export_hotwords()`：分词典 diff，只写变化的词文件 + 词典 outline 增量。
- TDD：export 写词文件 / 增量 0 变更 / 软删词传播（is_deleted=true 文件原地重写）/ 删词典删目录。

### Task 6：`merge_hotword_words` 核心（word 级 3-way merge）
- 在 `merge_hotwords` set merge 后，对每个词典做 word 3-way merge（读词典 outline + DB 该词典的 words），对称 vault cipher merge。
- 判定：outline 有+DB无→pull；都有→比 updated_ms（remote>local pull / local>remote push / 相等 md5 比对 DB 赢）；DB有+outline无→push。软删通过 is_deleted 文件原地传播。
- `HotwordMergeReport` 复用（pulled/pushed/conflicts/skipped 是 set+word 合计）。
- TDD 4+1 场景（新增 `write_remote_word(set_id, word, is_deleted, updated_ms)` 辅助）：pull 远程新词 / 本地新词不被覆盖 / DB-only push / 软删跨设备传播 / 时间戳相等冲突 DB 赢。

### Task 7：desktop `refill_sync_md5` 简化 + 文档同步 + enable_sync 调用点去参
- `refill_sync_md5` 回归纯元数据 `hotword_set_md5(&h)`（移除词指纹组合）。
- enable_sync / push_initial 调用点 `export_all_hotwords(&sets)`→`export_all_hotwords()`。
- spec §3 状态「word 级 merge 待做」→「已实现」；spec §3 文件布局改两级；architecture.md 更新。

---

## 验证（全过才算完成）

```bash
cargo test -p octopus-sync --lib      # 新 word 级 merge + 两级 outline 测试
cargo test -p octopus-infra --lib     # word sync_md5 填充
cargo build -p octopus-vault --tests  # 修好的集成测试
cargo build --workspace               # 0 error
cargo build -p octopus-desktop        # refill + 调用点改动
```

## 实施顺序

1→2（能编译）→3+4（并行）→5→6（核心）→7（收尾）→全量验证 + review plan 回写偏差到本计划文件。

---

## 实施记录（review 阶段回写实际偏差）

### 任务合并 vs 分解

原计划 7 个 Task 分解，实际实施时把 Task 1（两级 outline）+3（路径/原子写）+4 sync 部分（HotwordWordFile/md5）+5（export word）+6（word merge）合并到一次重写 `crates/sync/src/hotword.rs`——因为这些任务在同一个文件且高度耦合（HotwordOutline 决定路径函数签名，路径决定 export/merge 遍历结构），分开改反而引入中间编译态。Task 2（vault 测试）+ Task 4 DB 部分（infra sync_md5）+ Task 7（desktop + 文档）独立完成。

### 实际偏差（vs 计划）

1. **`export_all_hotwords_with` / `incremental_export_hotwords_with` 保留参数版本**：计划 D7 说全部无参。实际为测试隔离（纯文件系统测试用 `SyncRootGuard` 无 DB），保留了 `*(sets, words)` 参数版本（`pub`），无参版本内部读 DB 后委托参数版本。这是合理的——对齐 vault `export_all_to_files` 也接收数据参数的模式。

2. **DB 层 `set_words_in_set_at` 软删部分需逐词算 md5**：原 SQL 是批量 `UPDATE ... WHERE word NOT IN (...)`，但 md5 需逐词的 pinyin。实际改为先 `SELECT word, pinyin` 查出待软删词，再逐词 `UPDATE ... SET sync_md5=?`（多一轮查询，但词数 ≤3000 可接受）。

3. **vault engine 测试额外修了 `set_hotword_set_words` broken 调用**：计划只说改 `outline.ciphers`→`sets`。实际发现这 3 个测试在 v57 迁移后已 broken（引用已删的 `set_hotword_set_words`），一并改成 `add_words_to_set` + `hotword_meta_file_path`——让 vault test target 终于能编译（main 上其实已 broken，本任务顺带修好）。

4. **`collect_json_files` 死代码删除**：原扁平 `import_hotwords_from_files` 用它递归扫 `sets/`，新两级结构改为扫词典目录的 `meta.json`，旧的 `collect_json_files` 成死代码，删掉（编译 warning 驱动）。

5. **未删 `pull_hotwords_from_files` / `push_hotwords_to_files`**：计划没说删，实际保留（首次 clone 场景 + 测试 reference，注释标明「已被 merge 取代，常规 sync 不用」）。与 vault 的死代码清理对称（vault 删了 `pull_from_files`，但热词的还有 vault 集成测试引用，留待后续清理）。

### 验证结果（全过）

| 验证项 | 结果 |
|---|---|
| `cargo test -p octopus-sync --lib` | ✅ 121 passed（原 108 + 新增 13：md5/路径/word文件/两级 outline/word merge 5 场景） |
| `cargo test -p octopus-infra --lib` | ✅ 170 passed（原 168 + 新增 2：word sync_md5 填充） |
| `cargo test -p octopus-vault --lib` | ✅ 258 passed（修复 3 个 broken 测试，0 回归） |
| `cargo build --workspace` | ✅ 0 error |
| `cargo test --workspace --lib` | ✅ 全 crate 0 failed（跨 crate 无回归） |
| desktop `hotword_commands` 测试 | ✅ 12 passed（含 2 个新 word-md5 测试，2 个 set-md5 行为变更测试） |
| tsc | N/A（无前端改动——全部 Rust + 文档） |

### 核心 merge 测试覆盖（5 场景全过）

- `merge_pulls_remote_newer_word` ✅
- `merge_keeps_local_newer_word_not_overwritten` ✅
- `merge_pushes_db_only_word` ✅
- `merge_soft_delete_propagates` ✅（软删跨设备传播）
- `merge_db_wins_on_equal_timestamp_word_conflict` ✅

