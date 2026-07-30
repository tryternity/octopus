# ActionBar keydown 抽取重构 — 设计规格

- **日期**：2026-07-30
- **类型**：重构（行为保持，零功能变化）
- **范围**：`crates/desktop/frontend/src/pages/ActionBar/index.tsx` 的 keydown `useEffect`（652-863，212 行）
- **动机**：单块巨型 effect 把「按键判断逻辑」和「副作用执行」耦合，逻辑无法单测；改一个分支要读完整段。

## 目标与非目标

**目标**：
1. 把 212 行 keydown effect 拆成**纯逻辑层 + 副作用层**，纯逻辑可独立单测
2. 降低 `index.tsx` 体积（预计 1030 → ~820）
3. 行为零变化——所有按键响应与重构前逐条对应

**非目标**：
- 不改 resize effect（154-222）、show/refresh effect（226-278）、密码生成器按钮等其他逻辑
- 不引入新功能、不改快捷键行为、不调整 UI
- 不迁移到 useReducer（会波及搜索/resize/show 等所有 effect，超出范围）

## 现状（重构前）

```
index.tsx (1030 行)
└─ keydown useEffect (212 行, 652-863)
    ├─ 公共前置：IME 放行 / Escape / loading 拦截 / 可打印字符放行
    ├─ 搜索模式分支（query 非空）：Tab 切页 / ↑↓ 导航 / Enter 执行
    └─ 菜单模式分支（query 空）：Alt+字母/数字 / Tab 移动 / ↑↓ 切层 / Enter / 子菜单展开
```

闭包捕获 **24 个外部标识符**：13 个 ref（`queryRef`/`viewRef`/`focusLayerRef`/`selectedIdxRef`/`subSelectedIdxRef`/`mainItemsRef`/`subItemsRef`/`menuItemsRef`/`activeTabRef`/`searchSelectedIdxRef`/`searchEngineRef`/`inputRef`/`lastImeKeyTime`/`filteredResultsRef`/`contextRef`）+ 8 个 setState（`setQuery`/`setActiveTab`/`setSearchSelectedIdx`/`setSelectedIdx`/`setSubSelectedIdx`/`setView`/`setFocusLayer`）+ 2 个 callback（`executeItem`/`executeSearchResult`）+ `invoke`。

## 架构（重构后）

```
keyNavigation.ts          ← 新增：纯逻辑层（无 React/DOM 依赖，可单测）
├─ KeyContext type        纯函数输入（模式 + 当前选中 + items + searchEngine + IME 时间）
├─ KeyAction type         纯函数输出（判别联合，12 个动作类型）
└─ decideKeyAction(e, ctx)  核心：给定 keyevent + ctx → 返回 KeyAction

useActionBarKeydown.ts    ← 新增：副作用层 hook（绑定 + 执行 action）
└─ useActionBarKeydown(params)  从 refs 组装 ctx → 调 decideKeyAction → switch 执行

index.tsx (≈ 820 行)
└─ useActionBarKeydown({ ...refs, ...setters, ...callbacks, ... })
```

**分层原则**（沿用现有 `searchLogic.ts` 已确立的模式——顶部 doc 注释 + 类型 import + 每函数 JSDoc + 无 React/DOM 依赖）：
- **纯逻辑层** `keyNavigation.ts`：只「决定做什么」。输入 `KeyContext` + `KeyboardEvent`，输出 `KeyAction`。无 `useState`/`useRef`/`addEventListener`/`invoke`。**完全可单测**。
- **副作用层** `useActionBarKeydown.ts`：mount 时 `addEventListener`，回调内从 refs 读出 ctx、调 `decideKeyAction`、根据 action 做 setState/invoke，unmount 时 `removeEventListener`。

## 核心类型

```ts
// keyNavigation.ts

/**
 * 纯函数输入——从 refs 同步读取的上下文快照。无 React 依赖。
 *
 * 设计要点：items 传「已过滤可见+启用」的列表（mainItemsRef/subItemsRef
 * 在 index.tsx 已维护），而非原始 menuItems——纯函数不做过滤，只做决策。
 */
export interface KeyContext {
  mode: "search" | "menu";           // hasQuery(query) 判定
  view: View;                        // viewRef.current（含 "loading"）
  focusLayer: "main" | "sub";        // focusLayerRef.current
  query: string;
  /** 主菜单项（已过滤 isVisible + isEnabled），来自 mainItemsRef */
  mainItems: ActionBarItem[];
  /** 子菜单项（已过滤），来自 subItemsRef */
  subItems: ActionBarItem[];
  /** 全量菜单项（Alt+字母快捷键匹配 + submenu 子项展开计算用），来自 menuItemsRef */
  menuItems: ActionBarItem[];
  selectedIdx: number;               // selectedIdxRef
  subSelectedIdx: number;            // subSelectedIdxRef
  searchSelectedIdx: number;         // searchSelectedIdxRef
  activeTab: TabId;                  // activeTabRef
  hasContext: boolean;               // !!contextRef.current
  searchEngine: string;              // searchEngineRef
  /** IME 最后按键时刻（Date.now()）；Enter 500ms 内 = 选词确认窗口 */
  lastImeKeyTime: number;
  /** 结果列表长度（搜索模式 ↑↓/Enter 用），来自 filteredResultsRef.current.length */
  searchResultsCount: number;
}

/**
 * 纯函数输出——判别联合，枚举所有键盘动作。
 *
 * preventDefault 规则（hook 执行时统一应用）：
 *   - passthrough / ignore / ime-composing / ime-confirm-enter → 不 preventDefault（放行给 input）
 *   - 其余所有 action → preventDefault（handler 消费了该键）
 */
export type KeyAction =
  | { type: "ime-composing" }            // keyCode 229 / isComposing → 记录 IME 时间，放行
  | { type: "ime-confirm-enter" }        // Enter 在 IME 后 500ms 内 → 选词确认，放行
  | { type: "passthrough" }              // 无修饰可打印字符 → 放行给 input
  | { type: "ignore" }                   // loading 视图，不处理
  | { type: "escape-clear-query" }       // 搜索中 Esc → 清 query + focus input
  | { type: "escape-dismiss" }           // 菜单中 Esc → invoke("action_bar_dismiss")
  | { type: "search-tab"; dir: 1 | -1 }
  | { type: "search-nav"; dir: 1 | -1 }
  | { type: "search-enter" }
  | { type: "menu-move"; forward: boolean }
  | { type: "menu-toggle-layer" }        // ↑↓ 在 main↔sub 间切层（可能触发 submenu 展开）
  | { type: "menu-enter" }
  | { type: "open-submenu"; parentId: number; subIdx: number }  // 移到 submenu 项 / Alt+数字命中 submenu
  | { type: "close-submenu" }            // 移到非 submenu 项 → 收起子菜单回 main
  | { type: "alt-execute"; item: ActionBarItem }  // Alt+字母命中快捷键
  | { type: "alt-goto-sub"; idx: number }         // Alt+数字，焦点在 sub 层
  | { type: "alt-goto-main"; idx: number; expandSubmenu: boolean; parentId?: number; subIdx?: number };
```

## `decideKeyAction` 判断顺序（严格复刻现有 handler）

以下顺序与现有 handler（652-860）逐条对应，**不可调换**：

| # | 条件 | 返回 | 对应原行 |
|---|------|------|---------|
| 1 | `e.keyCode === 229 \|\| e.isComposing` | `ime-composing` | 657 |
| 2 | `e.key === "Enter" && Date.now() - ctx.lastImeKeyTime < 500` | `ime-confirm-enter` | 662 |
| 3 | `e.key === "Escape" && mode === "search"` | `escape-clear-query` | 668-672 |
| 4 | `e.key === "Escape" && mode === "menu"` | `escape-dismiss` | 673-675 |
| 5 | `ctx.view === "loading"` | `ignore` | 680 |
| 6 | 无修饰(`!alt && !meta && !ctrl`) 且 `e.key` 不在 navKeys | `passthrough` | 686-693 |
| 7 | `mode === "search"` 且 `moveDirection !== null` | `search-tab` | 702-707 |
| 8 | `mode === "search"` 且 `ArrowDown` | `search-nav {dir:1}` | 710 |
| 9 | `mode === "search"` 且 `ArrowUp` | `search-nav {dir:-1}` | 715 |
| 10 | `mode === "search"` 且 `Enter` | `search-enter` | 722 |
| 11 | `mode === "search"` 其他 | `passthrough`（兜底） | 730 |
| 12 | `e.altKey` 且 `codeToChar` 是 `[a-z]` 且匹配到 item | `alt-execute {item}` | 742-746 |
| 13 | `e.altKey` 且 `codeToChar` 是 `[1-9]` 且焦点在 sub | `alt-goto-sub {idx}` | 752-753 |
| 14 | `e.altKey` 且 `[1-9]` 且焦点在 main，命中 submenu 项 | `alt-goto-main {idx, expandSubmenu:true, parentId, subIdx}` | 755-768 |
| 15 | `e.altKey` 且 `[1-9]` 且焦点在 main，命中非 submenu 项 | `alt-goto-main {idx, expandSubmenu:false}` | 769-772 |
| 16 | `e.altKey` 其他 | `passthrough`（原 `return`，无 preventDefault）→ 见不变量 §IMPROVE | 778 |
| 17 | 菜单模式 `moveDirection !== null`，焦点 sub | `menu-move`（hook 内循环移 subSelectedIdx） | 786-792 |
| 18 | 菜单模式 `moveDirection !== null`，焦点 main，命中 submenu | `open-submenu {parentId, subIdx}`（hook 内 setSelectedIdx + 展开） | 800-809 |
| 19 | 菜单模式 `moveDirection !== null`，焦点 main，命中非 submenu | `close-submenu`（hook 内 setSelectedIdx + 收起） | 810-813 |
| 20 | `ArrowUp/Down`，焦点 sub | `menu-toggle-layer`（→ main） | 824-825 |
| 21 | `ArrowUp/Down`，焦点 main，当前项是 submenu | `menu-toggle-layer`（→ sub，hook 内按需展开） | 828-843 |
| 22 | `ArrowUp/Down`，焦点 main，当前项非 submenu | `ignore`（原无 preventDefault? 见不变量） | — |
| 23 | `Enter` 或 `Space` | `menu-enter` | 848-859 |
| 24 | 其他 | `passthrough` | — |

**不变量 §PRESERVE（行为对齐歧义点，重构保持原样）**：

两处原行为略显怪异，但重构**严格保持**，不借机「修正」（修正 = 行为变化 = 违反重构前提）。如需改记为独立 bug：

- **行 778**：`e.altKey` 但 `codeToChar` 非 `[a-z]`/`[1-9]`（如 Alt+符号、Alt 无 codeToChar）→ 原 `return` **无 preventDefault** → `passthrough`。意味 Alt+这类键会放行给 input/浏览器（可能触发浏览器 Alt 菜单）。保持。
- **行 820-846 ↑↓**：原代码在 `if (e.key === "ArrowUp" || e.key === "ArrowDown")` 入口**无条件 `e.preventDefault()`**，然后才判断是否切层。焦点在 main 且当前项非 submenu 时 → preventDefault 了但**什么都不做**（不切层、不移动）。重构中 `menu-toggle-layer` **始终 preventDefault**（符合 action 消费即 preventDefault 规则），hook 内按「当前项是否 submenu」决定是否实际切层（非 submenu → no-op）。

## 子菜单 engineIdx 预选算法（纯函数内完成）

现有代码在 4 处重复同一段「移到/展开 submenu 项时算 subSelectedIdx」（行 761-768、802-809、835-842、753 区域）。抽成纯函数：

```ts
/**
 * 给定 submenu 的子项列表 + 当前搜索引擎，算初始 subSelectedIdx。
 * 规则：子项首项是 url 类型时，按 title(小写) 匹配 searchEngine；
 *       匹配不到或首项非 url → 0。
 * 严格复刻 index.tsx 行 762-767 / 803-808 / 836-841 的逻辑。
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

`decideKeyAction` 在返回 `open-submenu` / `alt-goto-main{expandSubmenu}` 时，内部用 `pickSubIdx` 算好 `subIdx` 一并返回——hook 不重复算。

## 副作用层 `useActionBarKeydown` 契约

```ts
// useActionBarKeydown.ts
export interface ActionBarKeydownParams {
  // refs（同步读取，组装 KeyContext）
  queryRef: RefObject<string>;
  viewRef: RefObject<View>;
  focusLayerRef: RefObject<"main" | "sub">;
  contextRef: RefObject<Context | null>;
  selectedIdxRef: RefObject<number>;
  subSelectedIdxRef: RefObject<number>;
  searchSelectedIdxRef: RefObject<number>;
  activeTabRef: RefObject<TabId>;
  mainItemsRef: RefObject<ActionBarItem[]>;
  subItemsRef: RefObject<ActionBarItem[]>;
  menuItemsRef: RefObject<ActionBarItem[]>;
  searchEngineRef: RefObject<string>;
  filteredResultsRef: RefObject<SearchHit[]>;
  inputRef: RefObject<HTMLInputElement>;
  lastImeKeyTime: MutableRefObject<number>;
  // setters
  setQuery, setActiveTab, setSearchSelectedIdx,
  setSelectedIdx, setSubSelectedIdx, setView, setFocusLayer;
  // 命令回调
  executeItem: (item: ActionBarItem) => void;
  executeSearchResult: (r: SearchHit) => void;
}

export function useActionBarKeydown(p: ActionBarKeydownParams): void;
```

**hook 内 `switch (action.type)` 执行映射**（每个 action 对应的副作用，严格复刻原 handler）：

| action | 副作用 |
|--------|--------|
| `ime-composing` | `lastImeKeyTime.current = Date.now()`（记 IME 时间） |
| `ime-confirm-enter` | `lastImeKeyTime.current = 0`（清窗口） |
| `escape-clear-query` | `setQuery("")` + `inputRef.current?.focus()` |
| `escape-dismiss` | `invoke("action_bar_dismiss", { reason: "escape" })` |
| `search-tab` | `setActiveTab(getNextTab(ctx.activeTab, dir, ctx.hasContext))` |
| `search-nav` | `setSearchSelectedIdx(navigateResults(ctx.searchSelectedIdx, dir, ctx.searchResultsCount))` |
| `search-enter` | 取 `filteredResultsRef[searchSelectedIdx] ?? [0]`，`executeSearchResult` |
| `menu-move`（焦点 sub） | `setSubSelectedIdx` 循环移位（复刻 788-792） |
| `menu-move`（焦点 main） | `setSelectedIdx` 循环移位（复刻 795-815） |
| `open-submenu` | `submenuParentIdRef = parentId` + `setSubSelectedIdx(subIdx)` + `setView("submenu")` |
| `close-submenu` | `submenuParentIdRef = null` + `setView("main")` |
| `menu-toggle-layer` | sub→main: `setFocusLayer("main")`；main→sub 且当前项 submenu: `setFocusLayer("sub")` + 按需 `setView("submenu")`（复刻 832-842） |
| `menu-enter` | 按 focusLayer 取 mainItems/subItems 对应项，`executeItem` |
| `alt-execute` | `executeItem(item)` |
| `alt-goto-sub` | `setSubSelectedIdx(idx)`（边界：idx < subItems.length） |
| `alt-goto-main` | `setSelectedIdx(idx)`；expandSubmenu → open-submenu 副作用，否则 close-submenu |

> 注：`submenuParentIdRef` 是 index.tsx 组件内的 ref（控制子菜单展开哪个父项），hook 通过 params 接收并直接写——它不是 React state，写 ref 不触发重渲染，与原行为一致。

## 不变量（重构必须保持）

1. **行为零变化**——`decideKeyAction` 的 24 条判断顺序、preventDefault 时机、IME 放行规则、500ms 确认窗口、子菜单 engineIdx 预选，逐条对应原 handler。
2. **preventDefault 规则**：`passthrough` / `ignore` / `ime-composing` / `ime-confirm-enter` 不 preventDefault；其余 action 都 preventDefault。hook 在 `decideKeyAction` 返回后统一处理（action 命中即 preventDefault，放行类不调）。
3. **`addEventListener` 生命周期不变**：mount 一次、空依赖 `[]`、unmount 时 `removeEventListener`。
4. **24 个捕获变量全部经 params 传入**，无遗漏、无新增全局。
5. **`KeyAction` 完备**：handler 每个 `return` 分支必须映射到某个 action，无遗漏分支。
6. **`lastImeKeyTime` 写时机不变**：`ime-composing` 时写 `Date.now()`；`ime-confirm-enter` 时清 0。

## 测试策略

### 纯函数单测（`keyNavigation.test.ts`）— 重构最大收益

对 `decideKeyAction` 每条判断构造 `KeyContext` + 模拟 `KeyboardEvent`，断言返回的 `KeyAction`。覆盖矩阵：

- **公共前置**：IME 组合中（229/isComposing）→ `ime-composing`；Enter 在 500ms 内/外 → `ime-confirm-enter`/`search-enter`；loading 视图 → `ignore`；无修饰可打印字符 → `passthrough`
- **Escape 双语义**：search 模式 → `escape-clear-query`；menu 模式 → `escape-dismiss`
- **搜索模式**：Tab/Shift+Tab → `search-tab {dir}`；↑↓ → `search-nav`；Enter → `search-enter`
- **菜单模式移动**：Tab 焦点 main 命中 submenu → `open-submenu`；命中非 submenu → `close-submenu`；焦点 sub → `menu-move`
- **菜单模式切层**：↑↓ 焦点 sub → `menu-toggle-layer`(→main)；焦点 main submenu 项 → `menu-toggle-layer`(→sub)；焦点 main 非 submenu → `menu-toggle-layer`(no-op)
- **Alt 快捷键**：Alt+[a-z] 命中 → `alt-execute`；未命中 → `passthrough`；Alt+[1-9] 焦点 sub → `alt-goto-sub`；焦点 main submenu → `alt-goto-main{expandSubmenu:true}`；非 submenu → `alt-goto-main{expandSubmenu:false}`
- **`pickSubIdx`**：首项 url 且 engine 匹配 → 返回匹配 idx；首项 url 不匹配 → 0；首项非 url → 0；空列表 → 0

模拟 `KeyboardEvent` 用原生 `new KeyboardEvent(...)`（Vitest jsdom 支持），`keyCode`/`isComposing`/`altKey`/`key`/`code` 均可设。

### 回归保障

- 现有 `searchLogic.test.ts`（477 行）不动
- `tsc -b` + `vite build` 必过（0 error）
- 现有 ActionBar 手动 e2e（菜单移动/搜索/子菜单/Alt 快捷键）行为不变

## 风险与缓解

| 风险 | 缓解 |
|------|------|
| 24 个 params 漏传导致编译错 | tsc 强制类型检查，漏传即编译失败 |
| `decideKeyAction` 判断顺序写错导致行为偏移 | 24 条判断矩阵 + 单测逐条覆盖 |
| `KeyContext` 每次按键从 13 ref 组装有开销 | 同步读 ref，与原闭包读 ref 等价，可忽略 |
| `submenuParentIdRef` 写时机改变 | 保留为 hook 内同步写（非 state），与原一致 |
| 拆出文件后 import 循环 | `keyNavigation.ts` 只依赖类型（`ActionBarItem`/`View`/`TabId`/`SearchHit`），不依赖 React/index.tsx，无循环风险 |

## 文件清单

| 文件 | 操作 | 行数预估 |
|------|------|---------|
| `pages/ActionBar/keyNavigation.ts` | 新增 | ~180（含类型 + decideKeyAction + pickSubIdx） |
| `pages/ActionBar/keyNavigation.test.ts` | 新增 | ~250（24 条判断 × 多 case） |
| `pages/ActionBar/useActionBarKeydown.ts` | 新增 | ~130（params type + hook + switch） |
| `pages/ActionBar/index.tsx` | 改：删 212 行 effect，加 1 行 `useActionBarKeydown({...})` | 1030 → ~825 |
