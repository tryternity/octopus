# ActionBar `/` 斜杠命令 v2 增强 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`).

**Goal:** 基于 v1（已落地）增强 slash 命令交互：IME 兼容（`、`）+ 候选池扩大（所有菜单项 + 标题匹配）+ Tab 补全 + 菜单标题字符约束。

**Architecture:** 后端 `search_slash_commands` 扩展候选池 + 双维匹配（命令名/标题）+ `、` 兼容。前端加 Tab 补全交互（slash 模式 Tab 键补全标题+空格 + 选中锁定）+ 菜单标题校验。

**Tech Stack:** Rust（octopus-search），TypeScript/React，Vitest。

**Spec:** `docs/superpowers/specs/2026-07-30-actionbar-slash-command-design.md` §「交互增强 v2」

## Global Constraints

- **IME 兼容**：query 开头 `/` 或 `、` 都触发 slash 模式（只在开头，后续字符不兼容）。
- **候选池**：`is_enabled && action_type != "submenu"` 的所有菜单项（不限 trigger_keyword）。
- **匹配维度**：trigger_keyword（若有）+ title 双维 fuzzy，取最高分；命令名精确匹配优先。
- **候选显示**：菜单项标题（携带 id），不管匹配源。
- **Tab 补全**：slash 模式下 Tab 键 → 补全选中项标题 + 空格；锁定选中（selectedItemId）；输参数期间不重新搜索。
- **执行**：用锁定的菜单 id + 输入框空格后文本作参数。
- **菜单标题约束**：中文字母数字 `-_`，禁空格/特殊字符。
- **工作目录**：`/Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730`。

---

## Task 1: 后端候选池扩大 + IME 兼容 + 标题匹配（TDD）

**Files:**
- Modify: `crates/search/src/providers/menu.rs`（`search_slash_commands` 扩展）
- Test: `menu.rs` 内联 `slash_command_tests`

**说明**：v1 的 `search_slash_commands` 只匹配 trigger_keyword。v2 扩大候选池到所有菜单项，加标题匹配维度 + `、` 兼容。

- [ ] **Step 1: 更新测试反映 v2 行为**

更新 `slash_command_tests`（已有 12 测试，v1 + final fix）。关键变化：
- 候选池：无 trigger_keyword 的菜单项也进候选（按标题匹配）
- 匹配维度：trigger_keyword + title 双维
- `、` 开头兼容

新增/修改测试：
```rust
#[test]
fn slash_ideographic_comma_prefix_also_works() {
    // 、（顿号，U+3001）开头等同 / 开头
    let rows = vec![item(1, "", "google", "url")];
    let results = search_slash_commands("、google hello", &rows);
    assert_eq!(results.len(), 1);
    let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
    assert_eq!(data["params"], "hello");
}

#[test]
fn slash_title_match_for_item_without_trigger_keyword() {
    // 无 trigger_keyword 的项，按标题匹配
    let rows = vec![item_with_title(2, "百度搜索", "", "url")]; // trigger_keyword 空
    let results = search_slash_commands("/百度", &rows);
    assert_eq!(results.len(), 1);
    // 候选显示标题
    assert_eq!(results[0].title, "百度搜索");
}

#[test]
fn slash_command_name_outranks_title_match() {
    // trigger_keyword 精确匹配优先于标题 fuzzy
    let rows = vec![
        item_with_title(1, "Google", "google", "url"),         // trigger=google
        item_with_title(2, "Google Scholar", "", "url"),        // 标题含 google，无 trigger
    ];
    let results = search_slash_commands("/google", &rows);
    assert_eq!(results[0].action_data, /* id=1 项 */); // trigger 命中的排前
}

#[test]
fn slash_all_items_are_candidates() {
    // 所有 is_enabled 非 submenu 项都进候选池（即使无 trigger_keyword）
    let rows = vec![
        item_with_title(1, "Google", "google", "url"),
        item_with_title(2, "翻译", "", "ai"),       // 无 trigger
        item_with_title(3, "Agent菜单", "", "submenu"), // submenu 排除
    ];
    let results = search_slash_commands("/", &rows);
    assert_eq!(results.len(), 2); // google + 翻译，submenu 排除
}
```

> `item_with_title` 是新 helper（支持自定义 title），或扩展现有 `item` 签名加 title 参数。现有 `item(id, over, trigger, action_type)` 的 `over` 参数没用上，改成 `item(id, title, trigger, action_type)`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p octopus-search --lib slash_command_tests 2>&1 | tail -10`
Expected: FAIL（新测试不通过）。

- [ ] **Step 3: 实现 v2 search_slash_commands**

重写 `search_slash_commands`（menu.rs）。关键改动：
```rust
fn search_slash_commands(query: &str, rows: &[octopus_infra::db::ActionBarItem]) -> Vec<SearchResult> {
    // IME 兼容：开头 / 或 、（U+3001 顿号）
    let rest = query.strip_prefix('/').or_else(|| query.strip_prefix('、'));
    let rest = match rest { Some(r) => r, None => return vec![] };

    // 候选池：is_enabled && 非 submenu（不限 trigger_keyword）
    let candidates: Vec<_> = rows.iter()
        .filter(|r| r.is_enabled && r.action_type != "submenu")
        .collect();

    // 仅 / 或 、 → 返回全部候选
    if rest.is_empty() {
        return candidates.iter().map(|r| slash_result(r, "", SLASH_BASE_SCORE)).collect();
    }

    let (cmd, params) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    };
    let cmd_lower = cmd.to_lowercase();

    // 双维匹配：trigger_keyword（若有）+ title，取最高分
    let mut scored: Vec<(i32, &ActionBarItem)> = candidates.iter()
        .filter_map(|r| {
            // 命令名匹配（trigger_keyword 非空时）
            let kw_score = if !r.trigger_keyword.is_empty() {
                match_score(&cmd_lower, &r.trigger_keyword.to_lowercase())
            } else { None };
            // 标题匹配
            let title_score = match_score(&cmd_lower, &r.title.to_lowercase());
            // 取最高，命令名匹配加 boost（优先）
            let score = match (kw_score, title_score) {
                (Some(k), _) => Some(k + SLASH_KW_BOOST),  // 命令名命中加分
                (None, Some(t)) => Some(t),
                (None, None) => None,
            };
            score.map(|s| (s, r))
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(10)
        .map(|(s, r)| slash_result(r, params, SLASH_BASE_SCORE + s))
        .collect()
}
```

新增常量：`SLASH_KW_BOOST: i32 = 1000`（命令名匹配比标题匹配略优先）。

`slash_result` 改为接收 `&ActionBarItem`（用 title 显示）+ params + score：
```rust
fn slash_result(r: &ActionBarItem, params: &str, score: i32) -> SearchResult {
    SearchResult {
        source: "slash".into(),
        title: r.title.clone(),  // 显示菜单标题（非 /trigger_keyword）
        subtitle: if r.trigger_keyword.is_empty() { r.action_type.clone() }
                  else { format!("/{}", r.trigger_keyword) },
        icon: None,
        action_type: r.action_type.clone(),
        action_data: serde_json::json!({
            "id": r.id,
            "title": r.title,      // 供 Tab 补全用
            "cmd": r.trigger_keyword,
            "params": params,
            "action_type": r.action_type,
            "action_data": r.action_data,
        }).to_string(),
        score,
    }
}
```

> action_data 加 `title` 字段供前端 Tab 补全。subtitle 显示 `/cmd`（有命令名时）或 action_type（无命令名时），让用户知道匹配源。

- [ ] **Step 4: 更新既有测试适配新行为**

v1 的测试可能断言 `title == "/google"`，v2 改为显示菜单标题。更新断言。如 `slash_with_cmd_and_params_matches` 的 `results[0].title` 从 `/google` 改为 `Test 1`（item 的 title）。

- [ ] **Step 5: 跑测试 + 编译**

Run: `cargo test -p octopus-search --lib 2>&1 | tail -5 && cargo build -p octopus-search 2>&1 | tail -3`
Expected: 全过。

- [ ] **Step 6: Commit**

```bash
git add crates/search/src/providers/menu.rs
git commit -m "feat(actionbar): slash v2 候选池扩大 + 标题匹配 + IME 兼容

候选池扩大到所有非 submenu 菜单项（不限 trigger_keyword）；双维 fuzzy
（命令名+标题，命令名加 boost 优先）；、开头兼容（IME）；候选显示菜单标题
携带 id。"
```

---

## Task 2: 前端 Tab 补全 + 选中锁定

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/keyNavigation.ts`（slash 模式 Tab → slash-complete action）
- Modify: `crates/desktop/frontend/src/pages/ActionBar/useActionBarKeydown.ts`（执行 slash-complete）
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`（选中锁定状态 + query 拦截 + 执行用 id）

**说明**：核心交互增强。Tab 补全 + 锁定选中是新的交互逻辑。

- [ ] **Step 1: keyNavigation 加 slash-complete action**

`KeyAction` 加 `{ type: "slash-complete" }`。`decideKeyAction` 里 search 模式的 Tab 处理加判断：activeTab==="slash" 时返回 slash-complete 而非 search-tab。

读 keyNavigation.ts search 模式 Tab 分支（约 :131）：
```rust
const dir = moveDirection(e.key, e.shiftKey, ARROW_AS_TAB);
if (dir !== null) {
    // slash 模式 Tab → 补全（不切 tab）
    if (ctx.activeTab === "slash") return { type: "slash-complete" };
    return { type: "search-tab", dir: dir ? 1 : -1 };
}
```

记得更新 `useActionBarKeydown.exhaustive.test-d.ts` 的 HandledActionTypes 加 `slash-complete`。

- [ ] **Step 2: index.tsx 加选中锁定状态**

加 state：
```ts
const [slashLockedItemId, setSlashLockedItemId] = useState<number | null>(null);
const slashLockedItemIdRef = useRef<number | null>(null);
useEffect(() => { slashLockedItemIdRef.current = slashLockedItemId; }, [slashLockedItemId]);
```

query effect 改：slash 模式 + 已锁定时，query 变化不触发 search_stream（保持候选）。检测用户删除补全标题 → 解锁。

- [ ] **Step 3: useActionBarKeydown 执行 slash-complete**

```ts
case "slash-complete": {
    const results = p.filteredResultsRef.current;
    const selected = results[p.searchSelectedIdxRef.current] ?? results[0];
    if (!selected) return;
    const data = JSON.parse(selected.actionData || "{}");
    const title = data.title as string;
    // 补全：输入框变为 /标题 + 空格
    p.setQuery("/" + title + " ");
    // 锁定选中
    p.setSlashLockedItemId(data.id as number);
    // focus 输入框末尾
    p.inputRef.current?.focus();
    return;
}
```

`ActionBarKeydownParams` 加 `setSlashLockedItemId`。

- [ ] **Step 4: 执行改用锁定 id**

`executeSearchResult` 的 slash 分流：优先用 `slashLockedItemIdRef.current`，否则用选中候选的 id。参数从 query 空格后解析。

- [ ] **Step 5: query 拦截（锁定时不重新搜索）**

slash 模式 + slashLockedItemId 非空时，query effect 跳过 search_stream 调用。检测解锁：query 不再以 `/锁定标题` 开头 → setSlashLockedItemId(null)。

- [ ] **Step 6: tsc + vite build + vitest**

Run: `cd crates/desktop/frontend && npx tsc -b && npx vitest run && npx vite build`
Expected: 0 error，测试全过。

- [ ] **Step 7: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/
git commit -m "feat(actionbar): slash v2 Tab 补全 + 选中锁定

slash 模式 Tab 补全菜单标题+空格；锁定选中（selectedItemId），输参数期间
不重新搜索；执行用锁定 id + query 参数。"
```

---

## Task 3: 菜单标题字符约束

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ActionBar/EditForm.tsx`（标题校验 UI）
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml`（校验文案）

**说明**：菜单标题加字符约束（中文字母数字 `-_`），支持 Tab 补全无歧义。

- [ ] **Step 1: EditForm 标题校验**

读 EditForm.tsx 标题输入框（grep `title` input）。加校验正则：允许中文（`\u4e00-\u9fa5`）+ 字母数字 + `-_`，禁空格/特殊字符。

```tsx
const TITLE_REGEX = /^[\u4e00-\u9fa5a-zA-Z0-9_-]+$/;
// 标题输入 onChange 后校验
{editingForm.title && !TITLE_REGEX.test(editingForm.title) && (
  <span className="text-destructive text-xs">{t("settings.actionBar.titleInvalid")}</span>
)}
```

- [ ] **Step 2: i18n**

`slashNameInvalid` 旁加 `titleInvalid`：「标题只能中文、字母、数字、连字符、下划线」。

- [ ] **Step 3: tsc + vite build**

Run: `cd crates/desktop/frontend && npx tsc -b && npx vite build`

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/ActionBar/EditForm.tsx crates/desktop/frontend/src/locales/
git commit -m "feat(actionbar): 菜单标题字符约束（中文/字母/数字/-/_）

支持 slash Tab 补全无歧义。现有 seed 标题均符合。"
```

---

## Task 4: 文档同步 + e2e

**Files:**
- Modify: `docs/architecture.md`

- [ ] **Step 1: architecture.md 更新 v2**

slash 命令段补 v2 增强：IME 兼容、候选池扩大、Tab 补全、标题约束。

- [ ] **Step 2: 手动 e2e**

- [ ] `、google hello`（顿号开头）→ 触发
- [ ] `/百度`（标题匹配，无 trigger）→ 命中
- [ ] `/goo` → trigger 命中优先
- [ ] Tab 补全 `/Google ` → 锁定，输参数回车执行
- [ ] 菜单标题输入空格 → 校验提示

- [ ] **Step 3: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: architecture slash v2 增强（IME/标题匹配/Tab补全/标题约束）"
```

---

## Self-Review

**1. Spec coverage**：IME 兼容→Task 1；候选池+标题匹配→Task 1；Tab 补全+锁定→Task 2；标题约束→Task 3；文档→Task 4 ✓

**2. Type consistency**：
- `slash_result(r, params, score)` Task 1 定义；action_data schema 加 `title`
- `slash-complete` action Task 2 定义（keyNavigation）+ 消费（useActionBarKeydown）
- `slashLockedItemId` state Task 2 定义（index.tsx）+ 消费（执行）
- `HandledActionTypes` 加 slash-complete（exhaustive test-d）

**3. 风险**：Tab 补全的锁定/解锁逻辑是交互核心，需仔细处理边界（用户删除部分标题、切 tab 等）。e2e 必须覆盖。
