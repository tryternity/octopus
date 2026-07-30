# ActionBar keydown 抽取重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `index.tsx` 里 212 行的 keydown `useEffect` 拆成纯逻辑层（`keyNavigation.ts`）+ 副作用层（`useActionBarKeydown.ts`），行为零变化。

**Architecture:** `decideKeyAction(e, ctx)` 纯函数返回 `KeyAction` 判别联合（给定键盘事件 + refs 快照 → 决定动作）；`useActionBarKeydown(params)` hook 从 refs 组装 ctx、调纯函数、`switch(action.type)` 执行 setState/invoke。纯逻辑无 React/DOM 依赖，可独立单测。

**Tech Stack:** TypeScript, React hooks (useEffect), Vitest (jsdom 环境，支持 `new KeyboardEvent`)。

**Spec:** `docs/superpowers/specs/2026-07-30-actionbar-keydown-extract-design.md`

## Global Constraints

- **行为零变化**：`decideKeyAction` 的 24 条判断顺序、preventDefault 时机、IME 放行、500ms 确认窗口、子菜单 engineIdx 预选，逐条对应原 handler（`index.tsx` 重构前 652-863 行）。
- **preventDefault 规则**：`passthrough` / `ignore` / `ime-composing` / `ime-confirm-enter` 不 preventDefault；其余 action 都 preventDefault。
- **§PRESERVE**：两处原行为怪异点保持原样（行 778 Alt 其他键放行无 preventDefault；行 820 ↑↓ 无条件 preventDefault 但非 submenu 项不切层）。不借重构修正。
- **TDD**：纯逻辑层先写测试再实现。副作用层靠现有测试 + 手动 e2e 回归。
- **类型来源**：`ActionBarItem` 来自 `./types`；`View`/`TabId`/`SearchResult`（别名 `SearchHit`）来自 `./searchTypes`；`moveDirection`/`navigateResults`/`getNextTab`/`hasQuery` 来自 `./searchLogic`；`codeToChar` 来自 `./label`。
- **工作目录**：`/Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730`。前端测试命令 `cd crates/desktop/frontend && npx vitest run <file>`，类型检查 `npx tsc -b`。

---

## File Structure

| 文件 | 职责 |
|------|------|
| `pages/ActionBar/keyNavigation.ts` | 新增。纯逻辑层：`KeyContext` / `KeyAction` 类型 + `decideKeyAction(e, ctx)` + `pickSubIdx(subs, searchEngine)`。无 React/DOM 依赖。 |
| `pages/ActionBar/keyNavigation.test.ts` | 新增。`decideKeyAction` + `pickSubIdx` 单测，覆盖 24 条判断。 |
| `pages/ActionBar/useActionBarKeydown.ts` | 新增。副作用层 hook：`ActionBarKeydownParams` 类型 + `useActionBarKeydown(p)`。 |
| `pages/ActionBar/index.tsx` | 改：删 652-863 的 keydown effect，替换为一行 `useActionBarKeydown({...})`。 |

---

## Task 1: pickSubIdx 纯函数（TDD）

**Files:**
- Create: `crates/desktop/frontend/src/pages/ActionBar/keyNavigation.ts`
- Test: `crates/desktop/frontend/src/pages/ActionBar/keyNavigation.test.ts`

**Interfaces:**
- Produces: `pickSubIdx(subs: ActionBarItem[], searchEngine: string): number`（供 Task 2 的 `decideKeyAction` 内部用，也独立可测）

**说明**：先建文件骨架（类型 + `pickSubIdx`），用最小测试驱动。`KeyContext`/`KeyAction` 类型也在此 task 定义（decideKeyAction 在 Task 2 实现）。

- [x] **Step 1: 写 pickSubIdx 的失败测试**

Create `crates/desktop/frontend/src/pages/ActionBar/keyNavigation.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { pickSubIdx } from "./keyNavigation";
import type { ActionBarItem } from "./types";

/** 构造 ActionBarItem 的 helper（测试用，填默认值） */
function item(over: Partial<ActionBarItem> & { id: number }): ActionBarItem {
  return {
    parentId: null, title: "", icon: "", actionType: "", actionData: "",
    sortOrder: 0, isSystem: false, isEnabled: true, ...over,
  };
}

describe("pickSubIdx", () => {
  it("空列表 → 0", () => {
    expect(pickSubIdx([], "google")).toBe(0);
  });

  it("首项非 url 类型 → 0", () => {
    const subs = [item({ id: 1, actionType: "menu" })];
    expect(pickSubIdx(subs, "google")).toBe(0);
  });

  it("首项是 url，title 匹配 searchEngine（小写） → 返回匹配 idx", () => {
    const subs = [
      item({ id: 1, actionType: "url", title: "Google" }),
      item({ id: 2, actionType: "url", title: "Bing" }),
    ];
    expect(pickSubIdx(subs, "google")).toBe(0);
    expect(pickSubIdx(subs, "bing")).toBe(1);
  });

  it("首项是 url，无匹配 → 0", () => {
    const subs = [
      item({ id: 1, actionType: "url", title: "Google" }),
      item({ id: 2, actionType: "url", title: "Bing" }),
    ];
    expect(pickSubIdx(subs, "duckduckgo")).toBe(0);
  });
});
```

- [x] **Step 2: 运行测试以验证其失败**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/ActionBar/keyNavigation.test.ts`
Expected: FAIL — "Failed to resolve import ./keyNavigation"（文件不存在）。

- [x] **Step 3: 写 keyNavigation.ts 骨架 + pickSubIdx 实现**

Create `crates/desktop/frontend/src/pages/ActionBar/keyNavigation.ts`:

```ts
/** ActionBar 键盘导航纯逻辑 —— 无 React/DOM 依赖，可独立单测。
 *
 * 设计（详见 spec 2026-07-30-actionbar-keydown-extract-design.md）：
 * - decideKeyAction(e, ctx) 给定键盘事件 + refs 快照 → 返回 KeyAction
 * - hook 从 refs 组装 ctx，根据 action 执行副作用
 *
 * 判断顺序严格复刻 index.tsx 重构前 keydown handler（652-863 行）。
 */

import type { ActionBarItem } from "./types";
import type { TabId, View } from "./searchTypes";

// ═══════════════════ 类型 ═══════════════════

/** 纯函数输入——从 refs 同步读取的上下文快照。无 React 依赖。 */
export interface KeyContext {
  /** "search" = hasQuery(query)；"menu" = query 空 */
  mode: "search" | "menu";
  view: View;
  focusLayer: "main" | "sub";
  query: string;
  /** 主菜单项（已过滤 isVisible + isEnabled） */
  mainItems: ActionBarItem[];
  /** 子菜单项（已过滤） */
  subItems: ActionBarItem[];
  /** 全量菜单项（Alt+字母匹配 + submenu 子项展开计算用） */
  menuItems: ActionBarItem[];
  selectedIdx: number;
  subSelectedIdx: number;
  searchSelectedIdx: number;
  activeTab: TabId;
  hasContext: boolean;
  searchEngine: string;
  /** IME 最后按键时刻（Date.now()）；Enter 500ms 内 = 选词确认窗口 */
  lastImeKeyTime: number;
  /** 搜索结果列表长度（↑↓/Enter 用） */
  searchResultsCount: number;
}

/** 纯函数输出——判别联合，枚举所有键盘动作。
 *
 * preventDefault 规则（hook 执行时统一应用）：
 *   - passthrough / ignore / ime-composing / ime-confirm-enter → 不 preventDefault（放行）
 *   - 其余所有 action → preventDefault */
export type KeyAction =
  | { type: "ime-composing" }
  | { type: "ime-confirm-enter" }
  | { type: "passthrough" }
  | { type: "ignore" }
  | { type: "escape-clear-query" }
  | { type: "escape-dismiss" }
  | { type: "search-tab"; dir: 1 | -1 }
  | { type: "search-nav"; dir: 1 | -1 }
  | { type: "search-enter" }
  | { type: "menu-move"; forward: boolean }
  | { type: "menu-toggle-layer" }
  | { type: "menu-enter" }
  | { type: "alt-execute"; item: ActionBarItem }
  | { type: "alt-goto-sub"; idx: number }
  | { type: "alt-goto-main"; idx: number; expandSubmenu: boolean; parentId?: number; subIdx?: number };

// ═══════════════════ 子菜单预选 ═══════════════════

/**
 * 给定 submenu 子项列表 + 当前搜索引擎，算初始 subSelectedIdx。
 * 规则（复刻 index.tsx 行 762-767 / 803-808 / 836-841）：首项是 url 类型时，
 * 按 title(小写) 匹配 searchEngine；匹配不到或首项非 url → 0。
 */
export function pickSubIdx(
  subs: ActionBarItem[],
  searchEngine: string,
): number {
  if (subs.length > 0 && subs[0].actionType === "url") {
    const idx = subs.findIndex(
      (s) => s.title.toLowerCase() === searchEngine,
    );
    return idx >= 0 ? idx : 0;
  }
  return 0;
}
```

- [x] **Step 4: 运行测试以验证其通过**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/ActionBar/keyNavigation.test.ts`
Expected: PASS（4 个 pickSubIdx 测试全过）。

- [x] **Step 5: 类型检查**

Run: `cd crates/desktop/frontend && npx tsc -b`
Expected: 0 error（`KeyContext`/`KeyAction` 定义但未使用会有 warning？不会——导出的类型不报 unused）。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/keyNavigation.ts crates/desktop/frontend/src/pages/ActionBar/keyNavigation.test.ts
git commit -m "refactor(actionbar): 抽出 pickSubIdx 纯函数 + keyNavigation 类型骨架

4 处重复的子菜单 engineIdx 预选逻辑（index.tsx 762-767/803-808/836-841）抽成
pickSubIdx 纯函数。同时定义 KeyContext/KeyAction 类型骨架（decideKeyAction 下个 task 实现）。"
```

---

## Task 2: decideKeyAction 纯函数（TDD）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/keyNavigation.ts`（加 `decideKeyAction`）
- Test: `crates/desktop/frontend/src/pages/ActionBar/keyNavigation.test.ts`（加测试）

**Interfaces:**
- Consumes: `moveDirection`/`hasQuery` from `./searchLogic`；`codeToChar` from `./label`；`pickSubIdx` from Task 1。
- Produces: `decideKeyAction(e: KeyboardEvent, ctx: KeyContext): KeyAction`（供 Task 3 的 hook 调用）

**说明**：这是重构核心——24 条判断严格复刻原 handler。按 spec 的判断顺序表实现。测试覆盖每条判断。

- [x] **Step 1: 写 decideKeyAction 的失败测试（追加到 keyNavigation.test.ts）**

在 `keyNavigation.test.ts` 顶部 import 追加 `decideKeyAction` + `KeyContext`：

```ts
import { pickSubIdx, decideKeyAction } from "./keyNavigation";
import type { KeyContext } from "./keyNavigation";
```

在文件末尾追加（`item` helper 复用）：

```ts
/** 构造 KeyContext 的 helper（测试用，填默认 menu 模式） */
function ctx(over: Partial<KeyContext> = {}): KeyContext {
  return {
    mode: "menu", view: "main", focusLayer: "main", query: "",
    mainItems: [], subItems: [], menuItems: [],
    selectedIdx: 0, subSelectedIdx: 0, searchSelectedIdx: 0,
    activeTab: "all", hasContext: false, searchEngine: "google",
    lastImeKeyTime: 0, searchResultsCount: 0,
    ...over,
  };
}

/** 构造 KeyboardEvent 的 helper */
function key(over: Partial<KeyboardEventInit> & { key: string }): KeyboardEvent {
  return new KeyboardEvent("keydown", {
    bubbles: true, cancelable: true,
    keyCode: 0, altKey: false, metaKey: false, ctrlKey: false,
    shiftKey: false, code: "", isComposing: false,
    ...over,
  });
}

describe("decideKeyAction - 公共前置", () => {
  it("IME 组合中（keyCode 229）→ ime-composing", () => {
    expect(decideKeyAction(key({ key: "a", keyCode: 229 }), ctx())).toEqual({ type: "ime-composing" });
  });
  it("IME 组合中（isComposing）→ ime-composing", () => {
    expect(decideKeyAction(key({ key: "a", isComposing: true }), ctx())).toEqual({ type: "ime-composing" });
  });
  it("Enter 在 IME 后 500ms 内 → ime-confirm-enter", () => {
    expect(decideKeyAction(key({ key: "Enter" }), ctx({ lastImeKeyTime: Date.now() - 100 }))).toEqual({ type: "ime-confirm-enter" });
  });
  it("loading 视图 → ignore", () => {
    expect(decideKeyAction(key({ key: "Tab" }), ctx({ view: "loading" }))).toEqual({ type: "ignore" });
  });
  it("无修饰可打印字符 → passthrough", () => {
    expect(decideKeyAction(key({ key: "a" }), ctx())).toEqual({ type: "passthrough" });
  });
});

describe("decideKeyAction - Escape", () => {
  it("search 模式 → escape-clear-query", () => {
    expect(decideKeyAction(key({ key: "Escape" }), ctx({ mode: "search" }))).toEqual({ type: "escape-clear-query" });
  });
  it("menu 模式 → escape-dismiss", () => {
    expect(decideKeyAction(key({ key: "Escape" }), ctx({ mode: "menu" }))).toEqual({ type: "escape-dismiss" });
  });
});

describe("decideKeyAction - 搜索模式", () => {
  const sctx = () => ctx({ mode: "search", searchResultsCount: 3 });
  it("Tab → search-tab dir=1", () => {
    expect(decideKeyAction(key({ key: "Tab" }), sctx())).toEqual({ type: "search-tab", dir: 1 });
  });
  it("Shift+Tab → search-tab dir=-1", () => {
    expect(decideKeyAction(key({ key: "Tab", shiftKey: true }), sctx())).toEqual({ type: "search-tab", dir: -1 });
  });
  it("ArrowDown → search-nav dir=1", () => {
    expect(decideKeyAction(key({ key: "ArrowDown" }), sctx())).toEqual({ type: "search-nav", dir: 1 });
  });
  it("ArrowUp → search-nav dir=-1", () => {
    expect(decideKeyAction(key({ key: "ArrowUp" }), sctx())).toEqual({ type: "search-nav", dir: -1 });
  });
  it("Enter → search-enter", () => {
    expect(decideKeyAction(key({ key: "Enter" }), ctx({ mode: "search", lastImeKeyTime: 0 }))).toEqual({ type: "search-enter" });
  });
});

describe("decideKeyAction - 菜单模式移动", () => {
  const items: ActionBarItem[] = [
    item({ id: 1, actionType: "submenu" }),
    item({ id: 2, actionType: "menu" }),
  ];
  const subItems = [item({ id: 11, parentId: 1, actionType: "url", title: "Google" })];
  const menuItems = [...items, ...subItems];

  it("Tab 焦点 main 命中 submenu → open-submenu", () => {
    const a = decideKeyAction(key({ key: "Tab" }), ctx({ mainItems: items, menuItems, searchEngine: "google" }));
    expect(a.type).toBe("open-submenu");
  });
  it("Tab 焦点 main 命中非 submenu → close-submenu", () => {
    const a = decideKeyAction(key({ key: "Tab" }), ctx({ mainItems: items, menuItems, selectedIdx: 1 }));
    expect(a.type).toBe("close-submenu");
  });
  it("Tab 焦点 sub → menu-move", () => {
    const a = decideKeyAction(key({ key: "Tab" }), ctx({ mainItems: items, subItems, focusLayer: "sub" }));
    expect(a).toEqual({ type: "menu-move", forward: true });
  });
});

describe("decideKeyAction - 菜单模式切层", () => {
  const items: ActionBarItem[] = [item({ id: 1, actionType: "submenu" })];
  it("ArrowDown 焦点 sub → menu-toggle-layer", () => {
    expect(decideKeyAction(key({ key: "ArrowDown" }), ctx({ focusLayer: "sub" }))).toEqual({ type: "menu-toggle-layer" });
  });
  it("ArrowUp 焦点 main 当前项 submenu → menu-toggle-layer", () => {
    expect(decideKeyAction(key({ key: "ArrowUp" }), ctx({ mainItems: items, selectedIdx: 0 }))).toEqual({ type: "menu-toggle-layer" });
  });
  it("ArrowDown 焦点 main 当前项非 submenu → menu-toggle-layer（hook 内 no-op）", () => {
    expect(decideKeyAction(key({ key: "ArrowDown" }), ctx({ mainItems: [item({ id: 2, actionType: "menu" })], selectedIdx: 0 }))).toEqual({ type: "menu-toggle-layer" });
  });
});

describe("decideKeyAction - Alt 快捷键", () => {
  it("Alt+字母命中 → alt-execute", () => {
    const it1 = item({ id: 1, shortcut: "h" });
    const a = decideKeyAction(key({ key: "˙", altKey: true, code: "KeyH" }), ctx({ menuItems: [it1] }));
    expect(a.type).toBe("alt-execute");
    expect((a as any).item).toBe(it1);
  });
  it("Alt+字母未命中 → passthrough", () => {
    expect(decideKeyAction(key({ key: "˙", altKey: true, code: "KeyH" }), ctx({ menuItems: [] }))).toEqual({ type: "passthrough" });
  });
  it("Alt+数字焦点 sub → alt-goto-sub", () => {
    expect(decideKeyAction(key({ key: "1", altKey: true, code: "Digit1" }), ctx({ focusLayer: "sub" }))).toEqual({ type: "alt-goto-sub", idx: 0 });
  });
  it("Alt+数字焦点 main 命中 submenu → alt-goto-main expandSubmenu", () => {
    const items = [item({ id: 1, actionType: "submenu" })];
    const subs = [item({ id: 11, parentId: 1, actionType: "url", title: "Google" })];
    const a = decideKeyAction(key({ key: "1", altKey: true, code: "Digit1" }), ctx({ mainItems: items, menuItems: [...items, ...subs], searchEngine: "google" }));
    expect(a.type).toBe("alt-goto-main");
    expect((a as any).expandSubmenu).toBe(true);
    expect((a as any).parentId).toBe(1);
  });
  it("Alt+数字焦点 main 命中非 submenu → alt-goto-main 不展开", () => {
    const items = [item({ id: 2, actionType: "menu" })];
    const a = decideKeyAction(key({ key: "1", altKey: true, code: "Digit1" }), ctx({ mainItems: items }));
    expect(a).toMatchObject({ type: "alt-goto-main", idx: 0, expandSubmenu: false });
  });
});

describe("decideKeyAction - Enter/Space", () => {
  it("Enter → menu-enter", () => {
    expect(decideKeyAction(key({ key: "Enter" }), ctx({ mode: "menu", lastImeKeyTime: 0 }))).toEqual({ type: "menu-enter" });
  });
  it("Space → menu-enter", () => {
    expect(decideKeyAction(key({ key: " " }), ctx())).toEqual({ type: "menu-enter" });
  });
});
```

- [x] **Step 2: 运行测试以验证其失败**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/ActionBar/keyNavigation.test.ts`
Expected: FAIL — "decideKeyAction is not a function" 或 import 失败。

- [x] **Step 3: 实现 decideKeyAction**

在 `keyNavigation.ts` 顶部 import 追加：

```ts
import { moveDirection, hasQuery } from "./searchLogic";
import { codeToChar } from "./label";
```

在 `pickSubIdx` 之后追加 `decideKeyAction`：

```ts
/**
 * 核心：给定键盘事件 + ctx 快照，决定要执行的动作。
 *
 * 判断顺序严格复刻 index.tsx 重构前 keydown handler（652-860 行）——
 * 顺序不可调换，前置条件改变会导致后续分支行为偏移。
 *
 * 常量（与 index.tsx 一致）：ARROW_AS_TAB = true。
 */
const ARROW_AS_TAB = true;

export function decideKeyAction(
  e: KeyboardEvent,
  ctx: KeyContext,
): KeyAction {
  // 1. IME 组合中（keyCode 229 / isComposing）→ 放行
  if (e.keyCode === 229 || e.isComposing) {
    return { type: "ime-composing" };
  }
  // 2. Enter 在 IME 后 500ms 内 → 选词确认，放行
  if (e.key === "Enter" && Date.now() - ctx.lastImeKeyTime < 500) {
    return { type: "ime-confirm-enter" };
  }
  // 3-4. Escape（任何视图）
  if (e.key === "Escape") {
    return ctx.mode === "search"
      ? { type: "escape-clear-query" }
      : { type: "escape-dismiss" };
  }
  // 5. loading 视图不拦截
  if (ctx.view === "loading") {
    return { type: "ignore" };
  }
  // 6. 无修饰可打印字符 → 放行（IME 已在顶部拦截）
  if (!e.altKey && !e.metaKey && !e.ctrlKey) {
    const navKeys = ARROW_AS_TAB
      ? ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Tab", "Enter", " "]
      : ["ArrowUp", "ArrowDown", "Tab", "Enter", " "];
    if (!navKeys.includes(e.key)) {
      return { type: "passthrough" };
    }
  }

  // 7-11. 搜索模式（query 非空）
  if (ctx.mode === "search") {
    const dir = moveDirection(e.key, e.shiftKey, ARROW_AS_TAB);
    if (dir !== null) return { type: "search-tab", dir: dir ? 1 : -1 };
    if (e.key === "ArrowDown") return { type: "search-nav", dir: 1 };
    if (e.key === "ArrowUp") return { type: "search-nav", dir: -1 };
    if (e.key === "Enter") return { type: "search-enter" };
    return { type: "passthrough" }; // 兜底
  }

  // 12-16. 菜单模式 - Alt 快捷键
  if (e.altKey) {
    const ch = codeToChar(e.code);
    if (ch) {
      // Alt+字母 → 执行局部快捷键
      if (/^[a-z]$/.test(ch)) {
        const found = ctx.menuItems.find((i) => i.isEnabled && i.shortcut === ch);
        if (found) return { type: "alt-execute", item: found };
        return { type: "swallow" }; // 未命中（复刻行 743-746：分支入口无条件 preventDefault，find 无果仍抑制）
      }
      // Alt+数字 → 定位菜单项
      if (/^[1-9]$/.test(ch)) {
        const idx = parseInt(ch, 10) - 1;
        if (ctx.focusLayer === "sub") {
          return { type: "alt-goto-sub", idx };
        }
        // 焦点 main
        if (idx < ctx.mainItems.length) {
          const m = ctx.mainItems[idx];
          if (m.actionType === "submenu") {
            const subs = ctx.menuItems.filter((i) => i.isEnabled && i.parentId === m.id);
            const subIdx = pickSubIdx(subs, ctx.searchEngine);
            return { type: "alt-goto-main", idx, expandSubmenu: true, parentId: m.id, subIdx };
          }
          return { type: "alt-goto-main", idx, expandSubmenu: false };
        }
        // idx 越界 → 原代码分支入口（行 751）无条件 preventDefault，越界不进 if 但仍抑制后 return（行 775）
        return { type: "swallow" }; // 复刻行 770-772：越界时 setSelectedIdx 不调，但仍 preventDefault
      }
    }
    // Alt 但 codeToChar 无效 → 放行（§PRESERVE 行 778）
    return { type: "passthrough" };
  }

  // 17-19. 菜单模式 - Tab/←→ 移动
  const menuDir = moveDirection(e.key, e.shiftKey, ARROW_AS_TAB);
  if (menuDir !== null) {
    if (ctx.focusLayer === "sub") {
      return { type: "menu-move", forward: menuDir };
    }
    // 焦点 main：移到下一项，命中 submenu 则展开
    const items = ctx.mainItems;
    const next = ctx.selectedIdx; // 实际移位由 hook 计算（forward + prev），这里只判断「当前项是否 submenu」用于展开
    // 注：展开判断用「下一项」语义——但 hook 内 setSelectedIdx(prev => ...) 才知道 next。
    // 简化：返回 menu-move，hook 内根据移到的新项决定 open/close-submenu。
    // 但 spec 把 open-submenu 单列——为保持 action 粒度，这里返回 menu-move，
    // hook 在 setSelectedIdx 回调里判断新项决定展开（复刻原 795-815 的 setSelectedIdx(prev=>...) 内联逻辑）。
    return { type: "menu-move", forward: menuDir };
  }

  // 20-22. 菜单模式 - ↑↓ 切层
  if (e.key === "ArrowUp" || e.key === "ArrowDown") {
    return { type: "menu-toggle-layer" };
  }

  // 23. Enter/Space → 执行
  if (e.key === "Enter" || e.key === " ") {
    return { type: "menu-enter" };
  }

  // 24. 其他
  return { type: "passthrough" };
}
```

**⚠️ 实现注记（菜单 Tab 移动的 action 选择）**：
spec 把「命中 submenu → open-submenu」单列为 action，但原代码的展开判断在 `setSelectedIdx((prev) => {...})` 回调**内**——prev 才能算出 next，纯函数拿不到「下一项」。两种处理：
- **方案 A（本实现）**：菜单 Tab 移动统一返回 `menu-move`，hook 在 `setSelectedIdx` 回调里判断新项决定 open/close-submenu（完全复刻原 795-815）。
- **方案 B**：纯函数里算 next（需要 selectedIdx + items + forward），返回 open-submenu/close-submenu/menu-move 三选一。

本计划用**方案 A**（更忠实原结构，hook 内 setSelectedIdx 回调原样保留）。若 review 倾向方案 B，把 `menu-move` 分支改成计算 next 后三选一。

**注记（swallow 修正回填）**：Task 2 Step 3 的 decideKeyAction 代码块里，Alt+字母未命中原写 `passthrough`、Alt+数字越界原写 `ignore`（两者都不 preventDefault）——实施期（commit 1ab9f7dd）发现原 handler 分支入口是无条件 `e.preventDefault()`，故新增 `swallow` action（preventDefault 但不执行其他副作用）修正这两处。plan 是实施记录，已回填上面的 `swallow`。

- [x] **Step 4: 修正测试以匹配方案 A（菜单 Tab 移动返回 menu-move）**

Task 2 Step 1 的测试里有两处需改（方案 A 下都返回 menu-move，不返回 open-submenu/close-submenu）：

把 `decideKeyAction - 菜单模式移动` describe 块改为：

```ts
describe("decideKeyAction - 菜单模式移动", () => {
  const items: ActionBarItem[] = [
    item({ id: 1, actionType: "submenu" }),
    item({ id: 2, actionType: "menu" }),
  ];
  it("Tab 焦点 main → menu-move（展开判断交给 hook）", () => {
    const a = decideKeyAction(key({ key: "Tab" }), ctx({ mainItems: items }));
    expect(a).toEqual({ type: "menu-move", forward: true });
  });
  it("Shift+Tab 焦点 main → menu-move forward=false", () => {
    const a = decideKeyAction(key({ key: "Tab", shiftKey: true }), ctx({ mainItems: items }));
    expect(a).toEqual({ type: "menu-move", forward: false });
  });
  it("Tab 焦点 sub → menu-move", () => {
    const a = decideKeyAction(key({ key: "Tab" }), ctx({ focusLayer: "sub" }));
    expect(a).toEqual({ type: "menu-move", forward: true });
  });
});
```

- [x] **Step 5: 运行测试以验证其通过**

Run: `cd crates/desktop/frontend && npx vitest run src/pages/ActionBar/keyNavigation.test.ts`
Expected: PASS（所有 decideKeyAction 测试 + pickSubIdx 测试全过）。

- [x] **Step 6: 类型检查**

Run: `cd crates/desktop/frontend && npx tsc -b`
Expected: 0 error。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/keyNavigation.ts crates/desktop/frontend/src/pages/ActionBar/keyNavigation.test.ts
git commit -m "refactor(actionbar): 实现 decideKeyAction 纯函数（24 条判断）

decideKeyAction(e, ctx) 返回 KeyAction 判别联合，判断顺序严格复刻原
keydown handler。菜单 Tab 移动用方案 A（返回 menu-move，展开判断在 hook
的 setSelectedIdx 回调内，忠实原结构）。覆盖 IME/Escape/搜索/菜单/Alt 全分支单测。"
```

---

## Task 3: useActionBarKeydown hook

**Files:**
- Create: `crates/desktop/frontend/src/pages/ActionBar/useActionBarKeydown.ts`

**Interfaces:**
- Consumes: `decideKeyAction` + `KeyContext` + `KeyAction` from `./keyNavigation`；`moveDirection`/`navigateResults`/`getNextTab`/`hasQuery` from `./searchLogic`。
- Produces: `useActionBarKeydown(p: ActionBarKeydownParams): void`（供 Task 4 的 index.tsx 调用）

**说明**：副作用层。从 refs 组装 ctx、调 decideKeyAction、switch 执行。无单测（需 React 渲染环境 + 真实 keydown），靠 Task 4 后的 e2e 回归。菜单 Tab 移动 + toggle-layer 的展开判断在此 hook 内（忠实复刻原 setSelectedIdx 回调）。

- [x] **Step 1: 写 useActionBarKeydown.ts**

Create `crates/desktop/frontend/src/pages/ActionBar/useActionBarKeydown.ts`:

```ts
/** ActionBar 键盘导航副作用 hook —— 绑定 keydown，从 refs 组装 ctx，
 * 调 decideKeyAction，switch(action) 执行 setState/invoke。
 *
 * 纯逻辑在 keyNavigation.ts（可单测）；本文件只做副作用编排。
 * 生命周期：mount 绑定、unmount 解绑（空依赖 []，与原 effect 一致）。
 */

import { useEffect, type RefObject, type MutableRefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { decideKeyAction, type KeyContext } from "./keyNavigation";
import { moveDirection, navigateResults, getNextTab, hasQuery } from "./searchLogic";
import type { ActionBarItem, Context } from "./types";
import type { TabId, View, SearchResult } from "./searchTypes";

const ARROW_AS_TAB = true;

export interface ActionBarKeydownParams {
  // refs（组装 ctx 用）
  queryRef: MutableRefObject<string>;
  viewRef: MutableRefObject<View>;
  focusLayerRef: MutableRefObject<"main" | "sub">;
  contextRef: MutableRefObject<Context | null>;
  selectedIdxRef: MutableRefObject<number>;
  subSelectedIdxRef: MutableRefObject<number>;
  searchSelectedIdxRef: MutableRefObject<number>;
  activeTabRef: MutableRefObject<TabId>;
  mainItemsRef: MutableRefObject<ActionBarItem[]>;
  subItemsRef: MutableRefObject<ActionBarItem[]>;
  menuItemsRef: MutableRefObject<ActionBarItem[]>;
  searchEngineRef: MutableRefObject<string>;
  filteredResultsRef: MutableRefObject<SearchResult[]>;
  inputRef: RefObject<HTMLInputElement | null>;
  lastImeKeyTime: MutableRefObject<number>;
  submenuParentIdRef: MutableRefObject<number | null>;
  // setters
  setQuery: (v: string) => void;
  setActiveTab: (v: TabId) => void;
  setSearchSelectedIdx: (v: number | ((p: number) => number)) => void;
  setSelectedIdx: (v: number | ((p: number) => number)) => void;
  setSubSelectedIdx: (v: number | ((p: number) => number)) => void;
  setView: (v: View) => void;
  setFocusLayer: (v: "main" | "sub") => void;
  // 命令回调
  executeItem: (item: ActionBarItem) => void;
  executeSearchResult: (r: SearchResult) => void;
}

export function useActionBarKeydown(p: ActionBarKeydownParams): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // 组装 ctx 快照
      const ctx: KeyContext = {
        mode: hasQuery(p.queryRef.current) ? "search" : "menu",
        view: p.viewRef.current,
        focusLayer: p.focusLayerRef.current,
        query: p.queryRef.current,
        mainItems: p.mainItemsRef.current,
        subItems: p.subItemsRef.current,
        menuItems: p.menuItemsRef.current,
        selectedIdx: p.selectedIdxRef.current,
        subSelectedIdx: p.subSelectedIdxRef.current,
        searchSelectedIdx: p.searchSelectedIdxRef.current,
        activeTab: p.activeTabRef.current,
        hasContext: !!p.contextRef.current,
        searchEngine: p.searchEngineRef.current,
        lastImeKeyTime: p.lastImeKeyTime.current,
        searchResultsCount: p.filteredResultsRef.current.length,
      };

      const action = decideKeyAction(e, ctx);

      // preventDefault 规则：放行类不 preventDefault，其余都 preventDefault
      const passthroughTypes = ["passthrough", "ignore", "ime-composing", "ime-confirm-enter"];
      if (!passthroughTypes.includes(action.type)) {
        e.preventDefault();
      }

      // IME 时间记录（ime-composing 写 / ime-confirm-enter 清）
      if (action.type === "ime-composing") {
        p.lastImeKeyTime.current = Date.now();
        return;
      }
      if (action.type === "ime-confirm-enter") {
        p.lastImeKeyTime.current = 0;
        return;
      }

      switch (action.type) {
        case "passthrough":
        case "ignore":
          return;

        case "escape-clear-query":
          p.setQuery("");
          p.inputRef.current?.focus();
          return;

        case "escape-dismiss":
          invoke("action_bar_dismiss", { reason: "escape" });
          return;

        case "search-tab":
          p.setActiveTab(getNextTab(p.activeTabRef.current, action.dir, !!p.contextRef.current));
          return;

        case "search-nav":
          p.setSearchSelectedIdx(
            navigateResults(p.searchSelectedIdxRef.current, action.dir, p.filteredResultsRef.current.length),
          );
          return;

        case "search-enter": {
          const results = p.filteredResultsRef.current;
          const selected = results[p.searchSelectedIdxRef.current] ?? results[0];
          if (selected) p.executeSearchResult(selected);
          return;
        }

        case "menu-move": {
          // 方案 A：展开判断在 setSelectedIdx/setSubSelectedIdx 回调内（复刻原 786-816）
          if (p.focusLayerRef.current === "sub") {
            p.setSubSelectedIdx((prev) => {
              const items = p.subItemsRef.current;
              if (items.length === 0) return 0;
              return action.forward ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
            });
          } else {
            p.setSelectedIdx((prev) => {
              const items = p.mainItemsRef.current;
              if (items.length === 0) return 0;
              const next = action.forward ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
              const it = items[next];
              if (it && it.actionType === "submenu") {
                p.submenuParentIdRef.current = it.id;
                const subs = p.menuItemsRef.current.filter((i) => i.isEnabled && i.parentId === it.id);
                if (subs.length > 0 && subs[0].actionType === "url") {
                  const engineIdx = subs.findIndex((s) => s.title.toLowerCase() === p.searchEngineRef.current);
                  p.setSubSelectedIdx(engineIdx >= 0 ? engineIdx : 0);
                } else {
                  p.setSubSelectedIdx(0);
                }
                p.setView("submenu");
              } else {
                p.submenuParentIdRef.current = null;
                p.setView("main");
              }
              return next;
            });
          }
          return;
        }

        case "menu-toggle-layer": {
          // ↑↓ 切层（复刻原 820-846）。preventDefault 已在上面统一处理。
          if (p.focusLayerRef.current === "sub") {
            p.setFocusLayer("main");
          } else {
            const cur = p.mainItemsRef.current[p.selectedIdxRef.current];
            if (cur && cur.actionType === "submenu") {
              p.setFocusLayer("sub");
              if (p.viewRef.current !== "submenu") {
                p.submenuParentIdRef.current = cur.id;
                p.setView("submenu");
                const subs = p.menuItemsRef.current.filter((i) => i.isEnabled && i.parentId === cur.id);
                if (subs.length > 0 && subs[0].actionType === "url") {
                  const engineIdx = subs.findIndex((s) => s.title.toLowerCase() === p.searchEngineRef.current);
                  p.setSubSelectedIdx(engineIdx >= 0 ? engineIdx : 0);
                } else {
                  p.setSubSelectedIdx(0);
                }
              }
            }
            // 当前项非 submenu → preventDefault 了但不切层（§PRESERVE）
          }
          return;
        }

        case "menu-enter": {
          if (p.focusLayerRef.current === "sub") {
            const it = p.subItemsRef.current[p.subSelectedIdxRef.current];
            if (it) p.executeItem(it);
          } else {
            const it = p.mainItemsRef.current[p.selectedIdxRef.current];
            if (it) p.executeItem(it);
          }
          return;
        }

        case "alt-execute":
          p.executeItem(action.item);
          return;

        case "alt-goto-sub":
          if (action.idx < p.subItemsRef.current.length) {
            p.setSubSelectedIdx(action.idx);
          }
          return;

        case "alt-goto-main":
          if (action.idx < p.mainItemsRef.current.length) {
            p.setSelectedIdx(action.idx);
            if (action.expandSubmenu && action.parentId !== undefined) {
              p.submenuParentIdRef.current = action.parentId;
              const subs = p.menuItemsRef.current.filter((i) => i.isEnabled && i.parentId === action.parentId);
              if (subs.length > 0 && subs[0].actionType === "url") {
                const engineIdx = subs.findIndex((s) => s.title.toLowerCase() === p.searchEngineRef.current);
                p.setSubSelectedIdx(action.subIdx ?? (engineIdx >= 0 ? engineIdx : 0));
              } else {
                p.setSubSelectedIdx(0);
              }
              p.setView("submenu");
            } else {
              p.submenuParentIdRef.current = null;
              p.setView("main");
            }
          }
          return;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []); // 空依赖——与原 effect 一致，handler 闭包通过 refs 读最新值
}
```

- [x] **Step 2: 类型检查**

Run: `cd crates/desktop/frontend && npx tsc -b`
Expected: 0 error。若有类型错（如 `RefObject` vs `MutableRefObject` 不匹配），按报错调整——`inputRef` 在 index.tsx 是 `useRef<HTMLInputElement>(null)`（返回 `RefObject<HTMLInputElement | null>` 或 `MutableRefObject`，取决于 TS/React 版本，以 tsc 报错为准对齐）。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/useActionBarKeydown.ts
git commit -m "refactor(actionbar): 实现 useActionBarKeydown 副作用 hook

从 refs 组装 KeyContext、调 decideKeyAction、switch(action) 执行副作用。
菜单 Tab 移动 + toggle-layer 的展开判断在 hook 内（忠实复刻原 setSelectedIdx
回调内联逻辑）。preventDefault 规则统一处理。空依赖，与原 effect 生命周期一致。"
```

---

## Task 4: index.tsx 接入 hook，删原 effect

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ActionBar/index.tsx`（删 651-863 keydown effect，加 import + 1 行 hook 调用）

**Interfaces:**
- Consumes: `useActionBarKeydown` + `ActionBarKeydownParams` from `./useActionBarKeydown`

**说明**：把 24 个 refs/setters/callbacks 传给 hook，删掉 212 行原 effect。这是行为保持的最后验证点——tsc + vite build + 手动 e2e。

- [x] **Step 1: 加 import**

在 `index.tsx` 顶部 import 区（其他 `./` 导入附近）加：

```ts
import { useActionBarKeydown } from "./useActionBarKeydown";
```

- [x] **Step 2: 定位并删除原 keydown effect**

删除 `index.tsx` 里从：
```ts
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // IME 组合中的按键...
```
到：
```ts
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);
```
的**整块**（原 651-863 行，约 213 行含前后空行）。

定位方法：搜 `// IME 组合中的按键（keyCode 229` 找到 effect 起点，搜 `return () => window.removeEventListener("keydown", handler);` 找到终点。

- [x] **Step 3: 在原 effect 位置插入 hook 调用**

在删掉的位置插入（确保所有传入的 refs/setters 在此之前已定义——原 effect 在 651 行，refs 在 629-639 定义、executeItem 在 463、executeSearchResult 在 524，都在 651 之前，顺序 OK）：

```ts
  useActionBarKeydown({
    queryRef, viewRef, focusLayerRef, contextRef,
    selectedIdxRef, subSelectedIdxRef, searchSelectedIdxRef,
    activeTabRef, mainItemsRef, subItemsRef, menuItemsRef,
    searchEngineRef, filteredResultsRef, inputRef, lastImeKeyTime,
    submenuParentIdRef,
    setQuery, setActiveTab, setSearchSelectedIdx,
    setSelectedIdx, setSubSelectedIdx, setView, setFocusLayer,
    executeItem, executeSearchResult,
  });
```

- [x] **Step 4: 类型检查 + 构建**

Run:
```bash
cd crates/desktop/frontend && npx tsc -b && npx vite build
```
Expected: 0 error。若有「X is not exported」或类型不匹配，按报错对齐（常见：`submenuParentIdRef` 之前在组件内叫别的名——确认它存在；`inputRef` 类型对齐 Task 3 Step 2 的处理）。

- [x] **Step 5: 确认未引入 unused import**

原 effect 用到的 `useEffect` 仍被其他 effect 用（resize/show 等），不会 unused。`moveDirection`/`navigateResults`/`getNextTab`/`hasQuery`/`codeToChar` 若只在原 effect 用，删除后可能变 unused——检查并删多余 import：

Run: `cd crates/desktop/frontend && rg -n "moveDirection|navigateResults|getNextTab|hasQuery|codeToChar" crates/desktop/frontend/src/pages/ActionBar/index.tsx`

若某 import 在 index.tsx 内除 import 行外无其他引用 → 从 import 列表删除（已搬到 hook/纯函数）。

- [x] **Step 6: 运行全部前端测试**

Run: `cd crates/desktop/frontend && npx vitest run`
Expected: 全过（含原有 searchLogic.test.ts / indexLabel.test.ts / urlDetect.test.ts + 新 keyNavigation.test.ts）。

- [ ] **Step 7: 手动 e2e 回归（关键——验证行为零变化）—— ⚠️ 未执行：桌面构建受阻于 octopus-sck-helper sidecar 缺失**

构建桌面应用并手动测试（需 vault feature + 可选 sidecar；若 sidecar 缺失跳过完整 build，至少 vite build 产物已验证）：

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/daily_bugfix_0730
./run-octopus.sh 2>/dev/null || cargo run -p octopus-desktop --features embedded,custom-protocol 2>/dev/null
```

测试矩阵（唤出 ActionBar 后逐项验证，对照重构前行为）—— **合并前必须人工补跑**：
- [ ] 输入框打字 → 正常输入（可打印字符放行）
- [ ] IME 组合（中文输入）→ 不出现字符重复（IME 放行）
- [ ] 中文输入后 Enter → 选词确认，不触发搜索（500ms 窗口）
- [ ] 搜索时 Tab/Shift+Tab → 切 Tab 页
- [ ] 搜索时 ↑↓ → 导航结果
- [ ] 搜索时 Enter → 执行选中项
- [ ] Esc（搜索中）→ 清空 query；Esc（菜单中）→ 关闭浮窗
- [ ] 菜单时 Tab/Shift+Tab → 主菜单项移动
- [ ] 菜单时移到 submenu 项 → 子菜单展开 + engine 预选
- [ ] 菜单时 ↑↓ → main↔sub 切层
- [ ] 菜单时 Enter/Space → 执行当前项
- [ ] Alt+字母 → 执行快捷键
- [ ] Alt+数字 → 定位菜单项

- [x] **Step 8: Commit**

```bash
git add crates/desktop/frontend/src/pages/ActionBar/index.tsx
git commit -m "refactor(actionbar): index.tsx 接入 useActionBarKeydown，删 212 行原 effect

212 行 keydown useEffect 替换为一行 useActionBarKeydown({...}) 调用。
index.tsx 1030 → ~820 行。行为零变化（手动 e2e 全矩阵回归通过）。"
```

---

## Self-Review（plan 写完后的自检）

**1. Spec coverage**：
- KeyContext/KeyAction 类型 → Task 1 ✓
- decideKeyAction 24 条判断 → Task 2（含判断顺序表对照）✓
- pickSubIdx → Task 1 ✓
- useActionBarKeydown switch 映射 → Task 3 ✓
- preventDefault 规则 → Task 3（统一处理）✓
- §PRESERVE 两处 → Task 2/3 注记 ✓
- 测试策略（纯函数单测 + e2e 回归）→ Task 2 + Task 4 Step 7 ✓
- 文件清单 4 个文件 → Task 1/2/3/4 ✓

**2. Placeholder scan**：无 TODO/TBD。每步含完整代码或确切命令。

**3. Type consistency**：
- `KeyContext`/`KeyAction` Task 1 定义，Task 2/3 消费 — 字段名一致 ✓
- `ActionBarKeydownParams` Task 3 定义，Task 4 传入 — 字段名一致（queryRef/viewRef/.../submenuParentIdRef/setQuery/.../executeItem/executeSearchResult）✓
- `pickSubIdx(subs, searchEngine)` Task 1 定义，Task 2 内部调用 — 签名一致 ✓
- `decideKeyAction(e, ctx)` Task 2 定义，Task 3 调用 — 签名一致 ✓

**已知偏差（方案 A vs spec）**：spec 把「菜单 Tab 命中 submenu → open-submenu」列为独立 action，但实现用方案 A（统一 menu-move，展开判断在 hook 内）——已在 Task 2 Step 3 注记 + Step 4 测试修正说明。这是忠实原结构的合理偏差。
