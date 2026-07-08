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
| **主题系统** | | | |
| 15. 内置 3 套主题 + JSON 扩展 | ✅ | `0628d13` | theme.rs + theme.ts + GeneralPanel 外观卡片 |
| 16. 主题切换同步到所有窗口 | ✅ | `f05f776` | App.tsx 每窗口 mount + listen config-changed |
| 17. 重新设计 3 套配色 | ✅ | `270cf57` | Warm Paper / Obsidian Glass / Nord Aurora（ui-ux-pro-max） |
| 18. result_window 透出 + 工具栏不可见 | ✅ | `2566dc8` | backdrop-blur 只在 clipboard_window；新增 surface/tool-icon token |
| 19. 暗色背景纯不透明 | ✅ | `2b87c86` | CSS backdrop-filter 无法实现均匀模糊，改为纯 hex |
| 20. 选中/hover 视觉冲突 | ✅ | `d7268f6` | hover 用 bg-muted，selected 加 voice 指示条 |
| 21. 编辑器跟随主题 | ✅ | `66f3e3b` | CompactEditor stone 硬编码改为语义 token |
| 22. 截图工具栏跟随主题 | ✅ | `f1451f1` | 工具栏/弹窗背景+图标改为 CSS 变量 |
| 23. 截图图标 icon_filter | ✅ | `e734876` | 暗色主题 brightness(0) invert(1) 反色 |
| 24. 剪贴管理页跟随主题 | ✅ | `d19e979` | ClipboardPanel 22 处 stone 硬编码改语义 token |
| 25. icon_filter 去掉 opacity | ✅ | `08e7d16` | opacity(0.65) 在深色背景上太暗 |
| 26. icon-filter 变量传递修复 | ✅ | `a6053a9` | 从 theme.colors 取（非顶层），跳过颜色遍历 |
| 27. HistoryPanel + blur 清理 | ✅ | `71dd3da` | 15 处 stone 改语义 token；移除 backdrop-blur 死代码 |
| 28. 全局 SVG 图标 icon-filter | ✅ | `70ef247` | ClipboardPanel/ClipboardItem SVG img 加 var(--icon-filter) |
| 29. ImagePreview 工具栏适配 | ✅ | `ffeef52` | ToolButton 加 color 让 Lucide 图标可见；卡片/弹窗改 CSS 变量 |
| 30. ScrollPreview 保存按钮 | ✅ | `24f5d02`+`831f41e` | #3b82f6→var(--color-voice)；修 JSX 语法 |
| **主题性能优化** | | | |
| 31. localStorage 快照恢复 | ✅ | `53678a4` | restoreCachedTheme 零 IPC 同步恢复 |
| 32. CSS 变量改 style 标签 | ✅ | `ad1e1b6` | inline style→`<style>` 注入，浏览器缓存 var() |
| 33. visible(false)+show 方案 | ❌→回退 | `3a3c218`→`f31b2e6` | 尝试消除 PPT slide，导致需点两次，回退 |
| 34. IPC 优化（方案 A） | ✅ | `ac2aea7` | list_themes OnceLock 缓存 + get_theme_id 轻量命令 |
| 35. data-theme 预编译（方案 B） | ✅ | `dd4b096` | [data-theme] CSS 规则块，属性选择器替代 JS var() 覆盖 |
| 36. 白屏+IPC 修复（审查反馈） | ✅ | `c3bfb56` | index.html 阻断脚本恢复主题；去 mount IPC；恢复 themeCache |
| 37. 时序竞态+白屏+配置同步（V2 审查） | ✅ | `3d52169` | index.html 不读 label（移到 main.tsx）；恢复 mount 时 applyThemeFromConfig |
| 38. CompactEditor 打开加速（V3 审查） | ✅ | `907acef` | 3 IPC→1（PendingTabFull 合并）；initialLoading 消除占位符闪烁 |
| 39. index.html 无条件设背景色 | ✅ | `8386eab` | 不区分 label，transparent 窗口不受影响；消除 main.tsx 加载前白屏 |
| 40. URL 参数注入零 IPC 打开 | ✅ | `0dca130` | Rust 建窗拼 URL query；前端 useState 初始化同步读取，零 IPC |
| 41. 截图窗口选区被背景色盖住 | ✅ | `3db7512` | 截图窗口 URL 加 ?screenshot=1，index.html 检测跳过背景色 |
| 42. V3 审查 4 项 | ✅ | `0175cc9` | 背景色移 main.tsx；图片尺寸 URL 注入；移除无条件背景色 |
| 43. 背景色白名单模式 | ✅ | `8fbfa38` | 只对 settings/compact_editor 设，result/clipboard/screenshot 不设 |
| 44. 截图窗口去掉 body 遮罩 | ✅ | `d292e9e` | 截图遮罩由 React 组件画（选区外），body 背景盖住选区 |
| 45. 背景色 URL hex 注入 | ✅ | `2f13ef0` | Rust 拼裸 hex 到 URL，index.html 首帧同步设色（零 CSS 依赖） |
| 46. 脏检查 + 透明窗口兜底移除 | ✅ | `4656d22` | applyThemeById 脏检查避免重复 recalc；移除 localStorage bg 兜底 |
| 47. 最大化 build 前设置 | ✅ | `f39e6c0` | builder.maximized(true) 替代 win.maximize()，消除 PPT slide |
| 48. 最大化时跳过 inner_size | ✅ | `bc1470e` | 审查建议的清理，避免冗余布局计算 |
| 49. 坐标偏差 outer/inner 统一 | ✅ | `452c934` | inner_position + inner_size 对称保存恢复 |
| 50. 多显示器位置越界检测 | ✅ | `01d6087` | 副屏关闭后坐标失效，恢复前检测 fallback 居中 |
| 51. visible(false)+show+maximize | ✅ | `9cf19a2`→`4aba9e6` | 消除放大过渡 + set_focus 确保激活 |
| 52. builder.maximized 未生效 | ✅ | `d62714d` | build 后 show 前 win.maximize() |
| 53. 最大化不设 position | ✅ | `1bc402c` | 防副屏 |
| 54. 主屏尺寸直接创建 | ❌→回退 | `1444b3f` | is_maximized=false 导致保存错误状态 |
| 55. maximize()+主屏位置 | ❌→回退 | `6a238d2` | 副屏最大化被挪到主屏 |
| 56. 大窗体+maximize | ✅ | `9326403` | 接近全屏尺寸创建，maximize 视觉差异小 |
| 57. 保存坐标找显示器+余量 | ✅ | `bd8fe4d` | 副屏不挪主屏；四边 80px 余量 |
| 58. 最大化保存最后非最大化位置 | ✅ | `92951d1` | DB key compact_editor_last_normal_pos |
| 59. 副屏未连接回退默认大小 | ✅ | `138848d`→`b421a53` | should_maximize 标记；fallback 主屏最大化 |
| 60. 显示器坐标物理/逻辑不匹配 | ✅ | `790ac15` | position 未除 scale，副屏永远匹配主屏 |
| 61. un-maximize 取真实位置 | ✅ | `e6acce0` | 最大化关闭时先 unmaximize 取 inner_position；非最大化越界检测同修 |
| 62. window_position 同修 | ✅ | `c3efb0c` | result/clipboard 的 is_position_visible 同样的物理像素未除 scale bug |
| 63. CompactEditor 2 P0 + 4 优化 | ✅ | `765b560` | 图片 tab 满额越界白屏 + 图片/只读快捷键删除 + readInitialTabFromUrl/ToolBtn/savedFlash/死代码 |
| 64. 前端审查 V1（4 项） | ✅ | `eb9f572` | replaceAll $ 特殊字符 + HistoryPanel 双展开 + Result 串行 listen 并行化 + popup toggle |
| 65. 剪贴板列表 50→200 | ✅ | `26d5602` | useClipboardHistory size 50→200 |
| 66. 前端审查 V2（5 项） | ✅ | `154a945` | Settings navigate e.payload + ClipboardPanel split \\ + text/transcription setActiveIdx + caret 零 rect + themeCache 清除 |
| 67. Settings listen→rawListen 遗漏 | ✅ | `c0ed7b8` | 154a945 改 import 漏改 config-changed 调用点，运行时 ReferenceError |
| 68. 搜索时禁用清理按钮 | ✅ | `c42c6f9` | 防误删全类别数据（clear_by_filter 不带 search 参数） |
| 69. 截图 Object URL 内存泄漏 | ✅ | `4bb9cca` | createObjectURL 后 onload 未 revokeObjectURL，每次截图泄漏 2-8MB |
| 70. 后端审查——OCR+get_config async | ✅ | `0bfc615` | ocr_image spawn_blocking；get_config async+spawn_blocking（DB+cpal） |

### 与原 plan 的偏差

1. **SearchBar 无改动**：plan 原设想 SearchBar 拦截 ←→/Tab，实际实现统一在 `index.tsx` window keydown handler 处理（更清晰，避免 SearchBar↔index 状态透传）。SearchBar 零改动。

2. **Cmd→可配置修饰键**：plan 原写 `Cmd+1..7`，实际发现 macOS Accessory 策略下 Cmd 被拦截 → 改 Ctrl → 再改为用户可配置（cmd/ctrl/alt）。

3. **Option 用 e.code**：plan 原用 `e.key >= "1" && e.key <= "7"`，实际发现 macOS Option 产生特殊字符（Option+1="¡"）→ 改用 `e.code`（物理键位 `Digit1..Digit7`）。

4. **配置系统 serde 重构**：plan 未涉及。实施中发现 `clipboard_tab_modifier` 新增字段需在 7 处注册（struct/default/apply_config_value/load/save/db.sql/前端），手动枚举已踩坑 4 次 → 根治为 serde 自动 + 无条件 emit + round-trip 测试。详见 architecture.md 配置系统章节。

5. **主题系统（plan 未涉及）**：借鉴 Wox 主题设计，新增 3 套内置主题 + JSON 扩展。关键决策：
   - 半透明改纯不透明（CSS backdrop-filter 在 Tauri WebView 下无法实现均匀模糊）
   - 新增 surface（不透明背景）、tool-icon（工具栏图标色）、icon-filter（截图图标 CSS filter）三个 token
   - 所有窗口的 stone 硬编码逐步替换为语义 token（Result/CompactEditor/ClipboardPanel/HistoryPanel/Screenshot/ImagePreview）
   - 两类图标适配：SVG `<img>` 用 `var(--icon-filter)` 反色；Lucide React 图标靠 ToolButton 设 `color` 让 `currentColor` 继承
6. **主题性能优化（A→B→审查反馈，三次迭代）**：
   - 用户报告主题导致窗口打开/拖动变慢。排查发现三个层面开销：IPC 往返、CSS var() 解析、白屏闪烁
   - **方案 A**（IPC 优化）：list_themes OnceLock 缓存 + get_theme_id 轻量命令 + 前端 themeCache
   - **方案 B**（CSS 预编译）：`data-theme` 属性 + `[data-theme="xxx"]` CSS 规则块，属性选择器替代 JS var() 覆盖
   - **审查反馈**（白屏+IPC 修复）：index.html 阻断脚本同步恢复主题（消除白屏）；App.tsx 去掉 mount 时无条件 IPC；自定义主题 CSS 缓存到 localStorage；恢复 themeCache（dd4b096 误删）
   - **V2 审查反馈**（时序竞态+白屏+配置同步）：index.html `__TAURI_INTERNALS__` 在 `<head>` 解析时尚未注入，label 读到空串→背景色被跳过。修复：index.html 只恢复 data-theme + 自定义 CSS（不读 label），背景色判断移到 main.tsx（桥接层已就绪）；恢复 App.tsx mount 时异步校正（首次运行/清缓存时必需）
   - visible(false)+show 方案试过但回退（窗口需点两次才出现）
   - 最终架构：index.html 阻断脚本恢复 data-theme + URL hex 背景色注入（零 CSS 依赖）+ data-theme CSS 预编译 + App.tsx config-changed 驱动 + mount 异步校正（脏检查避免重复 recalc）
7. **CompactEditor 打开加速（V3 审查反馈）**：
   - 诊断：tabs 初始 `[]` → "没有打开的条目"占位符闪烁 + 3 次串行 IPC（get_pending_compact_tab → get_clipboard_item_type → get_clipboard_item_text）
   - 修复：后端 `store_pending_tab` 时一次性读 DB，返回 `PendingTabFull`（含 itemType + text）；前端 mount 1 次 IPC 直接建 tab；`initialLoading` 状态隐藏占位符

### 新增文件

| 文件 | 用途 |
|------|------|
| `src/lib/clipboardNav.ts` | `moveIndex`/`moveTab` 纯函数 |
| `src/lib/clipboardNav.test.ts` | 纯函数单元测试（14 case） |
| `src/lib/theme.ts` | `applyThemeById`/`restoreCachedTheme`/`applyThemeFromConfig`（data-theme 属性切换） |
| `crates/desktop/src/theme.rs` | 3 套内置主题 + `list_themes`（OnceLock 缓存）+ `get_theme_id` |

### 最终验证

- 前端：79/79 测试通过，tsc clean，lint 无新增问题
- Rust：51/51 infra 测试通过（含新增 round-trip），desktop 编译通过
- E2E：用户确认 cmd/ctrl/alt 三种修饰键均可生效；3 套主题全窗口同步正确
- 性能：主题加载经五次优化（IPC 缓存→data-theme 预编译→index.html 阻断脚本→时序竞态修复→白名单背景色），白屏消除；CompactEditor 3 IPC→1→**0**（URL 参数注入）+ 占位符消除
- **背景色方案最终定型**（task 39→46 六次反复后）：Rust 建窗时从主题配置读 background hex，拼入 URL `?bg=2e3440`。index.html `<head>` 脚本读 bg 参数直接设裸 `#hex`——零 CSS 依赖、零 JS bundle 依赖、首帧即有色。透明窗口（result/clipboard/screenshot）无 bg 参数，不设背景色。applyThemeById 加脏检查（`data-theme` 值相同直接 return）避免重复 style recalc。教训：`transparent:true` 不覆盖 html 背景色；`var(--color-background)` 依赖 CSS 加载（dev 模式有延迟）；截图遮罩由 React 组件画在选区外（选区内全透明看桌面），body 背景会盖住选区。
