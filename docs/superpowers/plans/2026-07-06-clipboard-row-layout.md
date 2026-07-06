# 剪贴板历史条目两行布局 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把剪贴板浮窗的历史行从「单行 + 右侧 hover 操作占位空白」重构为两行布局——第一行铺满内容 + 行尾元数据，第二行时间戳 + 操作按钮，消除右侧大片空白；最常用的「复制」操作置顶。

**Architecture:** 纯前端渲染层改动。`types/clipboard.ts` 新增 `fileMeta` helper；`pages/Clipboard/ClipboardItem.tsx` 外层改 `flex items-center gap-2.5`，类型图标提为跨两行居中的「头像」列（兼复制入口），右侧内容栏分两行；操作组重排、复制居首。不改后端 / 数据库 / `item_type`，不破坏 `memo` 性能特性。

**Tech Stack:** TypeScript + React 19 + Vite + Tailwind v4。校验 `npx tsc -b`；视觉核验用 chrome-devtools（隔离预览 `preview.html`）。

**约定：** 除非另注，所有 shell 命令在 `crates/desktop/frontend/` 目录下执行；文件路径相对仓库根。

---

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/desktop/frontend/src/types/clipboard.ts` | 既有剪贴板类型 + 共享工具。新增 `fileMeta(item)` 文件类元数据 helper | 修改（追加） |
| `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | 历史行组件。重构为两行布局、图标跨行作头像、操作组复制居首 | 修改（重写 return） |
| `crates/desktop/frontend/preview.html` | 隔离预览入口（核验用，临时） | 新建 → 删除 |
| `crates/desktop/frontend/src/__preview__/clipboard-preview.tsx` | 渲染 5 类型样例，polyfill `__TAURI_INTERNALS__` | 新建 → 删除 |
| `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx` | 管理面板行组件 `ClipboardRow`。两行布局同步重构（保留 checkbox 多选 + 左缘色条） | 修改（重写 return） |
| `crates/desktop/frontend/preview-settings.html` | 管理页隔离预览入口（核验用，临时） | 新建 → 删除 |
| `crates/desktop/frontend/src/__preview__/clipboard-panel-preview.tsx` | 渲染 `ClipboardPanel`，mock invoke + event listen | 新建 → 删除 |

---

## Task 1: `fileMeta` helper

**Files:**
- Modify: `crates/desktop/frontend/src/types/clipboard.ts`（`imageMeta` 之后追加）

- [x] **Step 1: 实现 `fileMeta`**

  在 `imageMeta` 之后追加，与 `metaParts` / `imageMeta` 并列：

  ```ts
  export function fileMeta(item: ClipboardItem): string {
    const files = item.meta_info?.files;
    if (!files || files.length === 0) return "";
    const firstType = files.map((f) => f.type).find(Boolean);
    if (files.length === 1) return firstType || "";
    return firstType ? `${files.length}个 · ${firstType}` : `${files.length}个`;
  }
  ```

- [x] **Step 2: 校验类型**

  `npx tsc -b` → exit 0。

---

## Task 2: ClipboardItem 两行布局重写

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`

- [x] **Step 1: import 补 `fileMeta`**

  `import { metaParts, typeAccent, imageMeta, fileMeta, detectUrl } from "@/types/clipboard";`

- [x] **Step 2: 计算行尾元数据**

  按类型三选一：

  ```tsx
  const row1Meta =
    item.item_type === "image" ? imageMeta(item)
    : item.item_type === "file" ? fileMeta(item)
    : metaParts(item);
  ```

- [x] **Step 3: 外层改 flex 两列**

  外层容器 className 追加 `flex items-center gap-2.5`。voice 左侧加 `w-[2px] bg-voice/50` 竖条。

- [x] **Step 4: 图标提为「头像」列**

  类型图标改为 `<button onClick={handleCopy} title="单击复制">`，`w-5 h-5`、`flex-shrink-0`、
  跨两行垂直居中（靠外层 `items-center`）。`copied` 时图标 `scale-125 text-emerald-500` 并弹「已复制」气泡。

- [x] **Step 5: 内容栏两行**

  `flex-1 min-w-0` 内容栏：
  - 第一行 `flex items-center gap-2`：内容块（image→缩略图 / file→`formatFilePaths` / 其余→`line-clamp-1` 预览）+ `row1Meta`；
  - 第二行 `mt-1 flex items-center justify-between`：时间戳（`{item.created_at}`，完整 `YYYY-MM-DD HH:MM:SS`）+ 操作组。

- [x] **Step 6: 操作组复制居首**

  操作组顺序：复制 → 打开链接(`isUrl`) → 编辑/预览/保存/打开文件 → 删除 → 收藏。
  将原位于删除与收藏之间的复制按钮块移到操作组首位（`{isUrl && (` 之前）。

- [x] **Step 7: 校验**

  `npx tsc -b` → exit 0。

---

## Task 3: 视觉核验 + 脚手架清理

- [x] **Step 1: 隔离预览核验**

  新建 `preview.html` + `src/__preview__/clipboard-preview.tsx`（polyfill `__TAURI_INTERNALS__.invoke`，
  `get_image_thumb` 返回 canvas 生成的 mock PNG；5 类型样例时间戳用完整格式）。
  chrome-devtools 快照 + `evaluate_script` 量测确认：
  - 图标跨两行居中（各行 `btnCenter === rowCenter`，`iconW=20`）；
  - 元数据 `24字` / `23字 · 5.4s` / `36字` / `1920×1080 · 2.4M` / `3个 · fig`；
  - 时间戳完整；操作组每行以「复制」开头。

- [x] **Step 2: 删除浮窗预览脚手架**

  删除 `crates/desktop/frontend/preview.html` 与 `crates/desktop/frontend/src/__preview__/clipboard-preview.tsx`。

- [x] **Step 3: 关闭后台 dev server**

  停掉核验用的 vite dev server（后台任务）；管理页脚手架清理见 Task 5 Step 4。

---

## Task 4: 管理页 ClipboardRow — 准备

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx`

- [x] **Step 1: import `fileMeta`**

  `import { metaParts, typeAccent, imageMeta, fileMeta, detectUrl } from "@/types/clipboard";`

- [x] **Step 2: `meta` → `row1Meta` 三选一**

  ```tsx
  // 第一行行尾元数据：text/ocr→N字；voice→N字·Xs；image→W×H·size；file→类型/N个
  const row1Meta =
    item.item_type === "image" ? imageMeta(item)
    : item.item_type === "file" ? fileMeta(item)
    : metaParts(item);
  ```

---

## Task 5: 管理页 ClipboardRow — 两行 return + 核验 + 脚手架清理

- [x] **Step 1: 重写 return 为两行布局**

  外层 `items-start`→`items-center`、`py-2.5`→`py-2`；checkbox 与图标去 `mt` 偏移、图标 `w-3.5`→`w-4` 跨两行居中；内容栏拆两行（第一行 内容+元数据，第二行 时间戳+操作组）；操作组从右侧 rail 移入第二行、复制居首；去掉与内容块重复的链接文字 span；文本 `line-clamp-2`→`line-clamp-1`。

- [x] **Step 2: 校验**

  `npx tsc -b` → exit 0。

- [x] **Step 3: 隔离预览核验**

  新建 `preview-settings.html` + `src/__preview__/clipboard-panel-preview.tsx`（mock `query_clipboard_history` 返 5 类型样例 / `clipboard_stats` / `get_image_thumb` / `plugin:event|listen`）。chrome-devtools 快照 + `evaluate_script` 量测确认：行高 59/63/59/85/59px，每行 `checkboxCenter === iconCenter === rowCenter`，`iconW=20`，操作组右沿距行右沿 12px（无留白）；元数据 / 时间戳 / 复制居首顺序正确。

- [x] **Step 4: 删除管理页预览脚手架**

  已删除 `crates/desktop/frontend/preview-settings.html` 与 `crates/desktop/frontend/src/__preview__/` 整个目录。

- [x] **Step 5: 关闭后台 dev server**

  已停掉核验用的 vite dev server（1420 端口释放）。

---

## 收尾

- [x] 同步本文档与 spec 的 checkbox（代码已全部落地，Task 1/2/4/5 回标完成；Task 3/5 脚手架清理 + 关 server 已执行）。
- [ ] 桌面端真机回归（可选）：开剪贴板浮窗，确认 5 类型行渲染、复制 / 双击粘贴 / 删除 / 收藏交互正常。
