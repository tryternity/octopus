# 内联资源集中化设计

- 日期：2026-08-04
- 分支：`refactor/centralize-resources`
- Worktree：`.worktrees/refactor/centralize-resources`
- 类型：重构（纯结构调整，零行为变更）
- Baseline：见下表

---

## 1. 背景与动机

### 1.1 现状

工程中 **22 处** `include_bytes!`/`include_str!` 散落在 9 个 crate，大量使用 `../../` 跨 crate 相对路径：

```rust
// 脆弱路径示例——文件移动即坏
include_str!("../../infra/src/db.sql")          // clipboard/record
include_bytes!("../../data/public_suffix_list.dat")  // vault
include_bytes!("../../models/silero_vad_v6.onnx")    // asr-local
```

问题：
- **`../../` 魔法路径**：相对当前源文件，文件移动就要改——脆弱
- **资源散落**：同类资源（字典/词表）分散在 vault/data、ocr/assets、asr-local/data 三处
- **无统一入口**：找不到"工程内嵌了哪些资源"的总览
- **死文件残留**：`vault/data/eff_large_wordlist.txt`（106KB）零引用

### 1.2 目标

把**通用资源**（DB schema / 字典 / 模型 / 业务 prompt）集中到 `infra/resources/`，由 `infra::resources` 模块提供加载 API；**crate 专有资源**（desktop 图标/i18n/tauri.conf、pty shell 脚本）保留原位，但用 `env!("CARGO_MANIFEST_DIR")` 消除 `../../`。

### 1.3 非目标

- ❌ 不动运行时资源（`infra/seeds/` 目录 + 加载机制）
- ❌ 不动 `~/.octopus/` 路径相关代码（paths.rs）
- ❌ 不动 tauri.conf.json 的 `bundle.resources` 配置（打包时资源映射，与编译期内联无关）
- ❌ 不重构 `vault/src/generator/eff_wordlist.rs`（7786 行字面量 const，保留）
- ❌ 不引入 build.rs 代码生成
- ❌ 不加资源 checksum 校验（仓库内静态文件，git 保证完整性）

---

## 2. 目标结构

### 2.1 infra/resources/ 目录

```
crates/infra/resources/
├── sql/
│   └── schema.sql                              # 原 infra/src/db.sql
├── dicts/
│   ├── public_suffix_list.dat                  # 原 vault/data/
│   ├── words_common.txt                        # 原 ocr/assets/
│   ├── t2s.txt                                 # 原 asr-local/data/
│   ├── s2t.txt                                 # 原 asr-local/data/
│   ├── unigram.txt.gz                          # 原 asr-local/src/text/corrector_data/
│   └── NOTICE                                  # 原 asr-local/data/（版权跟随数据）
├── models/
│   └── vad/
│       └── silero_vad_v6.onnx                  # 原 asr-local/models/
└── prompts/
    └── hotword_mine.md                         # 原 desktop/resources/
```

### 2.2 infra::resources API

8 个 `pub fn`，每个内部 `include_bytes!`/`include_str!` 指向 `resources/` 子目录（相对本文件，无 `../../`）：

| API | 返回 | 资源路径 |
|---|---|---|
| `db_schema_sql()` | `&'static str` | resources/sql/schema.sql |
| `public_suffix_list()` | `&'static [u8]` | resources/dicts/public_suffix_list.dat |
| `ocr_words_common()` | `&'static str` | resources/dicts/words_common.txt |
| `hans_t2s()` | `&'static str` | resources/dicts/t2s.txt |
| `hans_s2t()` | `&'static str` | resources/dicts/s2t.txt |
| `corrector_unigram_gz()` | `&'static [u8]` | resources/dicts/unigram.txt.gz |
| `silero_vad_v6_onnx()` | `&'static [u8]` | resources/models/vad/silero_vad_v6.onnx |
| `hotword_mine_prompt()` | `&'static str` | resources/prompts/hotword_mine.md |

---

## 3. 迁移清单

### 3.1 物理文件搬迁（9 个 git mv）

| 旧路径 | 新路径 |
|---|---|
| `infra/src/db.sql` | `infra/resources/sql/schema.sql` |
| `vault/data/public_suffix_list.dat` | `infra/resources/dicts/public_suffix_list.dat` |
| `ocr/assets/words_common.txt` | `infra/resources/dicts/words_common.txt` |
| `asr-local/data/t2s.txt` | `infra/resources/dicts/t2s.txt` |
| `asr-local/data/s2t.txt` | `infra/resources/dicts/s2t.txt` |
| `asr-local/data/NOTICE` | `infra/resources/dicts/NOTICE` |
| `asr-local/src/text/corrector_data/unigram.txt.gz` | `infra/resources/dicts/unigram.txt.gz` |
| `asr-local/models/silero_vad_v6.onnx` | `infra/resources/models/vad/silero_vad_v6.onnx` |
| `desktop/resources/hotword_mine.md` | `infra/resources/prompts/hotword_mine.md` |

### 3.2 删除（1 个死文件）

| 文件 | 原因 |
|---|---|
| `vault/data/eff_large_wordlist.txt` (106KB) | 零引用——词表已字面量化在 `eff_wordlist.rs` |

### 3.3 消费点改动（14 处改为调 infra API）

| 消费点 | 现 | 改后 |
|---|---|---|
| infra/src/db/mod.rs:101 | `include_str!("../db.sql")` | `crate::resources::db_schema_sql()` |
| infra/src/seeds.rs:288 | `include_str!("db.sql")` | `crate::resources::db_schema_sql()` |
| infra/src/db/vault.rs:583 | `include_str!("../db.sql")` | `crate::resources::db_schema_sql()` |
| clipboard/src/store.rs:611 | `include_str!("../../infra/src/db.sql")` | `octopus_infra::resources::db_schema_sql()` |
| clipboard/src/cleanup.rs:123 | 同上 | 同上 |
| record/src/store.rs:227 | 同上 | 同上 |
| vault/src/matcher/psl.rs:26 | `include_bytes!("../../data/public_suffix_list.dat")` | `octopus_infra::resources::public_suffix_list()` |
| ocr/src/engine.rs:349 | `include_str!("../assets/words_common.txt")` | `octopus_infra::resources::ocr_words_common()` |
| asr-local/src/text/hans.rs:18 | `include_str!("../../data/t2s.txt")` | `octopus_infra::resources::hans_t2s()` |
| asr-local/src/text/hans.rs:20 | `include_str!("../../data/s2t.txt")` | `octopus_infra::resources::hans_s2t()` |
| asr-local/src/text/corrector.rs:11 | `include_bytes!("corrector_data/unigram.txt.gz")` | `octopus_infra::resources::corrector_unigram_gz()` |
| asr-local/src/audio/vad.rs:27 | `include_bytes!("../../models/silero_vad_v6.onnx")` | `octopus_infra::resources::silero_vad_v6_onnx()` |
| desktop/src/commands/hotword_commands.rs:188 | `include_str!("../../resources/hotword_mine.md")` | `octopus_infra::resources::hotword_mine_prompt()` |

### 3.4 env! 路径统一（5 处保留原位资源）

| 消费点 | 现 | 改后 |
|---|---|---|
| desktop/src/ui/i18n.rs:4 | `include_str!("../../frontend/src/locales/zh-CN.yaml")` | `concat!(env!("CARGO_MANIFEST_DIR"), "/frontend/src/locales/zh-CN.yaml")` |
| desktop/src/ui/i18n.rs:5 | `include_str!("../../frontend/src/locales/en.yaml")` | `concat!(env!("CARGO_MANIFEST_DIR"), "/frontend/src/locales/en.yaml")` |
| desktop/src/ui/tray.rs:363 | `include_bytes!("../../icons/icon.png")` | `concat!(env!("CARGO_MANIFEST_DIR"), "/icons/icon.png")` |
| desktop/src/ui/settings_window.rs:108 | `include_bytes!("../../icons/icon.png")` | `concat!(env!("CARGO_MANIFEST_DIR"), "/icons/icon.png")` |
| desktop/src/vault/autotype/macos.rs:293 | `include_str!("../../../tauri.conf.json")` | `concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json")` |

### 3.5 Cargo.toml 新增依赖

| crate | 新增 | 原因 |
|---|---|---|
| vault | `octopus-infra = { path = "../infra" }` | public_suffix_list 加载 |
| ocr | `octopus-infra = { path = "../infra" }` | words_common 加载 |

infra 不依赖 vault/ocr——单向，无环。

### 3.6 不动的

| 项 | 原因 |
|---|---|
| pty/src/shell_init.rs:17-19 | 已是相对路径（`scripts/zshenv.zsh`），无 `../../` |
| vault/src/generator/eff_wordlist.rs | 字面量 const，非 include |

---

## 4. 迁移步骤（7 步）

每步独立 commit + `cargo test` 全过。

| 步骤 | 内容 | 风险 |
|---|---|---|
| **1. 建 infra/resources/ 骨架** | 新建目录 + resources.rs + lib.rs 加 `pub mod resources;` | 极低 |
| **2. DB schema 搬迁** | git mv db.sql → resources/sql/schema.sql；加 `db_schema_sql()`；改 6 处消费点 | 低 |
| **3. vault 资源搬迁** | git mv psl.dat → resources/dicts/；删 eff_large_wordlist.txt；加 `public_suffix_list()`；改 psl.rs；vault/Cargo.toml 加 infra 依赖；改 psl.rs 注释 curl 路径 | 中 |
| **4. ocr 资源搬迁** | git mv words_common.txt → resources/dicts/；加 `ocr_words_common()`；改 engine.rs；ocr/Cargo.toml 加 infra 依赖 | 中 |
| **5. asr-local 资源搬迁** | git mv 4 文件（t2s/s2t/NOTICE/unigram/vad）；加 4 个 fn；改 hans.rs/corrector.rs/vad.rs | 中 |
| **6. hotword prompt + env! 统一** | git mv hotword_mine.md → resources/prompts/；加 `hotword_mine_prompt()`；改 hotword_commands.rs；desktop 5 处 `../../` 改 env! | 低 |
| **7. 清理 + 验证** | 删空目录；全量 cargo test；clippy；下游 desktop build | 低 |

---

## 5. 不变量（必须保持）

1. **零行为变更**——所有 crate 测试数与 baseline 一致（infra 183 / vault 258 / ocr 34 / asr-local 170 / clipboard 23 / record 50 / capx 55）
2. **`cargo build --workspace`**：0 error 0 warning
3. **`cargo clippy --workspace --all-targets`**：0 warning（≤ baseline 71）
4. **无残留引用**：`rg "infra/src/db\.sql\|vault/data/\|ocr/assets/\|asr-local/data/\|asr-local/models/\|corrector_data/"` 为空
5. **无空目录残留**：vault/data / ocr/assets / asr-local/data / asr-local/models / corrector_data / desktop/resources 全删

---

## 6. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| include 路径写错 | 中 | 编译失败（立刻发现） | 编译器即时报错；步骤 7 rg 二次防护 |
| 跨 crate 依赖循环 | 极低 | 编译失败 | infra 不依赖 vault/ocr——单向无环 |
| git mv 后 blame 断裂 | 确定 | 中 | git mv 保留 rename；`git log --follow` 可追溯 |
| Cargo.toml 依赖遗漏 | 中 | 编译失败 | 步骤 3/4 显式加；编译器报 unresolved import |
| tauri bundle resource 映射失效 | 低 | 打包缺资源 | 本次不动 tauri.conf.json bundle.resources（那是运行时资源，另一套） |

---

## 7. 成功标准（可客观验证）

1. `cargo build --workspace`：0 error 0 warning
2. `cargo test --workspace`：全过、各 crate 测试数不减
3. `cargo clippy --workspace --all-targets`：0 warning
4. `rg "infra/src/db\.sql\|vault/data/\|ocr/assets/\|asr-local/data/\|asr-local/models/\|corrector_data/"`：空
5. `ls crates/infra/resources/`：4 子目录（sql/dicts/models/prompts）
6. `git diff main..HEAD --stat`：9 rename + 1 delete + 代码改动

---

## 8. 文档同步

| 文档 | 更新 |
|---|---|
| `docs/architecture.md` §infra | 加 `resources` 模块描述 |
| 本 spec | 实施记录（见 §10） |

---

## 9. 关联文档

- 阶段 1-3 stitch 重构：`docs/superpowers/specs/2026-08-04-stitch-refactor-design.md`
- `infra/seeds.rs`：运行时 seed 加载（本次不动，仅对照机制差异）
- `infra/src/paths.rs`：运行时路径（本次不动）

---

## 10. 实施记录（2026-08-04）

### 10.1 完成情况

| 项 | spec 设计 | 实际实现 |
|---|---|---|
| infra/resources/ 目录 | sql/dicts/models/vad/prompts | ✅ 与 spec 一致 |
| 8 个 pub fn API | pub fn | ✅ 但改为 **`pub const fn`**（见 10.2） |
| 9 个 git mv | 见 §3.1 | ✅ 全部完成（含 NOTICE 跟随 t2s/s2t） |
| 删 eff_large_wordlist.txt | 死文件 | ✅ |
| 14 处消费点改 API | 见 §3.3 | ✅ |
| 5 处 env! 统一 | 见 §3.4 | ✅ |
| Cargo.toml 新增依赖 | vault/ocr | ❌ **vault/ocr 已依赖 infra，无需新增** |
| architecture.md 同步 | §infra 加 resources | ✅ |

### 10.2 关键偏差：API 用 `pub const fn`（非 `pub fn`）

spec §2.2 写的是 `pub fn`，实际实现改为 **`pub const fn`**。

**原因**：多个消费点是 `const` 上下文，不能用普通 fn 初始化：
- `infra/src/db/mod.rs:101`: `const INIT_SQL: &str = ...`
- `ocr/src/engine.rs:349`: `const WORDS_RAW: &str = ...`
- `asr-local/src/text/hans.rs:18,20`: `const T2S_DATA` / `const S2T_DATA`
- `asr-local/src/text/corrector.rs:11`: `const UNIGRAM_GZ`
- `asr-local/src/audio/vad.rs:27`: `const VAD_BYTES`
- `desktop/src/commands/hotword_commands.rs:188`: `const HOTWORD_MINE_PROMPT`
- `desktop/src/ui/settings_window.rs:108`: `const ICON_PNG`（env! 本身是 const，不受影响）

`include_str!`/`include_bytes!` 本身是 const 表达式，包装成 `const fn` 即可保留 const 语义——所有 `const X = fn_call()` 都能用。

### 10.3 Cargo.toml 依赖修正

spec §3.5 说 vault/ocr 新增 infra 依赖——实际检查发现**两者早已依赖 infra**（vault 用于 db/seeds，ocr 用于 paddle-ocr config），Cargo.toml 无需改动。

### 10.4 验证

- `cargo build --workspace`：0 error
- `cargo test --workspace`：全过，各 crate 测试数 = baseline（infra 183 / vault 258 / ocr 34 / asr-local 170 / clipboard 23 / record 50 / capx 55）
- clippy（涉及 crate）：0 新增 warning（各 crate warning 数 = main baseline）
- 残留引用扫描：空
- `../../` include 残留：空
- 空目录清理：6 个全删（vault/data / ocr/assets / asr-local/{data,models} / corrector_data / desktop/resources）

### 10.5 未在 architecture.md §各 crate 更新资源描述

spec §8 原计划"vault/ocr/asr-local/desktop 资源描述改为'经 infra::resources 加载'"——实际未改各 crate 章节，因为：
- architecture.md 各 crate 章节本就没有详细列每个 include 资源（只描述模块功能）
- 资源加载方式是实现细节，不属于架构描述
- 只在 §infra 加了 resources 模块总览即足够

### 10.6 architecture.md §各 crate 章节

未改——这些章节描述的是 crate 的功能职责，资源加载是实现细节，不属于架构层面。infra §resources 总览已足够让读者找到"工程内联资源在哪"。

- 阶段 1-3 stitch 重构：`docs/superpowers/specs/2026-08-04-stitch-refactor-design.md`
- `infra/seeds.rs`：运行时 seed 加载（本次不动，仅对照机制差异）
- `infra/src/paths.rs`：运行时路径（本次不动）
