# 实施计划：sync export 原子化（P2-sync1/sync2）

- 日期：2026-08-10
- 设计 spec：[`docs/superpowers/specs/2026-08-05-sync-export-atomicity-design.md`](../specs/2026-08-05-sync-export-atomicity-design.md)
- 触发：第二十三轮 P2-sync1/sync2（= 第 21 轮 P2-s1/s2），用户指令实施
- 分支：`bugfix/pr-0801`

---

## 方案

**方案 B（先写后清孤儿）+ 方案 C（删冗余 push 写）合并**，详见设计 spec §3/§4。

核心不变量：
1. export 不再 `remove_dir_all + 重建`，改为「先全量写新文件 → 扫目录删非 DB 孤儿」
2. outline 最后写（write_atomically 原子），保证崩溃后 outline 要么旧（自愈）要么新（与文件一致）
3. 孤儿不影响 merge pull（pull 按 outline 走，孤儿不在 outline）
4. merge 的 push_or_skip 不再 write_file（被 export_all 覆盖，省冗余 IO）

---

## Task 分解

### Task 1 · clipboard export_all 先写后清孤儿

**文件**：`crates/sync/src/clipboard.rs`

**变更点**：
- `export_all_favorites`（:427-519）：删 :438-443 的 `remove_dir_all(fav_dir) + create_dir_all(fav_dir)`；改为 `create_dir_all(fav_dir)`（确保存在，不删）；写完所有 favorite 后调 `cleanup_orphan_favorite_files(&keep_keys)`
- 新增 `cleanup_orphan_favorite_files(keep_keys: &HashSet<String>)`：遍历 `favorites/<2hex>/*.json`，删 stem 不在 keep_keys 的孤儿文件
- 收集 keep_keys：在写 favorite 循环里 `keep_keys.insert(fav.history_id.clone())`（含 active + tombstone，因为两者都写文件）

**验证命令**：
```bash
cargo test -p octopus-sync --lib clipboard
```

---

### Task 2 · hotword export_all 先写后清孤儿

**文件**：`crates/sync/src/hotword.rs`

**变更点**：
- `export_all_hotwords_with`（:414-495）：删 :426-437 的「清空所有词典目录」循环；改为只 `create_dir_all(dir)`（确保存在）；写完所有 set 后调 `cleanup_orphan_hotword_files(&keep_set_ids, &words_by_set, now_secs)`
- 新增 `cleanup_orphan_hotword_files(keep_set_ids, words_by_set, now_secs)`：两级清理
  - set 级：`hotword/<set-id>/` 不在 keep_set_ids → 删整个目录（含超期 tombstone set）
  - word 级：存活的 set 内，`<2hex>/<word-uuid>.json` 不在 keep_word_ids（含超期 word tombstone 过滤）→ 删文件

**验证命令**：
```bash
cargo test -p octopus-sync --lib hotword
```

---

### Task 3 · vault export_all_to_files 先写后清孤儿

**文件**：`crates/vault/src/sync/store.rs`

**变更点**：
- `export_all_to_files`（:516-?）：删 :529/:532 的 `remove_dir_all(ciphers_dir/folders_dir)`；改为只 `create_dir_all`（确保存在）；写完后清孤儿
- 新增 `cleanup_orphan_cipher_files(keep_ids)` + `cleanup_orphan_folder_files(keep_ids)`：扁平结构（`ciphers/<2hex>/<uuid>.json` / `folders/<uuid>.json`）

**注意**：vault 的 cipher 是 `<2hex>/<uuid>.json` 分片，folder 是扁平 `<uuid>.json`。folder 无分片。

**验证命令**：
```bash
cargo test -p octopus-vault --lib sync::store
```

---

### Task 4 · pipeline push_or_skip 删冗余 write_file

**文件**：`crates/sync/src/pipeline.rs`

**变更点**：
- `push_or_skip`（:240-251）：不调 `E::write_file(row)`，只记 `report.pushed += 1` + 返 true
- 加不变量注释：export_all 必须全量重建（写所有 DB 行），push 依赖此

**验证命令**：
```bash
cargo test -p octopus-sync --lib
```

---

### Task 5 · 回归测试

**新增测试**：

1. **clipboard 孤儿清理**（clipboard.rs test mod）：
   - export 后手动塞孤儿 .json → 再 export → 断言孤儿被清
   
2. **hotword 孤儿清理**（hotword.rs test mod）：
   - set 级孤儿（DB 无 set X 但目录存在）→ export → set 目录被清
   - word 级孤儿（set 存活但含 DB 无的词文件）→ export → 词文件被清

3. **vault 孤儿清理**（store.rs test mod）：
   - cipher 孤儿 + folder 孤儿同理

4. **push 不再 write_file**（pipeline.rs test mod，需 mock SyncEntity）：
   - 验证 push 分支不调 write_file（用计数器 trait mock）

**验证命令**：
```bash
cargo test -p octopus-sync -p octopus-vault --lib
```

---

## 风险与回退

- **风险**：孤儿清理的 `read_dir` 遍历在分片子目录结构错误的实体上报错（如目录含非 .json 文件）。对策：只删 `.json` 后缀 + stem 匹配，其他跳过。
- **回退**：每个 Task 独立 commit，任一 Task 测试失败可单独回退。

## 完成标准

- 3 处 export_all 去 remove_dir_all + 加孤儿清理
- pipeline push_or_skip 删 write_file
- 全量测试通过（sync 153+ / vault 264+ / desktop 525+）
- spec 标注「已实施」+ audit spec 留后续表移除 P2-sync1/sync2
