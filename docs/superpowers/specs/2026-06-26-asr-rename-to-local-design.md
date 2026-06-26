# octopus-asr → octopus-asr-local 重命名设计

> **目标**：把 `octopus-asr` crate 改名为 `octopus-asr-local`，与 `octopus-asr-cloud` 命名对称。
> **性质**：纯机械重命名，**零行为变更、零接口变更**。
> **范围决策（用户 2026-06-26）**：① 彻底对称（package + lib + 目录全改，lib → `octopus_asr_local`，11+ 文件 `use` 一并改）；② docs 全改含 archived（33 文件）。

## 1. 背景与现状

`crates/asr`（package `octopus-asr`，lib `octopus_asr`）是本地 ASR 零件库 + 无端 helper（`transcribe_batch` / `StreamingRunner` / `StreamingEngine`/`OfflineEngine`/`AudioSource` trait / `TranscriptEvent` / `PipelineConfig`）。与云端 `octopus-asr-cloud`（`crates/asr-cloud`，lib `octopus_asr_cloud`）职责对称（本地 vs 云端），但命名不对称：本地缺 `-local` 后缀。

被 `asr-cloud` / `cli` / `desktop` / `server` / `llm` 依赖（`asr-cloud` 也依赖它拿本地零件 trait）。三端（cli/desktop/server）已统一走 asr helper（阶段1/2/3 收官，2026-06-26）。

## 2. 改名映射

| 项 | 现 | 新 |
|---|---|---|
| package name | `octopus-asr` | `octopus-asr-local` |
| lib name（派生） | `octopus_asr` | `octopus_asr_local` |
| 目录 | `crates/asr` | `crates/asr-local` |

与 `octopus-asr-cloud`（package `octopus-asr-cloud` / lib `octopus_asr_cloud` / `crates/asr-cloud`）完全对称。**不**加显式 `[lib] name`——lib name 由 package 派生，与 asr-cloud 一致。

## 3. 影响清单

### 3.1 代码 / Cargo（必须改，编译器验证）

- `crates/asr-local/Cargo.toml`：`name = "octopus-asr"` → `"octopus-asr-local"`
- workspace `Cargo.toml`：members `"crates/asr"` → `"crates/asr-local"`
- **5 个依赖 Cargo.toml**（依赖名 + `path = "../asr"` → `"../asr-local"`）：`asr-cloud` / `cli` / `desktop`（含 feature `embedded = ["octopus-asr"]` → `["octopus-asr-local"]`） / `server` / `llm`
- **~17 源文件** `octopus_asr` → `octopus_asr_local`：desktop(13) / asr-cloud(2-3) / server(2) / cli(2) / llm(1) / asr-local/lib.rs(1)
- `Cargo.lock`：自动重新生成（**不手改**）

### 3.2 无向后兼容风险

cli/main.rs 的 46 处引用**全为下划线 `octopus_asr` 代码路径**（`use` / `octopus_asr::path`），**0 处连字符、0 处 `"octopus-asr"` 字符串字面量**。即没有用户可见的 `octopus-asr` 配置 key / engine source 名 / CLI 参数——改名不影响任何 config/脚本/外部调用。

desktop feature 名 `embedded` / `cloud` 本身不变（仅 `embedded` 启用项从 `octopus-asr` 改为 `octopus-asr-local`），`--features embedded cloud` 用法不受影响。

### 3.3 docs（全改含 archived，33 文件）

sed 全局替换，含 `architecture.md` / `AGENTS.md` / `usage.md` / `docs/superpowers/specs/*` / `docs/superpowers/plans/*`（含 `*-archived-*`）/ `crates/dlp/docs/architecture.md` / `docs/asr_archiveture_opt.md`。理由：代码里将无 `octopus-asr`，archived 保留旧名会误导 vibecoding；archived 保留的是设计决策/动机，非当时的 crate 名。

## 4. 执行顺序

1. `git mv crates/asr crates/asr-local`（保 rename history）
2. 改 `asr-local/Cargo.toml` `name` + workspace `members`
3. 改 5 依赖 Cargo.toml（name + path）
4. 替换源码 `octopus_asr` → `octopus_asr_local`
5. `cargo check --workspace --all-targets`（验证编译 + 自动更新 Cargo.lock）
6. `cargo test --workspace` + clippy（0 新 warning）
7. 替换 docs（33 文件）
8. commit

## 5. 关键执行细节：替换必须排除 `-cloud`

朴素 `s/octopus-asr/octopus-asr-local/g` 会把 `octopus-asr-cloud` 误改成 `octopus-asr-local-cloud`（下划线同理：`octopus_asr_cloud` → `octopus_asr_local_cloud`）。

**用 perl 负向 lookahead 精确排除**：
- 连字符：`perl -pi -e 's/octopus-asr(?!-)/octopus-asr-local/g'`（后不跟 `-`，排除 `-cloud`；已改的 `-local` 后跟 `-` 也不重复匹配）
- 下划线：`perl -pi -e 's/octopus_asr(?!_)/octopus_asr_local/g'`（后不跟 `_`，排除 `_cloud`；`::`/空白/引号前的 `octopus_asr` 全改）

macOS BSD sed 不支持 `\b`，故用 perl（负向 lookahead 可靠）。plan 给精确命令 + 每步 grep 复核。

## 6. 验证

- `cargo check --workspace --all-targets`：0 error
- `cargo test --workspace`：全绿（lib + 各 crate 单测）
- `cargo clippy --workspace --all-targets`：0 新 warning
- grep 复核：仓库内（排除 `target`/`node_modules`/`.git`）无残留 `octopus-asr`/`octopus_asr`（非 cloud）—— `grep -rnE 'octopus[-_]asr(?![-_])'` 应只剩 `*-cloud`/`*_cloud`

## 7. 风险

- **低**：纯机械替换，编译器抓所有代码遗漏；docs 遗漏由 grep 复核。
- perl 负向 lookahead 排除 cloud——执行后 grep 复核确认无 `*_local_cloud` 误伤。
- git history：`git mv` 保目录 rename；源文件内容仅 `use` 行变，git 识别为 modify（非 rename）。
- `asr-cloud` 依赖 `asr-local`（云端依赖本地零件 trait），语义不变。
