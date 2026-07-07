# 剪贴板浮窗键盘导航 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让剪贴板浮窗完全脱离鼠标可用，对齐 Wox/Raycast 的搜索框持焦 + 方向键导航范式。

**Architecture:** 在 `index.tsx` 注册一个 window 级 keydown handler 统一调度所有按键；选中索引和 tab 切换的纯计算逻辑抽到独立 `.ts` 模块做单元测试；SearchBar 不改交互逻辑，仅确保搜索框持焦。

**Tech Stack:** React 19 + TypeScript + TailwindCSS 4 + Tauri 2 + vitest（jsdom）

## Global Constraints

- **前置条件**：FilterTabs 收藏 tab 已提到第 2 位（main `254a4a2`，本 plan 假定已合入）
- **测试命令**：`npm test`（vitest run，仅匹配 `src/**/*.test.ts`，不含 `.tsx`）
- **类型检查**：`npm run build`（= `tsc -b && vite build`）；快速类型检查用 `npx tsc --noEmit`
- **Lint**：`npm run lint`（oxlint）
- **前端目录**：`crates/desktop/frontend/`，所有命令在该目录下执行
- **零后端改动**：所有 `#[tauri::command]` 复用现有，不改 Rust 代码
- **TABS 顺序**（`FilterTabs.tsx:5`，已调整）：all(1) / favorite(2) / asr(3) / text(4) / ocr(5) / image(6) / file(7)
- **不变量**：鼠标交互（点击/双击/hover 按钮）全部保持原有行为不回归

---

## File Structure

| 文件 | 职责 | 改动类型 |
|------|------|---------|
| `src/lib/clipboardNav.ts` | **新建** — 选中索引移动、tab 循环切换的纯函数 | Create |
| `src/lib/clipboardNav.test.ts` | **新建** — 纯函数单元测试 | Create |
| `src/pages/Clipboard/FilterTabs.tsx` | tab button 加 `data-tab-index` 属性 | Modify |
| `src/pages/Clipboard/SearchBar.tsx` | input 加 `id` 便于 focus 管理 | Modify |
| `src/pages/Clipboard/index.tsx` | 核心：selectedIndex 状态 + window keydown handler + 滚动跟随 + 过滤重置 | Modify |
| `src/pages/Clipboard/ClipboardItem.tsx` | 行根 div 加 `data-clip-index` | Modify |

---

## Task 1: 纯函数 + 单元测试（TDD）

抽取选中索引移动和 tab 循环切换的纯计算逻辑，用 vitest 表驱动测试。

**Files:**
- Create: `crates/desktop/frontend/src/lib/clipboardNav.ts`
- Test: `crates/desktop/frontend/src/lib/clipboardNav.test.ts`

**Interfaces:**
- Produces:
  - `moveIndex(current: number | null, len: number, delta: number): number | null` — 列表选中移动，delta=-1 上、+1 下；到首/末边界停止（不循环）；len=0 返回 null
  - `moveTab(current: number, len: number, delta: number): number` — tab 循环切换，delta=-1 左、+1 右；末尾右移绕回 0，首位左移绕到末尾（循环）

- [ ] **Step 1: 写失败测试**

创建 `crates/desktop/frontend/src/lib/clipboardNav.test.ts`：

```typescript
import { describe, it, expect } from "vitest";
import { moveIndex, moveTab } from "./clipboardNav";

describe("moveIndex", () => {
  const cases: Array<{ current: number | null; len: number; delta: number; want: number | null; note?: string }> = [
    // 正常移动
    { current: 0, len: 5, delta: 1, want: 1, note: "向下" },
    { current: 3, len: 5, delta: -1, want: 2, note: "向上" },
    // 边界夹紧（不循环）
    { current: 0, len: 5, delta: -1, want: 0, note: "首位再上停住" },
    { current: 4, len: 5, delta: 1, want: 4, note: "末位再下停住" },
    // null 初态
    { current: null, len: 5, delta: 1, want: 0, note: "null 向下落到首条" },
    { current: null, len: 5, delta: -1, want: 4, note: "null 向上落到末条" },
    // 空列表
    { current: null, len: 0, delta: 1, want: null, note: "空列表保持 null" },
    { current: 2, len: 0, delta: -1, want: null, note: "列表变空夹紧到 null" },
    // 越界夹紧（列表缩短后 current 超出）
    { current: 5, len: 3, delta: 1, want: 2, note: "current 越界向下夹到末位" },
    { current: 5, len: 3, delta: -1, want: 1, note: "current 越界向上从夹紧位置继续" },
  ];
  for (const c of cases) {
    it(`${c.note ?? "move"}: current=${c.current} len=${c.len} delta=${c.delta} → ${c.want}`, () => {
      expect(moveIndex(c.current, c.len, c.delta)).toBe(c.want);
    });
  }
});

describe("moveTab", () => {
  const len = 7;
  const cases: Array<{ current: number; delta: number; want: number; note?: string }> = [
    { current: 0, delta: 1, want: 1, note: "右移" },
    { current: 3, delta: -1, want: 2, note: "左移" },
    { current: 6, delta: 1, want: 0, note: "末位右移绕回首" },
    { current: 0, delta: -1, want: 6, note: "首位左移绕到末" },
  ];
  for (const c of cases) {
    it(`${c.note}: current=${c.current} delta=${c.delta} → ${c.want}`, () => {
      expect(moveTab(c.current, len, c.delta)).toBe(c.want);
    });
  }
});
```

- [ ] **Step 2: 运行测试确认失败**

```bash
cd crates/desktop/frontend && npm test -- src/lib/clipboardNav.test.ts
```
Expected: FAIL — `Failed to resolve import "./clipboardNav"`（模块不存在）

- [ ] **Step 3: 实现纯函数**

创建 `crates/desktop/frontend/src/lib/clipboardNav.ts`：

```typescript
/**
 * 列表选中索引移动。delta=-1 上移、+1 下移。
 * 边界夹紧（不循环）：到首/末停止。len=0 或 current 越界时夹紧到有效范围或 null。
 */
export function moveIndex(current: number | null, len: number, delta: number): number | null {
  if (len <= 0) return null;
  // 起点夹紧：null→按方向落到首/末；越界→夹到 [0, len-1]
  let start: number;
  if (current === null) {
    start = delta > 0 ? 0 : len - 1;
  } else if (current >= len) {
    start = len - 1;
  } else if (current < 0) {
    start = 0;
  } else {
    start = current;
  }
  const next = start + delta;
  return Math.max(0, Math.min(len - 1, next));
}

/**
 * tab 循环切换。delta=-1 左、+1 右。末尾右移绕回首，首位左移绕到末。
 */
export function moveTab(current: number, len: number, delta: number): number {
  return (current + delta + len) % len;
}
```

- [ ] **Step 4: 运行测试确认通过**

```bash
cd crates/desktop/frontend && npm test -- src/lib/clipboardNav.test.ts
```
Expected: PASS — 所有 case 通过

- [ ] **Step 5: 提交**

```bash
git add crates/desktop/frontend/src/lib/clipboardNav.ts crates/desktop/frontend/src/lib/clipboardNav.test.ts
git commit -m "feat(clipboard): 抽取选中索引/tab切换纯函数及单元测试"
```

---

## Task 2: 选中索引状态 + 行 data 属性 + 过滤重置

把 `index.tsx` 的鼠标驱动 `selectedId` 升级为索引驱动的 `selectedIndex`，为键盘导航建立状态基础。给行加 `data-clip-index` 供滚动定位。

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/index.tsx`
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`

**Interfaces:**
- Consumes: `moveIndex` from `@/lib/clipboardNav`（Task 1）
- Produces: `index.tsx` 暴露 `selectedIndex` 状态 + items 变化时自动重置；`ClipboardItem` 行带 `data-clip-index`

- [ ] **Step 1: index.tsx 引入 selectedIndex 状态**

在 `index.tsx` 顶部 import 区加：

```typescript
import { moveIndex } from "@/lib/clipboardNav";
```

把状态声明（`index.tsx:17-21` 附近）从：

```typescript
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [pinned, setPinned] = useState(false);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [recording, setRecording] = useState(true);
```

改为：

```typescript
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [pinned, setPinned] = useState(false);
  // 键盘导航以数组索引为第一性 citizen；执行动作时从 items[selectedIndex].id 取。
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);
  const [recording, setRecording] = useState(true);
```

把鼠标 `handleSelect`（`:28`）从 `setSelectedId` 改为 `setSelectedIndex`：

```typescript
  const handleSelect = useCallback((index: number) => setSelectedIndex(index), []);
```

（注意：原来是 `(id: number)`，现在改传 index。需同步改 `ClipboardItem` 调用处——见 Step 3。）

- [ ] **Step 2: items 变化时重置/夹紧 selectedIndex**

在 `useClipboardHistory` 调用之后（`:23`）加 useEffect：

```typescript
  const { items, total, refresh } = useClipboardHistory(filter, search);

  // items 变化（过滤/搜索/刷新）后夹紧 selectedIndex：越界则重置到首条或 null。
  useEffect(() => {
    setSelectedIndex((prev) => {
      if (items.length === 0) return null;
      if (prev === null) return 0;
      if (prev >= items.length) return 0;
      return prev;
    });
  }, [items]);
```

- [ ] **Step 3: 渲染时传 selectedIndex + 行加 data-clip-index**

把列表渲染（`:119-128`）从：

```tsx
          items.map((item, index) => (
            <ClipboardItemRow
              key={item.id}
              item={item}
              isLast={index === items.length - 1}
              isSelected={selectedId === item.id}
              onSelect={handleSelect}
              onChanged={refresh}
            />
          ))
```

改为：

```tsx
          items.map((item, index) => (
            <ClipboardItemRow
              key={item.id}
              item={item}
              index={index}
              isLast={index === items.length - 1}
              isSelected={selectedIndex === index}
              onSelect={handleSelect}
              onChanged={refresh}
            />
          ))
```

- [ ] **Step 4: ClipboardItem 接收 index prop + 加 data-clip-index**

在 `ClipboardItem.tsx` 的 props 类型（`:11-23`）加 `index`：

```typescript
function ClipboardItemRow({
  item,
  index,
  isLast,
  isSelected,
  onSelect,
  onChanged,
}: {
  item: ClipboardItem;
  index: number;
  isLast: boolean;
  isSelected: boolean;
  onSelect: (index: number) => void;
  onChanged: () => void;
}) {
```

行根 div（`:152`）加 `data-clip-index`，`handleClick`（`:75`）改传 index：

```tsx
    <div
      data-clip-index={index}
      className={cn(
```

```typescript
  const handleClick = () => {
    if (deletePending) return;
    onSelect(index);
  };
```

（`handleDoubleClick` 里调 `paste_clipboard_item` 用的是 `item.id`，不受影响——`index.tsx` 的 Enter 逻辑会从 `items[selectedIndex].id` 取 id。）

- [ ] **Step 5: 类型检查 + lint**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npm run lint
```
Expected: 无错误

- [ ] **Step 6: 手动验证**

`cargo run --release -p octopus-desktop --features embedded`，按 `Cmd+Shift+D` 唤出浮窗：
- 鼠标点击行仍能选中（背景高亮）——不回归
- 切过滤 tab 后选中态重置到第一行（背景高亮首条）

- [ ] **Step 7: 提交**

```bash
git add crates/desktop/frontend/src/pages/Clipboard/index.tsx crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx
git commit -m "refactor(clipboard): selectedId 改为 selectedIndex 索引驱动"
```

---

## Task 3: ↑↓ 移动选中 + 滚动跟随

注册 window keydown handler，处理 ArrowUp/ArrowDown，选中变化时自动滚动到可见。

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/index.tsx`

**Interfaces:**
- Consumes: `moveIndex` from Task 1；`items` from `useClipboardHistory`

- [ ] **Step 1: 加滚动跟随的 ref 和 useEffect**

在 `index.tsx` import 区加 `useRef`（若未导入）：

```typescript
import { useState, useCallback, useEffect, useRef } from "react";
```

在 selectedIndex 状态之后加滚动跟随：

```typescript
  // 选中变化时滚动到可见行。
  useEffect(() => {
    if (selectedIndex === null) return;
    const el = document.querySelector(`[data-clip-index="${selectedIndex}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);
```

- [ ] **Step 2: 注册 window keydown 处理 ↑↓**

在组件内（滚动 useEffect 之后）加 keydown handler。注意要用 ref 存最新值避免闭包陷阱：

```typescript
  // 全局按键处理需要读最新 items/search，用 ref 避免闭包过期。
  const itemsRef = useRef(items);
  itemsRef.current = items;

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // ↑↓ 移动选中（无条件拦截，即使焦点在搜索框）
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const cur = itemsRef.current;
        if (cur.length === 0) return;
        setSelectedIndex((prev) => moveIndex(prev, cur.length, e.key === "ArrowDown" ? 1 : -1));
        return;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);
```

- [ ] **Step 3: 类型检查 + lint**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npm run lint
```
Expected: 无错误

- [ ] **Step 4: 手动验证**

唤出浮窗，搜索框持焦状态下：
- `↓` 选中下移、`↑` 选中上移，背景高亮跟随
- 到首/末边界停止，不循环
- 选中条目滚出视区时自动滚入
- 搜索框内打字时 `↑↓` 仍移动选中（不移动光标）

- [ ] **Step 5: 提交**

```bash
git add crates/desktop/frontend/src/pages/Clipboard/index.tsx
git commit -m "feat(clipboard): 上下键移动选中并滚动跟随"
```

---

## Task 4: Enter 粘贴 + Esc 清空/隐藏

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/index.tsx`

**Interfaces:**
- Consumes: Tauri `invoke("paste_clipboard_item", { id })`；`getCurrentWindow().hide()`

- [ ] **Step 1: SearchBar 暴露 ref 给 index.tsx 用于 focus（Esc 隐藏后不需要，但后续 focus 管理需要）**

先不改 SearchBar——Esc 用 `getCurrentWindow().hide()` 即可。Enter 用现有 itemsRef 取 id。

- [ ] **Step 2: 在 keydown handler 加 Enter 和 Esc 分支**

在 Task 3 的 handler 函数内，`ArrowDown/ArrowUp` 分支之后追加：

```typescript
      // Enter：对选中条目执行粘贴（复用 paste_clipboard_item，后端已双保险：写剪贴板+模拟粘贴）
      if (e.key === "Enter") {
        e.preventDefault();
        const cur = itemsRef.current;
        const idx = selectedIndexRef.current;
        if (idx === null || idx >= cur.length) return;
        invoke("paste_clipboard_item", { id: cur[idx].id }).catch(console.error);
        return;
      }
      // Esc：有搜索内容则清空，已空则隐藏浮窗
      if (e.key === "Escape") {
        e.preventDefault();
        if (searchRef.current !== "") {
          setSearch("");
        } else {
          getCurrentWindow().hide();
        }
        return;
      }
```

- [ ] **Step 3: 补充 selectedIndexRef 和 searchRef**

handler 闭包内读 `selectedIndex` 和 `search` 会过期。在 Task 2 的 itemsRef 旁边加：

```typescript
  const itemsRef = useRef(items);
  itemsRef.current = items;
  const selectedIndexRef = useRef(selectedIndex);
  selectedIndexRef.current = selectedIndex;
  const searchRef = useRef(search);
  searchRef.current = search;
```

确保 `invoke` 和 `getCurrentWindow` 已导入（`index.tsx:2-3` 已有 `getCurrentWindow`；`invoke` 在 `:3` 已有）。如未导入则补：

```typescript
import { invoke } from "@/lib/tauri";
```

- [ ] **Step 4: 类型检查 + lint**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npm run lint
```
Expected: 无错误

- [ ] **Step 5: 手动验证**

- `↑↓` 选中条目后 `Enter` → 浮窗隐藏 + 内容粘贴到原应用（与双击行为一致）
- 搜索框输入文字后 `Esc` → 清空搜索内容（不隐藏）；再按 `Esc` → 隐藏浮窗
- 空搜索时 `Esc` → 直接隐藏浮窗

- [ ] **Step 6: 提交**

```bash
git add crates/desktop/frontend/src/pages/Clipboard/index.tsx
git commit -m "feat(clipboard): Enter粘贴 + Esc清空/隐藏"
```

---

## Task 5: Tab/←→/Cmd+N 切 tab + FilterTabs data-tab-index

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/FilterTabs.tsx`
- Modify: `crates/desktop/frontend/src/pages/Clipboard/index.tsx`

**Interfaces:**
- Consumes: `moveTab` from Task 1；`TABS` 顺序常量（7 个）

- [ ] **Step 1: FilterTabs 加 data-tab-index**

`FilterTabs.tsx` 的 button（`:25` 附近）加属性：

```tsx
        <button
          key={tabValue}
          data-tab-index={TABS.findIndex((t) => t.value === tabValue)}
          title={label}
```

- [ ] **Step 2: index.tsx 定义 TABS_VALUES 常量**

在 `index.tsx` 顶部（组件外）加 tab 值顺序常量，与 `FilterTabs.tsx` 的 TABS 保持一致：

```typescript
// 与 FilterTabs.tsx TABS 数组顺序一致——Cmd+N 序号映射。
const TABS_VALUES = ["all", "favorite", "asr", "text", "ocr", "image", "file"] as const;
```

- [ ] **Step 3: 在 keydown handler 加 Tab/←→/Cmd+数字 分支**

在 Task 4 的 handler 内，Escape 分支之后追加：

```typescript
      // Tab / Shift+Tab：恒定切过滤 tab（preventDefault 拦截，不让浏览器遍历全浮窗焦点）
      if (e.key === "Tab") {
        e.preventDefault();
        const cur = TABS_VALUES.indexOf(filterRef.current as typeof TABS_VALUES[number]);
        const next = moveTab(cur < 0 ? 0 : cur, TABS_VALUES.length, e.shiftKey ? -1 : 1);
        setFilter(TABS_VALUES[next]);
        return;
      }
      // ←→：仅搜索框为空时切 tab（有内容时让出给光标移动，不拦截）
      if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && searchRef.current === "") {
        e.preventDefault();
        const cur = TABS_VALUES.indexOf(filterRef.current as typeof TABS_VALUES[number]);
        const next = moveTab(cur < 0 ? 0 : cur, TABS_VALUES.length, e.key === "ArrowLeft" ? -1 : 1);
        setFilter(TABS_VALUES[next]);
        return;
      }
      // Cmd+1..7：直接跳 tab（metaKey=macOS，ctrlKey=Windows/Linux）
      if ((e.metaKey || e.ctrlKey) && e.key >= "1" && e.key <= "7") {
        e.preventDefault();
        const n = parseInt(e.key, 10) - 1;
        if (n < TABS_VALUES.length) setFilter(TABS_VALUES[n]);
        return;
      }
```

- [ ] **Step 4: 补充 filterRef**

在 Task 4 的 ref 们旁边加：

```typescript
  const filterRef = useRef(filter);
  filterRef.current = filter;
```

- [ ] **Step 5: 类型检查 + lint**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npm run lint
```
Expected: 无错误

- [ ] **Step 6: 手动验证（完整验收清单）**

唤出浮窗，逐项验证：
1. 搜索框**空**时 `←→` 循环切换 7 个 tab（← 右到左，→ 左到右）
2. 搜索框**有内容**时 `←→` 只移动光标不切 tab
3. `Tab/Shift+Tab` 无论搜索框是否有内容都能切 tab
4. `Cmd+1` 到 `Cmd+7`（Windows 用 `Ctrl`）直接跳对应 tab，顺序为 all/favorite/asr/text/ocr/image/file
5. 切 tab 后选中态重置到第一行
6. 鼠标点击 tab 仍正常（不回归）

- [ ] **Step 7: 提交**

```bash
git add crates/desktop/frontend/src/pages/Clipboard/FilterTabs.tsx crates/desktop/frontend/src/pages/Clipboard/index.tsx
git commit -m "feat(clipboard): Tab/方向键/Cmd+N 切换过滤 tab"
```

---

## 验收总清单（全部完成后逐项确认）

对照 spec 第 6 节：
- [ ] 1. 不碰鼠标即可完成：搜索 → `↑↓` 选条目 → `Enter` 粘贴到原应用
- [ ] 2. 搜索框空时 `←→` 循环切换 7 个 tab；`Cmd+1..7` 可直接跳转
- [ ] 3. 搜索框有内容时 `←→` 只移动光标不切 tab；`Tab/Shift+Tab` 仍可切 tab
- [ ] 4. `Esc` 在有搜索内容时清空，已空时隐藏浮窗
- [ ] 5. `↑↓` 选中会自动滚动跟随，选中条目始终可见
- [ ] 6. 过滤/搜索切换后选中态重置为第一条
- [ ] 7. 鼠标交互（点击/双击/hover 按钮）全部保持原有行为不回归

最终全量检查：
```bash
cd crates/desktop/frontend && npm test && npx tsc --noEmit && npm run lint
```

---

## 实施记录（plan 是实施记录，非一次性待办）

> 以下为实际执行后的偏差回写：新增决策、踩坑修复、scope 变更。

### 已完成的 5 个 Task + 额外 6 个修复/增强

| Task | 状态 | commit | 说明 |
|------|------|--------|------|
| 1. 纯函数 + TDD | ✅ | `6928137` | `moveIndex`/`moveTab` + 14 测试 |
| 2. selectedIndex 状态 | ✅ | `db01cf8` | selectedId→selectedIndex，data-clip-index |
| 3. ↑↓ 移动 + 滚动跟随 | ✅ | `086c964` | window keydown handler + scrollIntoView |
| 4. Enter 粘贴 + Esc | ✅ | `add1a4b` | 复用 paste_clipboard_item |
| 5. Tab/←→/Cmd+N 切 tab | ✅ | `9a6f855` | 全部按键在一个 handler 统一调度 |
| 6. Cmd→Ctrl 修复 | ✅ | `21a9b4d` | Accessory 激活策略下 Cmd+digit 被前一 app 拦截 |
| 7. 修饰键可配置 | ✅ | `8715940` | 设置页下拉 cmd/ctrl/alt，配置 `clipboard_tab_modifier` |
| 8. label "剪贴TAB切换" | ✅ | `87ad358` | 用户反馈 label 调整 |
| 9. UI 加 "+ 1..7" 提示 | ✅ | `a531377` | 下拉框右侧后缀 |
| 10. AppConfig 白名单注册 | ✅ | `b706dba` | apply_config_value 加 match 分支（踩坑："未知配置字段"） |
| 11. DB load/save 注册 | ✅ | `c706dba` | load/save 手动枚举补字段（踩坑第 4 次，archived specs 有记录） |
| 12. serde 重构 load/save | ✅ | `cfcebf0` | 根治手动枚举 + round-trip 回归测试 |
| 13. config-changed 无条件 emit | ✅ | `84fc67c` | emit 白名单（踩坑第 5 次） |
| 14. Option 用 e.code | ✅ | `8687982` | macOS Option+数字产生特殊字符，e.key 不匹配 |

### 与原 plan 的偏差

1. **SearchBar 无改动**：plan 原设想 SearchBar 拦截 ←→/Tab，实际实现统一在 `index.tsx` window keydown handler 处理（更清晰，避免 SearchBar↔index 状态透传）。SearchBar 零改动。

2. **Cmd→可配置修饰键**：plan 原写 `Cmd+1..7`，实际发现 macOS Accessory 策略下 Cmd 被拦截 → 改 Ctrl → 再改为用户可配置（cmd/ctrl/alt）。

3. **Option 用 e.code**：plan 原用 `e.key >= "1" && e.key <= "7"`，实际发现 macOS Option 产生特殊字符（Option+1="¡"）→ 改用 `e.code`（物理键位 `Digit1..Digit7`）。

4. **配置系统 serde 重构**：plan 未涉及。实施中发现 `clipboard_tab_modifier` 新增字段需在 7 处注册（struct/default/apply_config_value/load/save/db.sql/前端），手动枚举已踩坑 4 次 → 根治为 serde 自动 + 无条件 emit + round-trip 测试。详见 architecture.md 配置系统章节。

### 新增文件

| 文件 | 用途 |
|------|------|
| `src/lib/clipboardNav.ts` | `moveIndex`/`moveTab` 纯函数 |
| `src/lib/clipboardNav.test.ts` | 纯函数单元测试（14 case） |

### 最终验证

- 前端：62/62 测试通过，tsc clean，lint 无新增问题
- Rust：51/51 infra 测试通过（含新增 round-trip），desktop 编译通过
- E2E：用户确认 cmd/ctrl/alt 三种修饰键均可生效
