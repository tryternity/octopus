# 润色提示词 3 模板 + [] edited 标记机制 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** 3 个新润色模板（faithful/user-intent/app-casual）+ `[]` edited 内联标记替代 region 标记法 + 去掉 INCREMENTAL_RULE。

**Architecture:** prompt.rs 的 `regions_prompt` 改为 `[]` 内联拼接（edited 段 `[]` 包裹，全文连贯）；`INCREMENTAL_RULE` → `EDITED_MARKER_RULE`（代码层拼接，用户不可见）；3 个新模板 seed 文件含 few-shot；旧模板移到 `history/` 子目录。

**Tech Stack:** Rust + rusqlite + markdown seed files

## Global Constraints

- `[]` 标记规则在 system prompt 层（代码拼接），用户模板不含此规则
- 无 edited 段时行为等价全量润色（body 无 `[]`）
- ITN 数字归一化 + hans 简繁归一化在润色前执行（代码层，不受 prompt 影响）
- 旧模板移到 `history/` 子目录（保留对比，非删除）
- `build_system_prompt(content) = content + EDITED_MARKER_RULE`

**Spec:** `docs/superpowers/specs/2026-08-01-polish-prompt-templates-design.md`

---

## Task 1: prompt.rs — [] 标记机制 + EDITED_MARKER_RULE

**Files:**
- Modify: `crates/llm/src/prompt.rs`

**Interfaces:**
- Produces: `EDITED_MARKER_RULE`（替代 INCREMENTAL_RULE）；`regions_prompt` / `user_prompt` 改 `[]` 拼接

- [x] **Step 1: INCREMENTAL_RULE → EDITED_MARKER_RULE**

`crates/llm/src/prompt.rs`：
- 删 `const INCREMENTAL_RULE`（line 12）+ `const CONFIRMED_MARKER`（line 8）
- 加：
```rust
/// [] edited 标记规则（代码层拼接到 system prompt 末尾，用户不可见）。
/// 替代旧 INCREMENTAL_RULE：从「原样保留」改为「信任+遵循语境」。
const EDITED_MARKER_RULE: &str = "文本中 [方括号] 标记的词语是用户手动修正过的，请信任这些用词，并在润色全文时以其为语境参考。输出时去掉方括号标记，仅输出纯文本。";
```
- `build_system_prompt` 改用 `EDITED_MARKER_RULE`：
```rust
pub(crate) fn build_system_prompt(content: &str) -> String {
    format!("{}\n{}", content.trim_end(), EDITED_MARKER_RULE)
}
```

- [x] **Step 2: regions_prompt 改 [] 内联拼接**

```rust
pub(crate) fn regions_prompt(regions: &[crate::PolishRegion]) -> String {
    let mut body = String::new();
    for r in regions {
        if r.preserve {
            body.push_str(&format!("[{}]", r.text));
        } else {
            body.push_str(&r.text);
        }
    }
    format!("请润色以下语音识别文本：\n{}", body)
}
```
注意：不再有「无 preserve 段走全量润色分支」——统一拼接（无 preserve 时 body 无 `[]`，等价全量润色）。

- [x] **Step 3: user_prompt(preserved, to_polish) 改 [] 标记**

```rust
pub(crate) fn user_prompt(preserved: Option<&str>, to_polish: &str) -> String {
    match preserved {
        None => format!("请润色以下语音识别文本：\n{}", to_polish),
        Some(confirmed) => format!(
            "请润色以下语音识别文本：\n[{}]{}",
            confirmed, to_polish
        ),
    }
}
```
edited 部分用 `[]` 包裹拼到 raw 前，统一一句指令。

- [x] **Step 4: 更新测试**

prompt.rs 的 `#[cfg(test)] mod tests`：
- `user_prompt_without_preserved_is_plain`：不变（无 preserve 仍 plain）
- `user_prompt_with_preserved_marks_boundary`：改为断言含 `[已确认文本]`（不再是「已确认部分」/「原样保留」）
- `build_system_prompt_appends_incremental_rule`：改为断言含 `EDITED_MARKER_RULE` 内容（`方括号` / `信任`）
- `regions_prompt_no_preserve_is_plain`：不变（无 preserve 仍 plain）
- `regions_prompt_marks_preserved_regions`：改为断言含 `[已确认]`（不再是「原样保留」/「待润色」）

- [x] **Step 5: build + test 验证**

Run: `cargo build -p octopus-llm 2>&1 | grep -E "^error|^warning"`
Run: `cargo test -p octopus-llm 2>&1 | tail -3`
Expected: 0 error 0 warning，测试全过

- [x] **Step 6: Commit**

```bash
git add crates/llm/src/prompt.rs
git commit -m "refactor(prompt): [] edited 标记替代 region 标记法 + EDITED_MARKER_RULE"
```

---

## Task 2: 3 个新模板 seed 文件 + 旧模板移 history/

**Files:**
- Create: `crates/infra/seeds/prompts/faithful.md`
- Create: `crates/infra/seeds/prompts/user-intent.md`
- Create: `crates/infra/seeds/prompts/app-casual.md`
- Move: `crates/infra/seeds/prompts/default-polish.md` → `history/`
- Move: `crates/infra/seeds/prompts/advanced-polish.md` → `history/`
- Move: `crates/infra/seeds/prompts/sayit-*.md` → `history/`（4 个）

- [x] **Step 1: 创建 history/ 子目录 + 移动旧模板**

```bash
mkdir -p crates/infra/seeds/prompts/history
mv crates/infra/seeds/prompts/default-polish.md crates/infra/seeds/prompts/history/
mv crates/infra/seeds/prompts/advanced-polish.md crates/infra/seeds/prompts/history/
mv crates/infra/seeds/prompts/sayit-casual.md crates/infra/seeds/prompts/history/
mv crates/infra/seeds/prompts/sayit-faithful.md crates/infra/seeds/prompts/history/
mv crates/infra/seeds/prompts/sayit-intent.md crates/infra/seeds/prompts/history/
mv crates/infra/seeds/prompts/sayit-zh2en.md crates/infra/seeds/prompts/history/
```

- [x] **Step 2: 创建 faithful.md**

`crates/infra/seeds/prompts/faithful.md`——忠实校对模板（参考 spec §faithful 核心规则 + few-shot）。内容含：
- Role + 9 条规则（绝对防御/提纯去噪/纠错/ASR 异常修复/数字格式/中英空格/标点/静默/禁止改写）
- 3 个 few-shot 示例（含 `[]` edited 标记，演示输入→输出）

- [x] **Step 3: 创建 user-intent.md**

`crates/infra/seeds/prompts/user-intent.md`——意图整理模板。内容含：
- Role + 8 条规则（绝对防御/清除冗余/自我纠正/纠错/ASR 异常修复/标点+空格/主动结构化/静默）
- 2 个 few-shot 示例（含结构化列表输出 + `[]` 标记）

- [x] **Step 4: 创建 app-casual.md**

`crates/infra/seeds/prompts/app-casual.md`——口语化整理模板。内容含：
- Role + 7 条规则（绝对防御/去噪/顺句/纠错/ASR 异常修复/聊天标点/静默）
- 3 个 few-shot 示例（含 `[]` 标记，口语风输出）

- [x] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(prompts): 3 新模板（faithful/user-intent/app-casual）+ 旧模板移 history/"
```

---

## Task 3: seeds.rs seed 列表更新 + db.sql active_polish_prompt

**Files:**
- Modify: `crates/infra/src/seeds.rs`（`load_prompt_seeds` 的 seeds 数组）
- Modify: `crates/infra/src/db.sql`（active_polish_prompt 默认值）

- [x] **Step 1: seeds.rs — seeds 数组改为 3 个新模板**

`crates/infra/src/seeds.rs` `load_prompt_seeds`（line 67-71）：
```rust
let seeds = [
    (1i64, "faithful.md", "润色-忠实校对", "忠实校对",
     "只纠错不改意，保留原始句式。ASR 异常修复强（系统内置）"),
    (2i64, "user-intent.md", "润色-意图整理", "意图整理",
     "清洗噪声+结构化，多要点自动转列表（系统内置）"),
    (3i64, "app-casual.md", "润色-口语化", "口语化整理",
     "保留口语味，聊天标点，适合即时通讯（系统内置）"),
];
```

- [x] **Step 2: db.sql — active_polish_prompt 默认值**

`crates/infra/src/db.sql` 找 `active_polish_prompt` seed 行（约 line 400）：
`'1'` 保持（id=1 = faithful，新默认）。description 更新为 `'激活的润色 prompt id（prompts 表 id 字段，默认 1=忠实校对）'`。

- [x] **Step 3: build + test 验证**

Run: `cargo build -p octopus-infra 2>&1 | grep -E "^error|^warning"`
Run: `cargo test -p octopus-infra 2>&1 | tail -3`
Expected: 0 error 0 warning

- [x] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(seeds): prompt seed 列表改 3 新模板 + active_polish_prompt 默认 faithful"
```

---

## Task 4: 全量验证 + 文档同步

**Files:**
- Modify: `docs/architecture.md`（润色 prompt 段更新）

- [x] **Step 1: 全量验证**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | grep -E "^error|^warning|Finished"
cargo test -p octopus-desktop --features embedded 2>&1 | tail -3
```
Expected: build 0 error 0 warning，test 全过

- [x] **Step 2: architecture.md 更新**

找到润色/prompt 相关段，更新：
- 3 模板（faithful/user-intent/app-casual）替代旧 6 个
- [] edited 标记机制（替代 region 标记法 + INCREMENTAL_RULE）
- EDITED_MARKER_RULE 代码层拼接

- [x] **Step 3: spec 加实现状态段**

`docs/superpowers/specs/2026-08-01-polish-prompt-templates-design.md` 末尾加实现状态。

- [x] **Step 4: Commit**

```bash
git add -A
git commit -m "docs(sync): 润色提示词 3 模板文档同步 + spec 实现状态"
```

---

## Self-Review

**Spec coverage:**
- ✅ [] 标记机制（Task 1 prompt.rs）
- ✅ EDITED_MARKER_RULE 替代 INCREMENTAL_RULE（Task 1）
- ✅ 3 新模板 + few-shot（Task 2）
- ✅ 旧模板移 history/（Task 2 Step 1）
- ✅ seed 列表更新（Task 3）
- ✅ 文档同步（Task 4）

**Type consistency:** `PolishRegion { preserve: bool, text: String }` 不变；`regions_prompt` / `user_prompt` 签名不变（仅内部构造变）；`EDITED_MARKER_RULE` 替代 `INCREMENTAL_RULE` 在 `build_system_prompt` 一致。
