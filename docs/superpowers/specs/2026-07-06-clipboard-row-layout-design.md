# 剪贴板历史条目两行布局 — 设计

- 日期：2026-07-06
- 分支：clipboard-display-optimize
- 范围：桌面端前端（`crates/desktop/frontend`），仅渲染层

## 1. 背景与现状

剪贴板浮窗的历史列表每一行原先为**单行**结构：左侧类型图标 + 中间内容文本 + 右侧一排操作按钮。
操作按钮（编辑 / 删除 / 复制 / 收藏…）用 `opacity-0 group-hover:opacity-*` 平时隐藏、悬停才显形，
但它们**仍占据水平空间**，于是每行右侧常年留下一片无法消除的空白，视觉上非常不协调。

文本类条目尤其明显：内容被 `line-clamp-1` 截在一行，右边一大段留白；图片 / 文件类则连内容都
没有充分铺开。

## 2. 目标 / 非目标

**目标**

- 重构为**两行**布局，让内容铺满宽度、消除右侧大片空白。
- 第一行：类型图标 + 内容（或缩略图 / 文件路径）+ 行尾元数据。
- 第二行：时间戳 + 操作按钮（时间戳左、操作右）。
- 每种类型的第一行有统一且信息密度合适的格式。
- 操作行按使用频率排序，最常用的「复制」放第一位。
- 保持 50 行滚动时的渲染性能（`memo` + 稳定句柄不回退）。

**非目标**

- 不动后端、不动数据库 schema、不改 `item_type`。
- 不改双击粘贴（`paste_clipboard_item`）、单击选中、复制（`copy_clipboard_item`）等既有交互语义。
- 不重做配色 / 主题（沿用既有 stone 暖色板 + 类型强调色）。

## 3. 设计

### 3.1 整体结构：图标列 + 两行内容栏

外层行容器改为 `flex items-center gap-2.5`，横向分两列：

1. **图标列（「头像」）**：类型图标本身是一个 `<button>`，`onClick` 触发复制（`title="单击复制"`）。
   尺寸放大到 `w-5 h-5`，靠外层 `items-center` **跨两行垂直居中**，既是类型指示、又是随手可点的复制入口。
   复制成功时图标闪绿（`scale-125 text-emerald-500`）并在右侧弹「已复制」气泡 1.5s。
2. **内容栏 `flex-1 min-w-0`**：内含两行。
   - 第一行 `flex items-center gap-2`：`flex-1 min-w-0` 的内容块 + 行尾元数据（`flex-shrink-0`）。
   - 第二行 `mt-1 flex items-center justify-between`：左侧时间戳 + 右侧操作组。

voice 条目额外在最左侧加一条 `w-[2px]` 琥珀色竖条作类型标识。

### 3.2 第一行：按类型的格式

| 类型 | 第一行内容块 | 行尾元数据 |
|---|---|---|
| text | 文本预览（`line-clamp-1`） | `N字` |
| voice | 转写文本（`line-clamp-1`） | `N字 · Xs` |
| ocr | 识别文本（`line-clamp-1`） | `N字` |
| image | 缩略图（`w-10 h-10 rounded-md object-cover`） | `W×H · size` |
| file | 文件路径尾段（`formatFilePaths`） | `N个 · 首类型` / 单个则仅类型 |

元数据用各类型的强调色（`typeAccent`：text=stone-500、voice=amber-600、ocr=teal-600、
image=indigo-500、file=emerald-600），`text-[10px] tabular-nums`。

### 3.3 新增 helper：`fileMeta`

`types/clipboard.ts` 在既有 `imageMeta` 之后新增 `fileMeta(item)`，集中文件类元数据生成逻辑：

```ts
export function fileMeta(item: ClipboardItem): string {
  const files = item.meta_info?.files;
  if (!files || files.length === 0) return "";
  const firstType = files.map((f) => f.type).find(Boolean);
  if (files.length === 1) return firstType || "";
  return firstType ? `${files.length}个 · ${firstType}` : `${files.length}个`;
}
```

与 `metaParts`（text/ocr/voice）、`imageMeta`（image）并列，`ClipboardItem` 按类型三选一取行尾元数据。

### 3.4 第二行：时间戳 + 操作组

- **时间戳**：直接渲染后端 `created_at`（格式恰为 `YYYY-MM-DD HH:MM:SS`，来自 `store::iso_now()`），
  完整年月日时分秒，`text-[10px] tabular-nums text-muted-foreground/60`。第二行足够宽，无需截断。
- **操作组**（`flex items-center gap-0.5`，多数按钮 `opacity-0 group-hover:opacity-60` 悬停显形），
  按使用频率从左到右排序：
  1. **复制**（居首，最常用；图标已作复制入口，这里补一个显式带 `title` 的按钮）
  2. 打开链接（仅 `text` 且 `detectUrl` 判定为链接时）
  3. 编辑（text/voice/ocr）/ 预览（image）/ 保存为文件（image）/ 打开文件（file）
  4. 删除（两段式：首次点击进入待确认态 1.5s，再次点击才删）
  5. 收藏（已收藏时常驻显形 `fill-amber-400`）

第二行整行 `onDoubleClick` 阻止冒泡，避免在操作区误触发整行的「双击粘贴」。

### 3.5 性能保持

行组件仍是 `memo(ClipboardItemRow)`，配合 `index.tsx` 稳定的 `onSelect`（`useCallback`）与
`refresh` 句柄：选中行切换时仅新旧两行 `isSelected` 变化、触发重绘，其余行 props 浅比较不变即跳过。
本次布局重构不引入任何 inline 句柄或新 state，性能特性不变。

## 4. 验证（浮窗）

- `npx tsc -b` 通过。
- 隔离预览（`preview.html` + `src/__preview__/clipboard-preview.tsx`，polyfill `__TAURI_INTERNALS__`，
  canvas 生成 mock 缩略图）渲染 5 种类型样例，chrome-devtools 快照 + `evaluate_script` DOM 量测确认：
  - 图标列跨两行垂直居中（各类型行 `btnCenter === rowCenter`，`iconW=20`）；
  - 行尾元数据正确：`24字` / `23字 · 5.4s` / `36字` / `1920×1080 · 2.4M` / `3个 · fig`；
  - 时间戳完整：`2026-07-06 09:42:15`；
  - 操作组顺序：每行均以「复制」开头。

> 预览脚手架为核验用临时产物，定稿后删除（见 plan Task 3）。

## 5. 管理页（Settings/ClipboardPanel）同步

设置页「剪贴板」管理面板（`pages/Settings/ClipboardPanel.tsx` 的 `ClipboardRow`）原先也是
**单行**：左缘色条 + checkbox + 图标 + 内容（含一行半元信息）+ 右侧 hover 操作 rail，
同样存在「右侧操作组 `opacity-0` 占位留白」。按浮窗两行模式同步重构。

### 5.1 与浮窗的差异（管理页特有，保留）

- **checkbox 多选**：行首 checkbox（批量选中 / 批量删除），整行 `onClick=onToggleSelect`。
  外层改 `items-center`，让 checkbox 与图标跨两行垂直居中。
- **左缘类型色条**：`w-[3px]` 类型色（voice 琥珀渐变、其余类型低饱和色），选中变 `stone-900`、
  删除确认变 `red-500`——管理页的色彩编码签名，保留。
- 行操作按钮 padding 沿用管理页 `p-1`（浮窗为 `p-0.5`），尺寸不照搬。

### 5.2 两行结构（对齐浮窗）

外层 `flex items-center gap-2.5 pl-4 pr-3 py-2 border-b`：

1. 左缘色条（absolute）；
2. checkbox（`flex-shrink-0`，跨两行居中）；
3. 类型图标 button（单击复制，`w-4` 跨两行居中，copied 闪绿 + 气泡）；
4. 内容栏 `flex-1 min-w-0`：
   - 第一行 `flex items-center gap-2`：内容（image→缩略图 / file→路径 / 其余→`line-clamp-1` 文本）+ 行尾元数据；
   - 第二行 `mt-1 flex items-center justify-between`：时间戳 + 操作组（复制居首）。

### 5.3 精简

- 文本内容由原 `line-clamp-2` 收为 `line-clamp-1`（两行布局下第一行只占一行视觉）。
- 去掉元信息行里与内容块重复的链接文字预览（text 类内容块已含完整链接文本），
  链接仅留「打开链接」按钮，与浮窗一致。

### 5.4 验证（管理页）

隔离预览（`preview-settings.html` + `src/__preview__/clipboard-panel-preview.tsx`，mock
`query_clipboard_history` / `clipboard_stats` / `get_image_thumb` / `plugin:event|listen`）：
- 5 类型行结构正确，元数据 `24字` / `23字 · 5.4s` / `36字` / `1920×1080 · 2.4M` / `3个 · fig`；
- 时间戳完整；操作组每行复制居首；
- DOM 量测：行高 59/63/59/85/59px，每行 `checkboxCenter === iconCenter === rowCenter`
  （跨两行居中），`iconW=20`，操作组右沿距行右沿 12px（= `pr-3`，无留白）。

> 管理页预览脚手架同为核验用临时产物，定稿后删除（见 plan Task 5）。
