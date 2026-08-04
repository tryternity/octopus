# 内联资源集中化实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把散落在 9 个 crate 的 22 处 `include_bytes!`/`include_str!` 集中到 `infra/resources/`，由 `infra::resources` 统一加载。

**Architecture:** infra 新增 `resources` 模块 + `resources/` 目录（sql/dicts/models/vad/prompts 子目录）。通用资源物理搬入；crate 专有资源保留原位但用 `env!("CARGO_MANIFEST_DIR")` 消除 `../../`。

**Tech Stack:** Rust 2021 workspace。

## Global Constraints

- **零行为变更**——各 crate 测试数不减（baseline：infra 183 / vault 258 / ocr 34 / asr-local 170 / clipboard 23 / record 50 / capx 55）
- **`cargo build --workspace`**：0 error 0 warning
- **`cargo clippy --workspace --all-targets`**：0 warning（baseline 71，不新增）
- **Worktree**：`.worktrees/refactor/centralize-resources` 分支 `refactor/centralize-resources`
- **每 Task 独立 commit** + 相关 crate `cargo test` 通过
- **git mv 保 blame**——用 `git mv` 不用 cp+rm
- **未经用户明确指令不 push 到 main**

---

## File Structure

```
crates/infra/
├── src/
│   ├── resources.rs          # 新增：8 个 pub fn 加载 API
│   └── lib.rs                # 加 pub mod resources;
├── resources/                # 新增目录
│   ├── sql/schema.sql
│   ├── dicts/{public_suffix_list.dat, words_common.txt, t2s.txt, s2t.txt, unigram.txt.gz, NOTICE}
│   ├── models/vad/silero_vad_v6.onnx
│   └── prompts/hotword_mine.md
└── Cargo.toml                # 不改
```

---

## Task 1: 建 infra/resources/ 骨架

**Files:**
- Create: `crates/infra/src/resources.rs`
- Modify: `crates/infra/src/lib.rs`

- [x] **Step 1: 创建 resources.rs（空骨架）**

```rust
//! 编译期内联资源统一入口。
//!
//! 2026-08-04 集中化：DB schema / 字典 / 模型 / prompt 从各 crate 散落的
//! include_bytes!/include_str! 集中到 infra/resources/，消除跨 crate ../../ 脆弱路径。
//! crate 专有资源（desktop icon/i18n/tauri.conf、pty shell 脚本）保留原位，
//! 调用方用 env!("CARGO_MANIFEST_DIR") 消除 ../../。
```

- [x] **Step 2: lib.rs 加 pub mod**

在 `crates/infra/src/lib.rs` 找到现有 `pub mod` 列表，按字母序插入 `pub mod resources;`（在 `pub mod paths;` 之后、`pub mod seeds;` 之前）。

- [x] **Step 3: 建空目录**

```bash
mkdir -p crates/infra/resources/sql
mkdir -p crates/infra/resources/dicts
mkdir -p crates/infra/resources/models/vad
mkdir -p crates/infra/resources/prompts
```

- [x] **Step 4: 验证**

```bash
cargo build -p octopus-infra 2>&1 | tail -3
cargo test -p octopus-infra 2>&1 | grep "test result" | head -1
```

Expected: 0 error / 183 passed。可能有 "unused module" warning——下个 task 消化。

- [x] **Step 5: Commit**

```bash
git add crates/infra/src/resources.rs crates/infra/src/lib.rs
git commit -m "refactor(infra): 建 resources 模块骨架

- src/resources.rs：模块文档占位
- lib.rs 加 pub mod resources;
- 建 resources/{sql,dicts,models/vad,prompts}/ 空目录
- 下个 task 逐步搬入资源"
```

---

## Task 2: DB schema 搬迁

**Files:**
- Move: `crates/infra/src/db.sql` → `crates/infra/resources/sql/schema.sql`
- Modify: `crates/infra/src/resources.rs`
- Modify: `crates/infra/src/db/mod.rs:101`
- Modify: `crates/infra/src/seeds.rs:288`
- Modify: `crates/infra/src/db/vault.rs:583`
- Modify: `crates/clipboard/src/store.rs:611`
- Modify: `crates/clipboard/src/cleanup.rs:123`
- Modify: `crates/record/src/store.rs:227`

- [x] **Step 1: git mv db.sql**

```bash
git mv crates/infra/src/db.sql crates/infra/resources/sql/schema.sql
```

- [x] **Step 2: 加 db_schema_sql() 到 resources.rs**

```rust
/// SQLite schema（含表结构 + 短种子；长 seed 走 seeds.rs 运行时加载）。
pub fn db_schema_sql() -> &'static str {
    include_str!("resources/sql/schema.sql")
}
```

放在模块文档之后。

- [x] **Step 3: 改 infra 内 3 处消费点**

`crates/infra/src/db/mod.rs:101`：
```rust
// 前
const INIT_SQL: &str = include_str!("../db.sql");
// 后
const INIT_SQL: &str = crate::resources::db_schema_sql();
```

`crates/infra/src/seeds.rs:288`：
```rust
// 前
conn.execute_batch(include_str!("db.sql")).unwrap();
// 后
conn.execute_batch(crate::resources::db_schema_sql()).unwrap();
```

`crates/infra/src/db/vault.rs:583`：
```rust
// 前
conn.execute_batch(include_str!("../db.sql")).unwrap();
// 后
conn.execute_batch(crate::resources::db_schema_sql()).unwrap();
```

- [x] **Step 4: 改 clipboard 2 处消费点**

`crates/clipboard/src/store.rs:611`：
```rust
// 前
let sql = include_str!("../../infra/src/db.sql");
// 后
let sql = octopus_infra::resources::db_schema_sql();
```

`crates/clipboard/src/cleanup.rs:123`：同样替换。

- [x] **Step 5: 改 record 1 处消费点**

`crates/record/src/store.rs:227`：
```rust
// 前
let sql = include_str!("../../infra/src/db.sql");
// 后
let sql = octopus_infra::resources::db_schema_sql();
```

- [x] **Step 6: 验证**

```bash
cargo build -p octopus-infra -p octopus-clipboard -p octopus-record 2>&1 | tail -5
cargo test -p octopus-infra 2>&1 | grep "test result" | head -1
cargo test -p octopus-clipboard 2>&1 | grep "test result" | head -1
cargo test -p octopus-record 2>&1 | grep "test result" | head -1
```

Expected: 0 error / 183 + 23 + 50 passed。

- [x] **Step 7: 残留检查**

```bash
rg "infra/src/db\.sql\|include_str!.*db\.sql" crates/ --type rust | grep -v target
```

Expected: 空。

- [x] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(infra): DB schema 搬到 resources/sql/schema.sql

- git mv infra/src/db.sql → resources/sql/schema.sql
- resources.rs 加 db_schema_sql()
- 6 处消费点改调 db_schema_sql()（infra 内 3 + clipboard 2 + record 1）

零行为变更：infra 183 / clipboard 23 / record 50 passed"
```

---

## Task 3: vault 资源搬迁（首次新增 infra 依赖）

**Files:**
- Move: `crates/vault/data/public_suffix_list.dat` → `crates/infra/resources/dicts/`
- Delete: `crates/vault/data/eff_large_wordlist.txt`（死文件）
- Modify: `crates/infra/src/resources.rs`
- Modify: `crates/vault/src/matcher/psl.rs:26` + 注释
- Modify: `crates/vault/Cargo.toml`

- [x] **Step 1: git mv psl + 删死文件**

```bash
git mv crates/vault/data/public_suffix_list.dat crates/infra/resources/dicts/
git rm crates/vault/data/eff_large_wordlist.txt
```

- [x] **Step 2: 加 public_suffix_list() 到 resources.rs**

```rust
/// Mozilla Public Suffix List（vault 域名匹配用）。
/// 季度级同步：curl -o crates/infra/resources/dicts/public_suffix_list.dat \
///   https://publicsuffix.org/list/public_suffix_list.dat
pub fn public_suffix_list() -> &'static [u8] {
    include_bytes!("resources/dicts/public_suffix_list.dat")
}
```

- [x] **Step 3: vault/Cargo.toml 加 infra 依赖**

在 `[dependencies]` 加（如果没有）：
```toml
octopus-infra = { path = "../infra" }
```

- [x] **Step 4: 改 psl.rs 消费点 + 注释**

`crates/vault/src/matcher/psl.rs:26`：
```rust
// 前
static PSL_BYTES: &[u8] = include_bytes!("../../data/public_suffix_list.dat");
// 后
static PSL_BYTES: &[u8] = octopus_infra::resources::public_suffix_list();
```

`psl.rs:22-24` 注释里的 curl 路径：
```bash
# 前
curl -o crates/vault/data/public_suffix_list.dat \
  https://publicsuffix.org/list/public_suffix_list.dat
# 后
curl -o crates/infra/resources/dicts/public_suffix_list.dat \
  https://publicsuffix.org/list/public_suffix_list.dat
```

- [x] **Step 5: 验证**

```bash
cargo build -p octopus-vault 2>&1 | tail -5
cargo test -p octopus-vault 2>&1 | grep "test result" | head -1
```

Expected: 0 error / 258 passed。

- [x] **Step 6: 删空目录 vault/data/**

```bash
# 如果 git mv + git rm 后 vault/data/ 空了
rmdir crates/vault/data 2>/dev/null || ls -la crates/vault/data
```

- [x] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(vault): public_suffix_list 搬到 infra/resources/dicts/

- git mv vault/data/public_suffix_list.dat → infra/resources/dicts/
- git rm vault/data/eff_large_wordlist.txt（死文件 106KB，零引用）
- resources.rs 加 public_suffix_list()
- psl.rs 改调 octopus_infra::resources::public_suffix_list()
- vault/Cargo.toml 新增 octopus-infra 依赖（单向无环）
- psl.rs 注释 curl 路径同步更新

零行为变更：vault 258 passed"
```

---

## Task 4: ocr 资源搬迁

**Files:**
- Move: `crates/ocr/assets/words_common.txt` → `crates/infra/resources/dicts/`
- Modify: `crates/infra/src/resources.rs`
- Modify: `crates/ocr/src/engine.rs:349`
- Modify: `crates/ocr/Cargo.toml`

- [x] **Step 1: git mv**

```bash
git mv crates/ocr/assets/words_common.txt crates/infra/resources/dicts/
```

- [x] **Step 2: 加 ocr_words_common() 到 resources.rs**

```rust
/// OCR 常用词表（ocr 引擎识别后纠错用）。
pub fn ocr_words_common() -> &'static str {
    include_str!("resources/dicts/words_common.txt")
}
```

- [x] **Step 3: ocr/Cargo.toml 加 infra 依赖**

```toml
octopus-infra = { path = "../infra" }
```

- [x] **Step 4: 改 engine.rs 消费点**

`crates/ocr/src/engine.rs:349`：
```rust
// 前
const WORDS_RAW: &str = include_str!("../assets/words_common.txt");
// 后
const WORDS_RAW: &str = octopus_infra::resources::ocr_words_common();
```

- [x] **Step 5: 验证 + 删空目录**

```bash
cargo build -p octopus-ocr 2>&1 | tail -5
cargo test -p octopus-ocr 2>&1 | grep "test result" | head -1
rmdir crates/ocr/assets 2>/dev/null || ls crates/ocr/assets
```

Expected: 0 error / 34 passed。

- [x] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(ocr): words_common 搬到 infra/resources/dicts/

- git mv ocr/assets/words_common.txt → infra/resources/dicts/
- resources.rs 加 ocr_words_common()
- engine.rs 改调 octopus_infra::resources::ocr_words_common()
- ocr/Cargo.toml 新增 octopus-infra 依赖

零行为变更：ocr 34 passed"
```

---

## Task 5: asr-local 资源搬迁（4 个文件）

**Files:**
- Move: `crates/asr-local/data/{t2s.txt,s2t.txt,NOTICE}` → `crates/infra/resources/dicts/`
- Move: `crates/asr-local/src/text/corrector_data/unigram.txt.gz` → `crates/infra/resources/dicts/`
- Move: `crates/asr-local/models/silero_vad_v6.onnx` → `crates/infra/resources/models/vad/`
- Modify: `crates/infra/src/resources.rs`
- Modify: `crates/asr-local/src/text/hans.rs:18,20`
- Modify: `crates/asr-local/src/text/corrector.rs:11`
- Modify: `crates/asr-local/src/audio/vad.rs:27`

- [x] **Step 1: git mv 5 个文件**

```bash
git mv crates/asr-local/data/t2s.txt crates/infra/resources/dicts/
git mv crates/asr-local/data/s2t.txt crates/infra/resources/dicts/
git mv crates/asr-local/data/NOTICE crates/infra/resources/dicts/
git mv crates/asr-local/src/text/corrector_data/unigram.txt.gz crates/infra/resources/dicts/
git mv crates/asr-local/models/silero_vad_v6.onnx crates/infra/resources/models/vad/
```

- [x] **Step 2: 加 4 个 fn 到 resources.rs**

```rust
/// 简繁转换：简体→繁体映射表。
pub fn hans_t2s() -> &'static str {
    include_str!("resources/dicts/t2s.txt")
}

/// 简繁转换：繁体→简体映射表。
pub fn hans_s2t() -> &'static str {
    include_str!("resources/dicts/s2t.txt")
}

/// ASR 文本纠错 unigram（gzip 压缩，运行时解压）。
pub fn corrector_unigram_gz() -> &'static [u8] {
    include_bytes!("resources/dicts/unigram.txt.gz")
}

/// Silero VAD v6 ONNX 模型（语音端点检测）。
/// 用户可在 ~/.octopus/models/vad.onnx 放自定义版本覆盖（见 asr vad.rs）。
pub fn silero_vad_v6_onnx() -> &'static [u8] {
    include_bytes!("resources/models/vad/silero_vad_v6.onnx")
}
```

- [x] **Step 3: 改 hans.rs 2 处**

`crates/asr-local/src/text/hans.rs:18,20`：
```rust
// 前
const T2S_DATA: &str = include_str!("../../data/t2s.txt");
const S2T_DATA: &str = include_str!("../../data/s2t.txt");
// 后
const T2S_DATA: &str = octopus_infra::resources::hans_t2s();
const S2T_DATA: &str = octopus_infra::resources::hans_s2t();
```

- [x] **Step 4: 改 corrector.rs 1 处**

`crates/asr-local/src/text/corrector.rs:11`：
```rust
// 前
const UNIGRAM_GZ: &[u8] = include_bytes!("corrector_data/unigram.txt.gz");
// 后
const UNIGRAM_GZ: &[u8] = octopus_infra::resources::corrector_unigram_gz();
```

- [x] **Step 5: 改 vad.rs 1 处**

`crates/asr-local/src/audio/vad.rs:27`：
```rust
// 前
const VAD_BYTES: &[u8] = include_bytes!("../../models/silero_vad_v6.onnx");
// 后
const VAD_BYTES: &[u8] = octopus_infra::resources::silero_vad_v6_onnx();
```

- [x] **Step 6: 验证 + 删空目录**

```bash
cargo build -p octopus-asr-local 2>&1 | tail -5
cargo test -p octopus-asr-local 2>&1 | grep "test result" | head -1
# 删空目录
rmdir crates/asr-local/data crates/asr-local/models crates/asr-local/src/text/corrector_data 2>/dev/null || echo "部分目录非空，检查"
```

Expected: 0 error / 170 passed。

- [x] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(asr-local): t2s/s2t/unigram/vad 搬到 infra/resources/

- git mv t2s.txt/s2t.txt/NOTICE/unigram.txt.gz → infra/resources/dicts/
- git mv silero_vad_v6.onnx → infra/resources/models/vad/
- resources.rs 加 hans_t2s() / hans_s2t() / corrector_unigram_gz() / silero_vad_v6_onnx()
- hans.rs / corrector.rs / vad.rs 改调 octopus_infra::resources::

零行为变更：asr-local 170 passed"
```

---

## Task 6: hotword prompt + env! 路径统一

**Files:**
- Move: `crates/desktop/resources/hotword_mine.md` → `crates/infra/resources/prompts/`
- Modify: `crates/infra/src/resources.rs`
- Modify: `crates/desktop/src/commands/hotword_commands.rs:188`
- Modify: `crates/desktop/src/ui/i18n.rs:4,5`
- Modify: `crates/desktop/src/ui/tray.rs:363`
- Modify: `crates/desktop/src/ui/settings_window.rs:108`
- Modify: `crates/desktop/src/vault/autotype/macos.rs:293`

- [x] **Step 1: git mv hotword prompt**

```bash
git mv crates/desktop/resources/hotword_mine.md crates/infra/resources/prompts/
```

- [x] **Step 2: 加 hotword_mine_prompt() 到 resources.rs**

```rust
/// 热词挖掘 LLM prompt（从用户编辑文本提取热词候选）。
pub fn hotword_mine_prompt() -> &'static str {
    include_str!("resources/prompts/hotword_mine.md")
}
```

- [x] **Step 3: 改 hotword_commands.rs**

`crates/desktop/src/commands/hotword_commands.rs:188`：
```rust
// 前
const HOTWORD_MINE_PROMPT: &str = include_str!("../../resources/hotword_mine.md");
// 后
const HOTWORD_MINE_PROMPT: &str = octopus_infra::resources::hotword_mine_prompt();
```

- [x] **Step 4: 改 desktop 5 处 env!**

`crates/desktop/src/ui/i18n.rs:4,5`：
```rust
// 前
const ZH_CN_YAML: &str = include_str!("../../frontend/src/locales/zh-CN.yaml");
const EN_YAML: &str = include_str!("../../frontend/src/locales/en.yaml");
// 后
const ZH_CN_YAML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/frontend/src/locales/zh-CN.yaml"));
const EN_YAML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/frontend/src/locales/en.yaml"));
```

`crates/desktop/src/ui/tray.rs:363`：
```rust
// 前
.unwrap_or_else(|| Image::from_bytes(include_bytes!("../../icons/icon.png")).unwrap()),
// 后
.unwrap_or_else(|| Image::from_bytes(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/icon.png"))).unwrap()),
```

`crates/desktop/src/ui/settings_window.rs:108`：
```rust
// 前
const ICON_PNG: &[u8] = include_bytes!("../../icons/icon.png");
// 后
const ICON_PNG: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/icon.png"));
```

`crates/desktop/src/vault/autotype/macos.rs:293`：
```rust
// 前
let conf = include_str!("../../../tauri.conf.json");
// 后
let conf = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"));
```

- [x] **Step 5: 验证 + 删空目录**

```bash
cargo build -p octopus-desktop 2>&1 | tail -5
rmdir crates/desktop/resources 2>/dev/null || ls crates/desktop/resources
```

注意：desktop build 可能需要先 `./scripts/build-macos-helper.sh`。

- [x] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(desktop): hotword prompt 搬 infra + 5 处 ../../ 改 env!

- git mv desktop/resources/hotword_mine.md → infra/resources/prompts/
- resources.rs 加 hotword_mine_prompt()
- hotword_commands.rs 改调 octopus_infra::resources::hotword_mine_prompt()
- 5 处 ../../ 改 concat!(env!(CARGO_MANIFEST_DIR), ...)：
  i18n.rs×2（yaml） / tray.rs（icon） / settings_window.rs（icon） / macos.rs（tauri.conf）

零行为变更：desktop build 0 error"
```

---

## Task 7: 清理 + 最终验证

**Files:**
- Verify: 残留引用 + 空目录 + 全量测试

- [x] **Step 1: 残留引用扫描**

```bash
rg "infra/src/db\.sql|vault/data/|ocr/assets/|asr-local/data/|asr-local/models/|corrector_data/|desktop/resources/" crates/ --type rust | grep -v target
```

Expected: 空（或仅注释里的历史引用）。

- [x] **Step 2: 空目录扫描**

```bash
for d in crates/vault/data crates/ocr/assets crates/asr-local/data crates/asr-local/models crates/asr-local/src/text/corrector_data crates/desktop/resources; do
  [ -d "$d" ] && echo "❌ 还存在: $d" || echo "✅ 已删: $d"
done
```

Expected: 全 ✅。

- [x] **Step 3: 全量 build + test**

```bash
cargo build --workspace 2>&1 | tail -5
cargo test --workspace 2>&1 | grep "test result" | head -10
```

Expected: 0 error / 各 crate 测试数不减（infra 183 / vault 258 / ocr 34 / asr-local 170 / clipboard 23 / record 50 / capx 55）。

- [x] **Step 4: clippy**

```bash
cargo clippy --workspace --all-targets 2>&1 | grep -c "^warning:"
```

Expected: ≤ 71 baseline（不新增）。

- [x] **Step 5: 残留 ../../ 扫描（验证消除）**

```bash
rg "include_(str|bytes)!\(\"\.\./\.\." crates/ --type rust | grep -v target
```

Expected: 空（pty 本就没 `../../`；其他都改了）。

- [x] **Step 6: infra/resources 结构确认**

```bash
find crates/infra/resources -type f
```

Expected:
```
crates/infra/resources/sql/schema.sql
crates/infra/resources/dicts/NOTICE
crates/infra/resources/dicts/public_suffix_list.dat
crates/infra/resources/dicts/s2t.txt
crates/infra/resources/dicts/t2s.txt
crates/infra/resources/dicts/unigram.txt.gz
crates/infra/resources/dicts/words_common.txt
crates/infra/resources/models/vad/silero_vad_v6.onnx
crates/infra/resources/prompts/hotword_mine.md
```

- [x] **Step 7: 下游 desktop build**

```bash
./scripts/build-macos-helper.sh
cargo build --release -p octopus-desktop 2>&1 | tail -5
```

Expected: 0 error。

- [x] **Step 8: 文档同步**

更新 `docs/architecture.md` §infra 加 resources 模块描述。

- [x] **Step 9: 最终 Commit**

```bash
git add docs/architecture.md
git commit -m "docs(architecture): infra resources 模块描述

内联资源集中化完成：sql/dicts/models/prompts 4 类资源统一入口。"
```

- [x] **Step 10: 汇报用户**

报告：
- 7 个文件搬入 infra/resources/
- 1 个死文件删除
- 14 处消费点改为调 infra API
- 5 处 ../../ 改 env!
- vault/ocr 新增 infra 依赖
- 全量测试通过 + clippy 不新增 warning

**未经用户明确指令不 push 到 main。**
