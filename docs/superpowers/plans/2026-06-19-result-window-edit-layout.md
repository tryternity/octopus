# 结果窗编辑布局调整 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 调整结果窗编辑布局——进入/退出编辑文字水平不重排，保存入口从文本区「完成编辑」按钮迁移到 toolbar 的 ✏️ toggle（编辑态切 💾 icon）。

**Architecture:** 仅改 `crates/desktop/dist/result/index.html`（单文件前端）+ 已就位 `icons/save.svg`。无 Rust/config 改动。删 `#edit-done` 按钮 + 编辑态 `padding-right:90px`（消除水平重排根因）；✏️ 按钮复用 toggle + CSS mask 切换 icon；编辑态强制 toolbar 常驻保证保存可见。

**Tech Stack:** vanilla HTML/CSS/JS（无构建）+ Tauri webview

**设计 spec:** `docs/superpowers/specs/2026-06-19-result-window-edit-layout-design.md`

> **状态（2026-06-19）：已实施并合并到 main**（`d4401cb`：✏️ toggle + 删 edit-done + 文字不重排 + 编辑态 toolbar 常驻；e2e 通过）。下方 checkbox 标记实际完成进度。快捷键后续统一为 `edit_shortcut` toggle（`370e21e`）。

---

## 文件结构

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/dist/result/index.html` | 结果窗前端（编辑态 DOM/CSS/JS） | **修改** |
| `crates/desktop/dist/result/icons/save.svg` | 保存图标（CSS mask 源，FA 软盘） | 已就位（`596cf86` 入库） |

## 测试策略

前端为 vanilla HTML/CSS/JS，无自动测试框架（YAGNI，不为单次改动引入）。每个 task 改完用 `cargo run -p octopus-desktop` 启动应用按步骤手动验证；Task 4 做全量 e2e（spec §7 六项）。无 Rust 单测（后端命令 `enter_edit_mode`/`commit_edit` 不变）。

> **行号说明**：下方行号基于当前 `main`（`596cf86`）。前序 task 改动后行号会偏移，**以代码内容（old_string）定位**，勿依赖行号。

## 实现 worktree（用户要求隔离）

执行本 plan 前，先 `EnterWorktree`（或 `superpowers:using-git-worktrees`）创建新 worktree，在 worktree 内逐 task 实现 + commit，最后按 `superpowers:finishing-a-development-branch` merge 回 main。

---

### Task 1: ✏️ 按钮复用 toggle + 图标切换

**Files:**
- Modify: `crates/desktop/dist/result/index.html`（CSS `#tool-edit .icon` 规则约 L103 + JS `btnEdit` click 约 L466 + `enterEdit`/`commitEdit` 约 L428-457）

- [x] **Step 1: CSS 加编辑态 icon 切换**

在 `#tool-edit .icon` 规则（约 L103）之后新增：

```css
    /* 编辑态：✏️ 图标切为 💾（save.svg）；点 tool-edit = 保存（toggle 语义） */
    #container.editing #tool-edit .icon {
      -webkit-mask-image: url(icons/save.svg?v=1);
      mask-image: url(icons/save.svg?v=1);
    }
```

- [x] **Step 2: JS `btnEdit` click 改 toggle 语义**

old:
```js
    btnEdit.addEventListener('click', (e) => { e.preventDefault(); enterEdit(); });
```
new:
```js
    btnEdit.addEventListener('click', (e) => {
      e.preventDefault();
      editing ? commitEdit() : enterEdit();
    });
```

- [x] **Step 3: `enterEdit`/`commitEdit` 加 title/aria-label 切换**

`enterEdit()` 中，在 `btnEdit.classList.add('active');` 之后加：
```js
      btnEdit.title = '保存编辑';
      btnEdit.setAttribute('aria-label', '保存编辑');
```

`commitEdit()` 中，在 `btnEdit.classList.remove('active');` 之后加：
```js
      btnEdit.title = '编辑';
      btnEdit.setAttribute('aria-label', '编辑');
```

- [x] **Step 4: 手动验证**

Run: `cargo run -p octopus-desktop`
Expected: 识别出文字 → 点 ✏️ 进入编辑 → 图标变 💾、tooltip「保存编辑」→ 点 💾 → 保存退出、图标回 ✏️。
（此 task 后 `#edit-done` 按钮仍在，两保存入口临时并存——Task 2 删除。）

- [x] **Step 5: Commit**

```bash
git add crates/desktop/dist/result/index.html
git commit -m "feat(desktop): ✏️ 按钮复用 toggle——编辑态切 save icon + click toggle 保存"
```

---

### Task 2: 删除 `#edit-done` 按钮 + 移除编辑态 padding（文字不重排）

**Files:**
- Modify: `crates/desktop/dist/result/index.html`（DOM 约 L241 + CSS 约 L184-209 + JS `btnEditDone` 引用 L426/433/454/482-483/497）

- [x] **Step 1: 删 DOM**

删 `#text-wrapper` 内这一行：
```html
      <button id="edit-done" hidden>完成编辑</button>
```
（删后 `#text-wrapper` 内只剩 `<div id="result-text"></div>`）

- [x] **Step 2: 删 CSS**

删 `#edit-done` 全部规则（约 L184-198，含注释「完成编辑按钮：编辑态显示，浮于文本区右上」+ `#edit-done` + `#edit-done:hover`）。

删编辑态 `#result-text` 的 padding 规则（约 L206-208）：
```css
    #container.editing #result-text {
      padding: 1px 90px 7px 13px;   /* right 90px 给完成按钮让位 */
    }
```
**保留**其下的 `#container.editing #result-text:focus { background: transparent; }` 与 `#container.editing #text-wrapper` 淡蓝边框规则。

- [x] **Step 3: 删 JS `btnEditDone` 全部引用**

- 删 `const btnEditDone = document.getElementById('edit-done');`（约 L426）
- `enterEdit()` 删 `btnEditDone.hidden = false;`（约 L433）
- `commitEdit()` 删 `btnEditDone.hidden = true;`（约 L454）
- 删两行 `btnEditDone.addEventListener(...)`（mousedown/click，约 L482-483）
- `edit-force-exit` 处理删 `btnEditDone.hidden = true;`（约 L497）

- [x] **Step 4: 手动验证**

Run: `cargo run -p octopus-desktop`
Expected: ✏️ 进入编辑 → **文字水平位置不变（不重排）**；文本区无「完成编辑」按钮；点 💾 保存正常。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/dist/result/index.html
git commit -m "refactor(desktop): 删 edit-done 按钮 + 移除编辑态 padding（文字不重排）"
```

---

### Task 3: 编辑态 toolbar 强制常驻

**Files:**
- Modify: `crates/desktop/dist/result/index.html`（`enterEdit()` 约 L428-445）

- [x] **Step 1: `enterEdit` 末尾调 `showToolbar()`**

在 `enterEdit()` 内、`invoke('enter_edit_mode');` 之前加：
```js
      showToolbar();
```
（`showToolbar` 内部 `if (toolbarVisible) return`——点 ✏️ 进入时 toolbar 已 visible，no-op 无跳动；Cmd+E 进入若 hidden 则显示。`hideToolbar` 已有 `editing` 拦截，编辑中不会隐藏，无需改。）

- [x] **Step 2: 确认 force-exit 自动恢复 icon（CSS 驱动，无需额外 JS）**

icon 切换靠 `#container.editing #tool-edit .icon` CSS。`edit-force-exit` 处理（约 L491-500）已 `container.classList.remove('editing')`（移除 editing class → CSS 不再匹配 → 图标自动回 `edit.svg`）+ `btnEdit.classList.remove('active')`。**无需补图标恢复代码**，确认这两行存在即可。

- [x] **Step 3: 手动验证**

Run: `cargo run -p octopus-desktop`
Expected:
1. 鼠标移出结果窗使 toolbar 隐藏 → Cmd+E 进入 → toolbar 出现（窗口增高、文字下移 24px）→ 💾 可见可点
2. 编辑中 mouseleave → toolbar **不隐藏**（editing 拦截）
3. 编辑中触发新录音（force-exit）→ 图标自动回 ✏️、退出编辑态

- [x] **Step 4: Commit**

```bash
git add crates/desktop/dist/result/index.html
git commit -m "feat(desktop): 编辑态强制 toolbar 常驻（enterEdit showToolbar）"
```

---

### Task 4: 全量 e2e + 文档同步 + 收尾

- [x] **Step 1: 全量 e2e（spec §7 六项）**

Run: `cargo run -p octopus-desktop`，逐项验证：
1. 识别出文字 → ✏️ 进入 → **文字水平位置不变（不重排）** ✓
2. 编辑态图标 💾 → 点 💾 保存 → 退出、图标回 ✏️ ✓
3. `edit_shortcut` 进入 → 再按 `edit_shortcut` 保存（toggle）✓
4. Cmd+E 进入（toolbar 此前 hidden）→ toolbar 出现 → 💾 可见可点 ✓
5. 编辑中 mouseleave → toolbar 不隐藏 ✓
6. 编辑中触发新录音（force-exit）→ 图标回 ✏️、退出编辑态 ✓

- [x] **Step 2: 文档同步检查**

Run: `grep -rn "edit-done" docs/ crates/`（排除本 spec/plan）
Expected: 若 `architecture.md` L198-203「结果窗可编辑」段或 editable-result spec 提到 `edit-done`/「完成编辑」按钮，更新为「✏️ toggle（编辑态 💾）」。spec §8 已声明不改 editable-result spec 机制描述，预期改动小或无。

- [x] **Step 3: Commit（若有文档改动）**

```bash
git add docs/
git commit -m "docs: 同步结果窗编辑布局调整（保存按钮移 toolbar toggle）"
```
（若无改动跳过）

- [x] **Step 4: 收尾**

按 `superpowers:finishing-a-development-branch`：workspace 测试（手动 e2e 已过）→ merge worktree 分支回 main（ff）→ 删 worktree 分支。

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §4.1 删 `#edit-done`（DOM/CSS/JS） | Task 2 |
| §4.2 移除编辑态 padding（文字不重排） | Task 2 |
| §4.3 ✏️ toggle + 图标切换（CSS mask + JS toggle） | Task 1 |
| §4.4 编辑态 toolbar 强制常驻（`enterEdit` showToolbar） | Task 3 |
| §4.5 不变项（快捷键/后端命令） | 无改动（验证 Task 4） |
| §6 force-exit 图标恢复 | Task 3 Step 2（CSS 驱动自动） |
| §7 e2e 六项 | Task 4 Step 1 |
