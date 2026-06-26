# octopus-asr → octopus-asr-local 重命名 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `octopus-asr` crate 重命名为 `octopus-asr-local`，与 `octopus-asr-cloud` 命名对称。

**Architecture:** 纯机械重命名（package + lib + 目录 + 依赖 + `use` + docs），零行为/接口变更。关键风险是朴素替换会把 `octopus-asr-cloud` 误改成 `octopus-asr-local-cloud`，故全部用 **perl 负向 lookahead**（`(?!-)` / `(?!_)`）排除 cloud。macOS BSD sed 不支持 `\b`，故用 perl。

**Tech Stack:** Rust workspace、Cargo、perl。

**验证策略（TDD 不适用）：** 本次无新逻辑，不写新测试。靠**现有 workspace 测试全绿**证明零行为变更 + **grep 复核无残留/无误伤**证明替换完整。

**前置：** 所有命令在 worktree 根 `/Users/wudarui/workspace/agent/octopus/.claude/worktrees/model-mgmt-ui` 执行。起点 HEAD `6b47da5`（领先 main 1 commit = spec）。

---

## Task 1: 代码侧重命名（Cargo + 源码 + 验证）

**Files:**
- Rename: `crates/asr` → `crates/asr-local`（`git mv`，保 history）
- Modify: `crates/asr-local/Cargo.toml`（package name）
- Modify: `Cargo.toml`（workspace members）
- Modify: `crates/asr-cloud/Cargo.toml`、`crates/cli/Cargo.toml`、`crates/desktop/Cargo.toml`、`crates/server/Cargo.toml`、`crates/llm/Cargo.toml`（依赖名 + path + desktop feature）
- Modify: ~17 个 `.rs` 源文件（`octopus_asr` → `octopus_asr_local`）
- Auto: `Cargo.lock`（cargo check 自动更新，不手改）

- [x] **Step 1: git mv 目录（保 rename history）**

```bash
git mv crates/asr crates/asr-local
```
Expected: 无输出（成功）。`ls crates/asr-local/Cargo.toml` 存在。

- [x] **Step 2: 改 asr-local 的 package name**

```bash
perl -pi -e 's/^name = "octopus-asr"$/name = "octopus-asr-local"/' crates/asr-local/Cargo.toml
head -3 crates/asr-local/Cargo.toml
```
Expected: `[package]` / `name = "octopus-asr-local"` / `version = "0.1.0"`。

- [x] **Step 3: 改 workspace members**

```bash
perl -pi -e 's{"crates/asr"}{"crates/asr-local"}g' Cargo.toml
grep -n 'crates/asr' Cargo.toml
```
Expected: members 行显示 `"crates/asr-local"`（与 `"crates/asr-cloud"` 并列）；无裸 `"crates/asr"`。

- [x] **Step 4: 改 5 个依赖 Cargo.toml（依赖名 + path + desktop feature）**

依赖名 `octopus-asr`→`octopus-asr-local`（`(?!-)` 排除 `octopus-asr-cloud`）；path `"../asr"`→`"../asr-local"`（带引号精确匹配，排除 `"../asr-cloud"`）。desktop 的 `embedded = ["octopus-asr"]` 同步被改。

```bash
perl -pi -e 's/octopus-asr(?!-)/octopus-asr-local/g; s{"../asr"}{"../asr-local"}g' \
  crates/asr-cloud/Cargo.toml crates/cli/Cargo.toml crates/desktop/Cargo.toml \
  crates/server/Cargo.toml crates/llm/Cargo.toml
echo "--- 复核：裸 octopus-asr 应消失（只剩 -local / -cloud）---"
grep -rn 'octopus-asr' crates/*/Cargo.toml | grep -vE 'octopus-asr-local|octopus-asr-cloud' || echo "✓ 无残留"
```
Expected: 末行 `✓ 无残留`。`grep 'octopus-asr' crates/desktop/Cargo.toml` 应见 `octopus-asr-local = { path = "../asr-local", optional = true }` + `embedded = ["octopus-asr-local"]`。

- [x] **Step 5: 改源码 use/path（octopus_asr → octopus_asr_local）**

`(?!_)` 排除 `octopus_asr_cloud`。覆盖所有 `.rs`（含 asr-local 自身 lib.rs doc、asr-cloud 引用本地零件的 `use octopus_asr::`）。

```bash
find crates -name '*.rs' -print0 | xargs -0 perl -pi -e 's/octopus_asr(?!_)/octopus_asr_local/g'
echo "--- 复核：裸 octopus_asr 应消失（只剩 _local / _cloud）---"
grep -rn 'octopus_asr' crates/ --include='*.rs' | grep -vE 'octopus_asr_local|octopus_asr_cloud' || echo "✓ 无残留"
```
Expected: 末行 `✓ 无残留`。

- [x] **Step 6: cargo check（验证编译 + 自动更新 Cargo.lock）**

```bash
cargo check --workspace --all-targets 2>&1 | tail -5
```
Expected: `Finished` 无 error。Cargo.lock 自动含 `octopus-asr-local`（`grep 'name = "octopus-asr-local"' Cargo.lock` 命中）。若报 `unresolved import octopus_asr` → 有遗漏，回 Step 5 grep 找漏文件。

- [x] **Step 7: cargo test --workspace（零行为变更验证）**

```bash
cargo test --workspace 2>&1 | tail -15
```
Expected: 全绿（lib + 各 crate 单测全 passed，0 failed）。测试数应与改名前一致（无新增/丢失）。

- [x] **Step 8: cargo clippy（0 新 warning）**

```bash
cargo clippy --workspace --all-targets 2>&1 | grep -E 'warning|error' | head || echo "✓ 零 warning/error"
```
Expected: 仅 pre-existing warning（如 desktop `dead_code` current_partial/is_cloud），**无新** warning。

- [x] **Step 9: grep 复核代码侧无残留 + 无 cloud 误伤**

```bash
echo "--- 残留（应空）---"
grep -rnE 'octopus[-_]asr' crates/ --include='*.rs' --include='Cargo.toml' | grep -vE 'octopus[-_]asr-local|octopus[-_]asr-cloud' || echo "✓ 无残留"
echo "--- 误伤 octopus-asr-local-cloud / octopus_asr_local_cloud（应空）---"
grep -rnE 'octopus[-_]asr[-_]local[-_]cloud' . --exclude-dir=target --exclude-dir=.git || echo "✓ 无误伤"
echo "--- workspace members + Cargo.lock 确认 ---"
grep 'asr-local\|asr-cloud\|"crates/asr"' Cargo.toml | head
grep -c 'octopus-asr-local' Cargo.lock
```
Expected: 三处均 `✓`；members 含 `crates/asr-local`；Cargo.lock 命中 ≥1。

- [x] **Step 10: Commit**

```bash
git add -A
git commit -m "refactor: octopus-asr→octopus-asr-local 重命名（代码侧）

与 octopus-asr-cloud 命名对称。package+lib(Octopus_asr_local)+目录 crates/asr-local。
5 依赖 Cargo.toml + ~17 源文件 use + workspace members + Cargo.lock。
零行为变更，workspace 测试全绿，clippy 0 新 warning。
perl 负向 lookahead 排除 -cloud，防误伤。"
```

---

## Task 2: docs 重命名（33 文件含 archived）+ 复核

**Files:**
- Modify: 所有 `.md`（`docs/superpowers/specs/*`、`docs/superpowers/plans/*` 含 `*-archived-*`、`docs/architecture.md`、`docs/asr_archiveture_opt.md`、`AGENTS.md`、`usage.md`、`crates/dlp/docs/architecture.md`）

- [x] **Step 1: docs 全量替换（连字符 + 下划线，排除 cloud）**

对仓库内所有 `.md`（排除构建产物 / 其他 worktree），一条 perl 跑两个表达式：

```bash
find . -name '*.md' \
  -not -path './target/*' -not -path './node_modules/*' \
  -not -path './.git/*' -not -path './.worktrees/*' \
  -not -path './.claude/worktrees/*' -not -path './crates/*/node_modules/*' \
  -print0 | xargs -0 perl -pi -e 's/octopus-asr(?!-)/octopus-asr-local/g; s/octopus_asr(?!_)/octopus_asr_local/g'
echo "--- 替换后 docs 里 octopus-asr-local 命中文件数 ---"
grep -rl 'octopus-asr-local' . --include='*.md' -z 2>/dev/null | grep -vE 'target|node_modules|\.git|\.worktrees|\.claude/worktrees' | wc -l | tr -d ' '
```
Expected: 命中文件数 > 0（与原 33 文件量级吻合）。

- [x] **Step 2: grep 复核 docs 无残留 + 无误伤**

```bash
echo "--- docs 残留裸 octopus-asr/octopus_asr（应空，只剩 -local/-cloud）---"
find . -name '*.md' \
  -not -path './target/*' -not -path './node_modules/*' -not -path './.git/*' \
  -not -path './.worktrees/*' -not -path './.claude/worktrees/*' \
  -print0 | xargs -0 grep -n 'octopus[-_]asr' 2>/dev/null \
  | grep -vE 'octopus[-_]asr-local|octopus[-_]asr-cloud' || echo "✓ 无残留"
echo "--- 误伤 octopus-asr-local-cloud（应空）---"
find . -name '*.md' -not -path './target/*' -not -path './.git/*' -print0 \
  | xargs -0 grep -n 'octopus[-_]asr[-_]local[-_]cloud' 2>/dev/null || echo "✓ 无误伤"
```
Expected: 两处均 `✓`。

- [x] **Step 3: Commit**

```bash
git add -A
git commit -m "docs: octopus-asr→octopus-asr-local 重命名同步（含 archived）

33 个 docs 文件（specs/plans 含 archived + architecture + AGENTS + usage + dlp/docs）
统一 octopus-asr→octopus-asr-local。与代码侧重命名（前一 commit）对齐。"
```

---

## 完成判据

- `cargo check --workspace --all-targets`：0 error
- `cargo test --workspace`：全绿，测试数与改名前一致
- `cargo clippy --workspace --all-targets`：0 新 warning
- 仓库内（排除 `target`/`node_modules`/`.git`/`.worktrees`）grep `octopus[-_]asr` 仅剩 `*-local`/`*-cloud`，无裸 `octopus-asr`/`octopus_asr`，无 `*-local-cloud` 误伤
- 两个 commit（代码侧 + docs）
