# infra/db.rs 拆分 plan（infra crate 大文件重构）

> **对应 spec**: `docs/superpowers/specs/2026-07-30-infra-db-split.md`
> **分支**: `daily_refactor_asr_local`

## 阶段 0：目录化

### Task 0.1 — db.rs → db/mod.rs
- `mkdir -p crates/infra/src/db && git mv crates/infra/src/db.rs crates/infra/src/db/mod.rs`
- 验证：build + test

---

## 阶段 1：子模块提取（按行数从小到大）

### Task 1.1 — prompts.rs（~130 行）
搬出：PromptRecord struct + row_to_prompt + list/load/insert/update/delete prompt + load/save active_prompt_id + 对应测试

### Task 1.2 — config.rs（~200 行）
搬出：load/save_app_config(_at) + coerce_db_string + save/load_config_key + list/save/delete env_var + 对应测试

### Task 1.3 — transcription.rs（~200 行）
搬出：insert_transcription_at_id + update_text_segments/polished/edited_segments + finalize + list/delete transcriptions + TranscriptionRecord + escape_fts5_match + 对应测试

### Task 1.4 — agent.rs（~250 行）
搬出：AgentAdapterRecord + AgentTask struct + CRUD + set/clear_default_agent + 对应测试

### Task 1.5 — hotword.rs（~350 行）
搬出：HotwordSet + row_to_hotword_set + CRUD + words + hits + recent_text + list_active_hotword_words + 对应测试

### Task 1.6 — action_bar.rs（~450 行）
搬出：ActionBarItem + row_to_action_bar_item + CRUD + validate_shortcut + check_shortcut_conflict + launcher + search_frequency + ScriptRun + script_run CRUD + 对应测试

### Task 1.7 — vault.rs（~600 行）
搬出：VaultMeta/Cipher/Folder + Input struct + row mapper + meta/cipher/folder CRUD + security_stamp + secret migration + 对应测试

### Task 1.8 — models.rs（~900 行，最大）
搬出：ModelEntry/AsrSection/AsrConfig/CompatibleLlmConfig/LocalAsrModelRow/ModelRow/ModelDetailRow/AsrEngineRow/LlmProviderPresetRow/LlmModelInfo/OcrModelInfo/ModelSpec + 所有 model CRUD + 对应测试

---

## 阶段 2：收尾

### Task 2.1 — 全量验证 + 文档同步
- 全量验证：infra test + desktop embedded/cloud,vault + cli + server
- architecture.md 更新

---

## 每个 Task 的统一模式
1. 读 mod.rs 中目标函数完整内容
2. 创建 `db/<子文件>.rs`，搬入函数 + struct（保留 pub 可见性）
3. mod.rs 顶部加 `mod <子文件>; pub use <子文件>::*;`（glob re-export）
4. 从 mod.rs 删除这些函数
5. `cargo build -p octopus-infra` + `cargo test -p octopus-infra` 验证

## 验证 checklist
- [ ] `cargo build -p octopus-infra` — 0 error 0 warning
- [ ] `cargo test -p octopus-infra` — 全部 passed
- [ ] `cargo build -p octopus-desktop --features embedded,cloud,vault` — 0 error

## 回滚
每个 Task 独立 commit。失败 `git reset --hard HEAD~1`。
