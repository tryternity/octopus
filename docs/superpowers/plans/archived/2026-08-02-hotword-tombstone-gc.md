# 热词 tombstone GC 实施计划

> **日期**：2026-08-02
> **spec**：`docs/superpowers/specs/2026-08-02-hotword-tombstone-gc.md`
> **分支**：`feature/hotword-tombstone-gc`（worktree `.worktrees/hotword-tombstone-gc`）
> **状态**：🔄 实施中

## 任务分解（TDD）

### Task 1：word is_deleted bool→i64 秒统一（infra + sync）
- `HotwordWord.is_deleted: bool` → `i64`（infra struct）+ `row_to_hotword_word`
- `HotwordWordFile.is_deleted: bool` → `i64`（sync，version 1→2 serde default）
- `hotword_word_md5_from_fields` 第 4 参 `bool` → `i64`
- `remove_word_from_set_at`：`SET is_deleted=1` → `SET is_deleted=<now_secs>`
- `add_word_to_set_at` / `set_words_in_set_at`：恢复 `is_deleted=0`（不变）
- **TDD**：word 软删后 is_deleted>0（秒级 epoch）+ 恢复后 is_deleted=0

### Task 2：DB 层 GC（infra db/hotword.rs）
- 常量 `HOTWORD_TOMBSTONE_RETENTION_SECS = 10 * 86400`（放 infra）
- `purge_expired_hotword_tombstones(now_secs) -> usize`：硬删超期 set tombstone + 其词 + hits + 超期 word tombstone
- `count_hotword_tombstones() -> i64`：set tombstone 数（前端按钮用）
- `purge_all_hotword_tombstones() -> usize`：手动清空（不限年龄）
- **TDD**：超期 tombstone purge 后硬删 + 活跃词典不动

### Task 3：sync merge 按年龄过滤 tombstone（核心——防复活）
- `pull_set`：读 meta.json → if is_deleted>0 且超期 → skip
- `pull_word`：读 word file → if is_deleted>0 且超期 → skip
- `export_all_hotwords` / `incremental_export`：超期 set 不写文件 + outline 不含 + 删目录；超期 word 不写文件 + outline 不含
- **TDD**：A 软删 set（is_deleted=远过去超期）→ B merge → B 不 pull（不复活）+ B export 不含超期

### Task 4：scheduler 每日 GC + 手动清空命令 + 前端按钮
- `setup.rs init_scheduler`：register_task("hotword_tombstone_gc", 86400, purge + export)
- desktop 命令 `purge_hotword_tombstones` + `count_hotword_tombstones` + 注册 invoke_handler
- 前端 HotwordPanel.tsx「manage」tab 回收站按钮（frontend-design skill）
- **TDD**：命令层集成测试

## 验证
```bash
cargo test -p octopus-infra --lib
cargo test -p octopus-sync --lib
cargo build --workspace
cargo test --workspace --lib
tsc
```

## 实施记录（review 阶段回写实际偏差）

### 实际偏差

1. **word 旧文件兼容测试用 `_pub` 包装**：infra GC 函数用 `ensure_db`/`with_db`（全局 DB），in-memory 测试隔离需绕过。加了 `_pub` 测试包装（复制核心逻辑，接收 conn）。生产函数不受影响。
2. **`merge_soft_delete_propagates` 测试调整**：原 `write_remote_word(..., true, ...)` 改 i64 后用 `1700000000`（超期），导致年龄过滤 skip 不传播。改为 `now_secs`（未超期 tombstone，正常传播）。
3. **前端按钮条件渲染**：`tombstoneCount > 0` 才显示「🗑 N」按钮（无 tombstone 时不占位）。refresh 时 fetch count。

### 验证结果

| 验证项 | 结果 |
|---|---|
| `cargo test -p octopus-infra --lib` | ✅ 全过（+2 GC 测试：超期 purge + 手动清空） |
| `cargo test -p octopus-sync --lib` | ✅ 124 passed（+2 GC 年龄过滤：set/word 超期不复活） |
| `cargo build --workspace` | ✅ 0 error |
| `cargo test --workspace --lib` | ✅ 17 crate 全过，0 failed |
| desktop `hotword_commands` | ✅ 12 passed |
| tsc | ✅ EXIT 0（前端按钮 + i18n） |

