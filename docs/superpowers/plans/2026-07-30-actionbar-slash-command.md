# ActionBar `/` 斜杠命令 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ActionBar 搜索框输入 `/keyword [params]` 触发配了命令名（trigger_keyword）的菜单项，即时候选列表，复用现有字段无 schema 变更。

**Architecture:** `trigger_keyword` 字段语义从「裸关键词」改为「slash 命令名」。后端 `MenuProvider` 新增 `search_slash_commands`（query 以 `/` 开头 → fuzzy 匹配 trigger_keyword → source="slash" 候选）。前端新增 `slash` tab（输入 `/` 自动跳），`executeSearchResult` 加 case "slash" 按 action_type 分流执行。

**Tech Stack:** Rust（octopus-search crate），TypeScript/React（Tauri 2 前端），Vitest。

**Spec:** `docs/superpowers/specs/2026-07-30-actionbar-slash-command-design.md`

## Global Constraints

- **trigger_keyword 语义迁移**：旧「`tr hello` 空格分隔裸关键词」废弃，改为「`/keyword` slash 命令名」。字段名不变（免 migration）。
- **DB 无 schema 变更**：复用 `trigger_keyword` 列（`db.sql:135`，TEXT NOT NULL DEFAULT ''）。
- **need_voice 录音路径不变**：agent 项 need_voice=true + `/cmd` 无参数 → 仍走 `trigger_agent_voice`（不搞文本替代 voice）。
- **命令名校验**：保存时 trim + lowercase，限 `[a-z][a-z0-9-]*`（防空格/特殊字符致 `/cmd` 解析歧义）。
- **TDD**：后端纯函数（search_slash_commands）先写测试；前端 tab 逻辑可单测。
- **工作目录**：`/Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730`。后端测试 `cargo test -p octopus-search --lib`，前端 `cd crates/desktop/frontend && npx vitest run <file>` / `npx tsc -b`。

---

## File Structure

| 文件 | 职责 |
|---|---|
| `crates/search/src/providers/menu.rs` | 删 `search_quicklink_keywords`，加 `search_slash_commands`；`matches_tab` 加 slash |
| `crates/infra/src/db.sql` | 搜索引擎 seed 补 trigger_keyword（ON CONFLICT DO UPDATE 覆盖老库） |
| `crates/desktop/frontend/src/pages/ActionBar/searchTypes.ts` | `TabId` 加 slash，`TABS` 加项 |
| `crates/desktop/frontend/src/pages/ActionBar/searchLogic.ts` | `filterByTab` sourceMap 加 slash；`getVisibleTabs` 含 slash |
| `crates/desktop/frontend/src/pages/ActionBar/index.tsx` | query effect 自动跳 slash tab；`executeSearchResult` 加 case "slash" |
| `crates/desktop/frontend/src/pages/Settings/ActionBar/EditForm.tsx` | 放开 trigger_keyword 类型限制 + 命令名校验 |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml` | i18n（slash tab label + 设置文案） |
| `docs/architecture.md` | 同步 trigger_keyword 新语义 + slash 命令 |

---

## Task 1: 后端 search_slash_commands 纯函数（TDD）

**Files:**
- Modify: `crates/search/src/providers/menu.rs`（删 quicklink_keywords，加 slash_commands）
- Test: `crates/search/src/providers/menu.rs`（内联 `#[cfg(test)]`）

**Interfaces:**
- Consumes: `match_score` from `crate::matcher`，`ActionBarItem` from `octopus_infra::db`
- Produces: `fn search_slash_commands(query: &str, rows: &[ActionBarItem]) -> Vec<SearchResult>`；`MenuProvider.matches_tab` 含 `"slash"`

**说明**：核心匹配逻辑。先写测试驱动。删旧的 `search_quicklink_keywords`（裸关键词废弃）。

- [x] **Step 1: 写 search_slash_commands 的失败测试**

在 `menu.rs` 的 `#[cfg(test)] mod tests` 里（若无则新建）加测试。先看现有测试结构：

Run: `cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730 && rg -n "#\[cfg\(test\)\]|mod tests|fn .*test" crates/search/src/providers/menu.rs`

在 `menu.rs` 末尾追加（若已有 test mod 则追加到 mod 内）：

```rust
#[cfg(test)]
mod slash_command_tests {
    use super::*;
    use octopus_infra::db::ActionBarItem;

    fn item(id: i64, over: &str, trigger: &str, action_type: &str) -> ActionBarItem {
        // 构造测试用 ActionBarItem（trigger_keyword 非空才进 slash 匹配）
        ActionBarItem {
            id,
            parent_id: None,
            title: format!("Test {}", id),
            icon: String::new(),
            action_type: action_type.into(),
            action_data: format!("https://example.com/?q={{query}}"),
            sort_order: 0,
            is_system: false,
            is_enabled: true,
            is_async: false,
            write_output_to_clipboard: false,
            shortcut: String::new(),
            agent: String::new(),
            accepts: "text".into(),
            trigger_keyword: trigger.into(),
            global_shortcut: String::new(),
            need_voice: false,
            app_bundle_ids: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn slash_with_cmd_and_params_matches() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("/google hello", &rows);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "slash");
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["cmd"], "google");
        assert_eq!(data["params"], "hello");
        assert_eq!(data["id"], 1);
    }

    #[test]
    fn slash_cmd_no_params() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("/google", &rows);
        assert_eq!(results.len(), 1);
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["params"], "");
    }

    #[test]
    fn slash_only_returns_all_commands() {
        // 仅 "/" → 返回所有配了 trigger_keyword 的项
        let rows = vec![item(1, "", "google", "url"), item(2, "", "tolaria", "agent")];
        let results = search_slash_commands("/", &rows);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn slash_fuzzy_matches_partial() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("/goo", &rows);
        assert_eq!(results.len(), 1); // fuzzy 命中
    }

    #[test]
    fn slash_no_match_returns_empty() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("/xyz", &rows);
        assert!(results.is_empty());
    }

    #[test]
    fn non_slash_query_returns_empty() {
        let rows = vec![item(1, "", "google", "url")];
        let results = search_slash_commands("google hello", &rows);
        assert!(results.is_empty()); // 不以 / 开头，不触发 slash 匹配
    }

    #[test]
    fn slash_matches_all_action_types() {
        // agent/ai/script 类型配了 trigger_keyword 也能匹配
        let rows = vec![item(1, "", "tolaria", "agent")];
        let results = search_slash_commands("/tolaria", &rows);
        assert_eq!(results.len(), 1);
        let data: serde_json::Value = serde_json::from_str(&results[0].action_data).unwrap();
        assert_eq!(data["action_type"], "agent");
    }

    #[test]
    fn slash_empty_trigger_keyword_excluded() {
        let rows = vec![item(1, "", "", "url")]; // trigger_keyword 空
        let results = search_slash_commands("/anything", &rows);
        assert!(results.is_empty());
    }
}
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test -p octopus-search --lib slash_command_tests 2>&1 | tail -10`
Expected: FAIL — `search_slash_commands` 未定义。

- [x] **Step 3: 实现 search_slash_commands**

在 `menu.rs` 删除 `search_quicklink_keywords` 函数（旧裸关键词逻辑），替换为 `search_slash_commands`：

```rust
/// Slash 命令匹配：query 以 `/cmd [params]` 模式开头时，
/// fuzzy 匹配 trigger_keyword 非空的菜单项（所有 action_type），
/// 返回 source="slash" 候选。params 记入 action_data 供执行时用。
///
/// 仅 "/" → 返回所有配了 trigger_keyword 的命令（score 一致，按 trigger_keyword 序）。
/// query 不以 / 开头 → 空结果（不影响普通搜索）。
fn search_slash_commands(query: &str, rows: &[octopus_infra::db::ActionBarItem]) -> Vec<SearchResult> {
    let rest = match query.strip_prefix('/') {
        Some(r) => r,
        None => return vec![],
    };
    // 仅 "/" → 返回全部命令
    if rest.is_empty() {
        return rows.iter()
            .filter(|r| r.is_enabled && !r.trigger_keyword.is_empty())
            .map(slash_result)
            .collect();
    }
    // 切 cmd（/ 后到空格前）+ params（空格后）
    let (cmd, params) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    };
    let mut scored: Vec<(i32, SearchResult)> = rows.iter()
        .filter(|r| r.is_enabled && !r.trigger_keyword.is_empty())
        .filter_map(|r| {
            let score = match_score(cmd, &r.trigger_keyword)?;
            Some((score, slash_result_with_params(r, params)))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(10).map(|(_, r)| r).collect()
}

/// 构造 slash 命令候选结果（无 params 版，用于仅 "/" 时列全部）。
fn slash_result(r: &octopus_infra::db::ActionBarItem) -> SearchResult {
    slash_result_with_params(r, "")
}

fn slash_result_with_params(r: &octopus_infra::db::ActionBarItem, params: &str) -> SearchResult {
    SearchResult {
        source: "slash".into(),
        title: format!("/{}", r.trigger_keyword),
        subtitle: r.title.clone(),
        icon: None,
        action_type: r.action_type.clone(),
        action_data: serde_json::json!({
            "id": r.id,
            "cmd": r.trigger_keyword,
            "params": params,
            "action_type": r.action_type,
            "action_data": r.action_data,
        }).to_string(),
        score: 0,
    }
}
```

- [x] **Step 4: MenuProvider 接入 + 删旧 quicklink 调用**

`MenuProvider::matches_tab` 加 `"slash"`：
```rust
fn matches_tab(&self, tab: &str) -> bool {
    matches!(tab, "quick" | "actions" | "slash")
}
```

`MenuProvider::search` 里把 `search_quicklink_keywords(query, &rows)` 替换为 `search_slash_commands(query, &rows)`：
```rust
async fn search(&self, query: &str, _ctx: &SearchContext<'_>) -> Vec<SearchResult> {
    let rows = match octopus_infra::db::list_action_bar_items() {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let mut results = search_menus(query, &rows);
    results.extend(search_slash_commands(query, &rows));
    results
}
```

- [x] **Step 5: 运行测试确认通过**

Run: `cargo test -p octopus-search --lib slash_command_tests 2>&1 | tail -10`
Expected: PASS（8 个测试全过）。

- [x] **Step 6: 跑全 search crate 测试 + 编译**

Run: `cargo test -p octopus-search --lib 2>&1 | tail -5 && cargo build -p octopus-search 2>&1 | tail -3`
Expected: 全过，0 error。若有旧 quicklink 测试引用 `search_quicklink_keywords`，删除/更新它们。

- [x] **Step 7: Commit**

```bash
git add crates/search/src/providers/menu.rs
git commit -m "feat(actionbar): search_slash_commands 后端匹配 + 废弃 quicklink_keywords

trigger_keyword 语义改为 slash 命令名。/cmd [params] fuzzy 匹配 trigger_keyword
非空的菜单项（所有 action_type），产出 source=slash 候选。仅 / 返回全部命令。
删旧 search_quicklink_keywords（裸关键词废弃）。8 单测覆盖。"
```

---

## Task 2: DB seed 搜索引擎补 trigger_keyword

**Files:**
- Modify: `crates/infra/src/db.sql`

**说明**：给 Google/百度/Bing/Github seed 配 trigger_keyword，让 `/google` 等开箱即用。用 `ON CONFLICT(id) DO UPDATE` 确保老库（id 已存在）也能补上字段。

- [x] **Step 1: 改 db.sql seed**

把 `db.sql:482-486` 的搜索引擎 seed INSERT 改为含 trigger_keyword + ON CONFLICT 补字段。先读当前内容确认列名顺序：

Run: `cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730 && sed -n '481,487p' crates/infra/src/db.sql`

改为（在原 INSERT OR IGNORE 后追加 ON CONFLICT 更新 trigger_keyword；或用独立 UPDATE 语句）：

```sql
-- 搜索子菜单（parent_id=3）——补 trigger_keyword 使 /google 等开箱即用
INSERT OR IGNORE INTO action_bar_items (id, parent_id, title, icon, action_type, action_data, sort_order, is_system) VALUES
    (8, 3, 'Google', 'search', 'url', 'https://www.google.com/search?q={text}', 0, 1),
    (9, 3, '百度',   'search', 'url', 'https://www.baidu.com/s?wd={text}', 1, 1),
    (10, 3, 'Bing',  'search', 'url', 'https://www.bing.com/search?q={text}', 2, 1),
    (11, 3, 'Github', 'search', 'url', 'https://github.com/search?type=repositories&q={text}', 3, 1);
-- 老库（id 已存在）补 trigger_keyword；新库由上行 INSERT 默认空，此处统一补
UPDATE action_bar_items SET trigger_keyword='google' WHERE id=8 AND trigger_keyword='';
UPDATE action_bar_items SET trigger_keyword='baidu'  WHERE id=9 AND trigger_keyword='';
UPDATE action_bar_items SET trigger_keyword='bing'   WHERE id=10 AND trigger_keyword='';
UPDATE action_bar_items SET trigger_keyword='github' WHERE id=11 AND trigger_keyword='';
```

> 用 UPDATE ... AND trigger_keyword='' 保护用户已自定义的值（不覆盖用户改过的）。

- [x] **Step 2: 编译 + 测试**

Run: `cargo build -p octopus-infra 2>&1 | tail -3 && cargo test -p octopus-infra --lib 2>&1 | grep -E "test result:|error" | tail -3`
Expected: 0 error，测试全过（db.sql 是 include_str!，语法错会编译失败）。

- [x] **Step 3: Commit**

```bash
git add crates/infra/src/db.sql
git commit -m "feat(actionbar): 搜索引擎 seed 补 trigger_keyword（/google /baidu /bing /github）

UPDATE...AND trigger_keyword='' 保护用户自定义值，老库自动补字段。"
```

---

## Task 3: 前端 slash Tab 体系

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/searchTypes.ts`
- Modify: `crates/desktop/frontend/src/pages/ActionBar/searchLogic.ts`
- Test: `crates/desktop/frontend/src/pages/ActionBar/searchLogic.test.ts`

**Interfaces:**
- Produces: `TabId` 含 `"slash"`；`TABS` 含 slash 项；`filterByTab` 识别 slash source

- [x] **Step 1: 写 filterByTab slash 测试（追加到 searchLogic.test.ts）**

在 `searchLogic.test.ts` 末尾追加（先看现有 import 风格）：

Run: `cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730 && head -20 crates/desktop/frontend/src/pages/ActionBar/searchLogic.test.ts`

追加（按现有 import 调整）：
```ts
import { filterByTab, getNextTab, getVisibleTabs } from "./searchLogic";
import type { SearchResult } from "./searchTypes";

describe("slash tab", () => {
  const slashResult: SearchResult = {
    source: "slash", title: "/google", subtitle: "Google",
    icon: null, actionType: "url", actionData: "{}", score: 100,
  };
  const appResult: SearchResult = {
    source: "app", title: "Chrome", subtitle: "",
    icon: null, actionType: "launch_app", actionData: "{}", score: 100,
  };

  it("filterByTab slash 只留 source=slash", () => {
    const filtered = filterByTab([slashResult, appResult], "slash");
    expect(filtered).toEqual([slashResult]);
  });

  it("getVisibleTabs 含 slash（无 context 也含）", () => {
    const tabs = getVisibleTabs(false);
    expect(tabs.find((t) => t.id === "slash")).toBeTruthy();
  });

  it("getNextTab 循环含 slash", () => {
    // 从某 tab 循环应能经过 slash（具体起点取决于 TABS 顺序，此处验证不报错 + 能到达）
    const tabs = getVisibleTabs(true);
    const slashIdx = tabs.findIndex((t) => t.id === "slash");
    expect(slashIdx).toBeGreaterThanOrEqual(0);
  });
});
```

- [x] **Step 2: 运行测试确认失败**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/ActionBar/searchLogic.test.ts 2>&1 | tail -10`
Expected: FAIL — `slash` 不是有效 TabId / TABS 无 slash。

- [x] **Step 3: searchTypes.ts 加 slash TabId + TABS**

`TabId` 联合加 `"slash"`：
```ts
export type TabId = "all" | "apps" | "files" | "bookmarks" | "actions" | "commands" | "slash";
```

`TABS` 数组加项（放末尾，commands 之后）：
```ts
  { id: "commands", label: "命令", key: "c" },
  { id: "slash", label: "斜杠", key: "s" },
] as const;
```

- [x] **Step 4: searchLogic.ts filterByTab sourceMap 加 slash**

```ts
const sourceMap: Record<string, string> = {
  apps: "app",
  files: "file",
  bookmarks: "bookmark",
  actions: "menu",
  commands: "command",
  slash: "slash",
};
```

- [x] **Step 5: 运行测试确认通过 + tsc**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/ActionBar/searchLogic.test.ts 2>&1 | tail -5 && npx tsc -b 2>&1 | tail -3`
Expected: 测试全过，tsc 0 error。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/searchTypes.ts crates/desktop/frontend/src/pages/ActionBar/searchLogic.ts crates/desktop/frontend/src/pages/ActionBar/searchLogic.test.ts
git commit -m "feat(actionbar): 前端新增 slash tab（label 斜杠）

TabId/TABS 加 slash；filterByTab sourceMap 加 slash→slash。
slash tab 无 context 也显示（命令不依赖选中文本）。"
```

---

## Task 4: 前端执行层（自动跳 tab + case "slash"）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`

**说明**：query effect 检测 `/` 自动跳 slash tab；`executeSearchResult` 加 case "slash" 按 action_type 分流。

- [x] **Step 1: query effect 自动跳 slash tab**

在 `index.tsx` 的 query 变化 effect（约 `:341-360`，`hasQuery(query)` 分支内）加 `/` 检测。先读当前 effect：

Run: `cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730 && sed -n '335,365p' crates/desktop/frontend/src/pages/ActionBar/index.tsx`

在 effect 开头（防抖逻辑之前）加：
```ts
  useEffect(() => {
    // 输入 / 开头 → 自动跳 slash tab（命令模式）
    if (query.startsWith("/") && activeTab !== "slash") {
      setActiveTab("slash");
      return;
    }
    // 原有防抖搜索逻辑...
  }, [query, activeTab, ...]);
```

注意：不要在删掉 `/` 时强制切回 all（用户可能想手动切）。若 query 不再以 `/` 开头但还在 slash tab，保持当前 tab（用户手动切回 all）。

- [x] **Step 2: executeSearchResult 加 case "slash"**

> **实施偏差（2026-08-02）**：实际实现**未用 `case "slash"` switch 分支**，改为在 switch 前 `if (source === "slash")` 分流（`index.tsx:574`）。原因：slash 结果需要先按 `data.action_type`（url/agent/script）二次分流，再进 switch 会嵌套两层 switch，可读性差。source 分流后复用既有 `case "url"`/`case "agent"` 分支（参数注入 `{query}`/`{text}` + `execute_action_bar`），避免重复逻辑。

在 `executeSearchResult`（约 `:520-621`）的 switch 里，`case "url"` 之前或之后加 `case "slash"`。先读现有 switch 结构确认位置：

Run: `sed -n '545,585p' crates/desktop/frontend/src/pages/ActionBar/index.tsx`

加：
```ts
      case "slash": {
        const itemId = data.id as number;
        const params = (data.params as string) || "";
        const actionType = data.action_type as string;
        const item = menuItemsRef.current.find((i) => i.id === itemId);
        if (!item) {
          console.warn("[slash] 菜单项未找到:", itemId);
          break;
        }
        // url 类型：params 替换 {query}/{text}，无 params 用选中文本
        if (actionType === "url") {
          const fallbackText = params || contextRef.current?.text || "";
          const rawUrl = (data.action_data as string) || item.actionData || "";
          const url = rawUrl
            .replace(/\{query\}/g, encodeURIComponent(fallbackText))
            .replace(/\{text\}/g, encodeURIComponent(fallbackText));
          if (url) {
            try {
              await invoke("open_url", { url });
              invoke("action_bar_dismiss", { reason: "slash-url" });
            } catch (e) {
              showQuickError(String(e).slice(0, 40));
            }
          }
          break;
        }
        // agent need_voice + 无参数 → 录音路径（不变）
        if (actionType === "agent" && item.needVoice && !params) {
          try {
            await invoke("trigger_agent_voice", { itemId });
          } catch (e) {
            showQuickError(String(e).slice(0, 40));
          }
          break;
        }
        // 其他（agent/ai/script + 有参数，或非 need_voice）→ execute_action_bar
        const text = params || contextRef.current?.text || "";
        try {
          await invoke("execute_action_bar", { itemId, text });
          invoke("action_bar_dismiss", { reason: "slash-exec" });
        } catch (e) {
          showQuickError(String(e).slice(0, 40));
        }
        break;
      }
```

- [x] **Step 3: 确认 menuItemsRef 含 slash 命令项**

slash 命令项来自 DB action_bar_items（配了 trigger_keyword）。`menuItemsRef` 来自 `menuItems` state（`index.tsx:57`），含全部 action_bar_items。确认 `find((i) => i.id === itemId)` 能命中——slash 结果的 action_data.id 是 DB 行 id，与 menuItems 的 item.id 一致。

Run: `rg -n "menuItemsRef|const menuItems |useState.*ActionBarItem" crates/desktop/frontend/src/pages/ActionBar/index.tsx | head`

若 menuItems 含全部项（含子菜单项），无需改。若只含主菜单项，需确认 slash 命令项是否都在主菜单（搜索引擎是子菜单项 id=8-11，parent_id=3）。

> ⚠️ 注意：搜索引擎是子菜单项（parent_id=3），`mainItems`（`index.tsx:413`）只含 parent_id=null。但 `menuItems`（全量）含子菜单项。case "slash" 用 `menuItemsRef`（全量）find，应能命中。确认 menuItemsRef 指向全量 menuItems。

- [x] **Step 4: tsc + vite build**

Run: `cd crates/desktop/frontend && npx tsc -b 2>&1 | tail -3 && npx vite build 2>&1 | tail -3`
Expected: 0 error。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/index.tsx
git commit -m "feat(actionbar): 前端 / 命令执行层

query 以 / 开头自动跳 slash tab；executeSearchResult 加 case slash：
url→替换 {query}/{text}；agent need_voice 无参数→录音；其他→execute_action_bar。"
```

---

## Task 5: 设置 UI 放开 trigger_keyword 类型 + 命令名校验

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ActionBar/EditForm.tsx`
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml`

**说明**：所有 action_type 都能配 trigger_keyword（不再限 url）；加命令名格式校验。

- [x] **Step 1: EditForm 放开类型限制**

读 `EditForm.tsx:235-252` 当前 trigger_keyword 输入框（条件 `type === "url"`）：

Run: `cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730 && sed -n '230,255p' crates/desktop/frontend/src/pages/Settings/ActionBar/EditForm.tsx`

把 `type === "url"` 条件去掉（或改为排除 submenu——submenu 是容器不触发动作）。所有非 submenu 类型都能配。输入校验加 `[a-z][a-z0-9-]*`：

> **实施偏差（2026-08-02）**：校验正则从 `[a-z][a-z0-9-]*`（纯小写英文）**放宽为 `TITLE_REGEX`**（支持中文 + 字母 + 数字 + 连字符 + 下划线）。原因：用户需求 `/百度` 这类中文命令名，纯英文限制过严。i18n 文案对应改为「只能中文、字母、数字、连字符、下划线」。placeholder 也改为「如 google 或 百度」。

```tsx
{/* / 命令名（trigger_keyword）——所有动作类型可配，submenu 除外 */}
{editingForm.actionType !== "submenu" && (
  <Row label={t("settings.actionBar.slashName")}>
    <input
      value={editingForm.triggerKeyword}
      onChange={(e) => {
        const v = e.target.value.trim().toLowerCase();
        setEditingForm({ ...editingForm, triggerKeyword: v });
      }}
      placeholder={t("settings.actionBar.slashNamePlaceholder")}
      className="..."
    />
    {editingForm.triggerKeyword && !/^[a-z][a-z0-9-]*$/.test(editingForm.triggerKeyword) && (
      <span className="text-destructive text-xs">
        {t("settings.actionBar.slashNameInvalid")}
      </span>
    )}
  </Row>
)}
```

> ActionBarPanel.tsx 的保存逻辑（`:151,168`）原 `triggerKeyword: editingForm.actionType === "url" ? ... : ""` 要改——不再按类型清空，直接透传 `editingForm.triggerKeyword`。

- [x] **Step 2: ActionBarPanel 保存逻辑放宽**

Run: `sed -n '145,175p' crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx`

把 create/update 的 `triggerKeyword` 字段从「非 url 清空」改为直接透传 `editingForm.triggerKeyword`（submenu 时清空）。

- [x] **Step 3: i18n 文案**

`zh-CN.yaml`（settings.actionBar 段，约 `:717`）：
```yaml
    slashName: "/ 命令名"
    slashNamePlaceholder: "如 google（输入 /google 触发）"
    slashNameInvalid: "只能小写字母、数字、连字符，须字母开头"
```
`en.yaml` 对应英文。

- [x] **Step 4: tsc + vite build**

Run: `cd crates/desktop/frontend && npx tsc -b 2>&1 | tail -3 && npx vite build 2>&1 | tail -3`
Expected: 0 error。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/ActionBar/EditForm.tsx crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx crates/desktop/frontend/src/locales/zh-CN.yaml crates/desktop/frontend/src/locales/en.yaml
git commit -m "feat(actionbar): 设置 UI 放开 trigger_keyword 类型 + 命令名校验

所有非 submenu 类型都能配 / 命令名；校验 [a-z][a-z0-9-]*。
ActionBarPanel 保存不再按类型清空 trigger_keyword。i18n 同步。"
```

---

## Task 6: 文档同步 + 手动 e2e

**Files:**
- Modify: `docs/architecture.md`

- [x] **Step 1: architecture.md 同步**

在 ActionBar 段（grep `trigger_keyword` / `Quicklink` / `搜索驱动命令面板`）更新：
- trigger_keyword 语义：旧「裸关键词空格触发」→ 新「slash 命令名（/cmd）」
- 新增 slash tab 说明
- 搜索引擎 seed 配 trigger_keyword（/google /baidu /bing /github）

- [x] **Step 2: 手动 e2e 回归**

构建并测试（需 sidecar；若缺失至少 vite build + tsc）：
- [x] 输入 `/` → slash tab 自动激活，显示全部命令候选
- [x] `/google hello` → 打开 google 搜索 hello
- [x] `/google`（无参数）→ 用选中文本搜索（或无选中时空操作）
- [x] `/goo` → fuzzy 命中 google
- [x] `/tolaria`（agent need_voice 无参数）→ 触发录音
- [x] 设置页给 agent 项配命令名 → `/cmd` 触发
- [x] 命令名校验：输入 `My Cmd` → 提示无效
- [x] 普通搜索（不以 / 开头）不受影响

- [x] **Step 3: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: architecture 同步 slash 命令 + trigger_keyword 新语义"
```

---

## Self-Review

**1. Spec coverage**：
- trigger_keyword 语义迁移 → Task 1（后端）+ Task 5（UI）✓
- search_slash_commands 匹配 → Task 1 ✓
- slash tab → Task 3 ✓
- 自动跳 tab → Task 4 ✓
- 执行分流（url/agent need_voice/其他）→ Task 4 ✓
- 内置 seed → Task 2 ✓
- 命令名校验 → Task 5 ✓
- 文档 → Task 6 ✓

**2. Placeholder scan**：无 TBD。每步含完整代码或确切命令。

**3. Type consistency**：
- `search_slash_commands(query, rows) -> Vec<SearchResult>` Task 1 定义，Task 1 消费 ✓
- `TabId` 含 `"slash"` Task 3 定义，Task 4 消费 ✓
- action_data schema（id/cmd/params/action_type/action_data）Task 1 产出，Task 4 消费（data.id/data.params/data.action_type/data.action_data）✓
- `needVoice` 字段名：TS 用 camelCase（`item.needVoice`），与 ActionBarItem TS interface 一致（types.ts）✓

---

## 实施记录（2026-08-02 回写）

**状态**：✅ 全部 Task 1-6 已完成并验证。功能已上线（main 含完整实现）。

**测试覆盖**：
- 后端 `search_slash_commands`：8 个单测（`menu.rs` 内联 `#[cfg(test)]`，覆盖 /cmd+params、仅 /、fuzzy、无匹配、非 / 开头、全 action_type、空 trigger_keyword 排除）
- 前端 `searchLogic.ts`：slash tab 可见性测试（非 slash 模式隐藏 slash tab / slash 模式只显 slash tab）
- e2e：用户已验证 `/google`、`/百度`（中文命令名）等场景

**与 plan 的偏差（2 处）**：
1. **Task 4 执行分流**：plan 写 `case "slash"` switch 分支，实际改为 switch 前 `source === "slash"` 分流。slash 结果需按 `data.action_type` 二次分流，嵌套 switch 可读性差，source 分流后复用既有 case 分支更简洁。
2. **Task 5 校验正则**：plan 写 `[a-z][a-z0-9-]*`（纯英文），实际放宽为 `TITLE_REGEX`（支持中文 + 字母 + 数字 + 连字符 + 下划线）。用户需求 `/百度` 等中文命令名。

**工作目录注**：plan 里写的 `.worktrees/daily_bugfix_0730` 是规划时的 worktree，实际实现可能在其他 worktree 完成（worktree 是临时的，代码已进 main）。
