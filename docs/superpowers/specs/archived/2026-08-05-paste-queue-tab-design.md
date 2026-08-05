# 粘贴队列 Tab 设计

- 日期：2026-08-05
- 类型：增量功能（基于 paste stack 已有实现）
- 依赖：`paste_stack.rs`（push/pop/clear/status 已有）

## 1. 需求

在剪贴板浮窗 FilterTabs 加「队列」tab，展示 paste stack 当前内容，支持：
- 拖拽调整顺序
- 单条删除
- 清空全部

## 2. 后端改造

### 2.1 paste_stack.rs 新增 API

```rust
/// 读取整个队列内容（按 FIFO 顺序，front=先出）。
/// 返回 history_id 列表 + 每条的预览（复用 status 的 DB 查询逻辑）。
pub fn peek_all() -> Vec<PasteStackItem>

pub struct PasteStackItem {
    pub history_id: String,
    pub item_type: String,
    pub preview: String,       // 内容前 50 字符
}

/// 删除指定位置的条目（0 = front）。越界返 Err。
pub fn remove_at(index: usize) -> Result<(), String>

/// 移动条目：from → to（insert at to，remove from from）。
pub fn move_item(from: usize, to: usize) -> Result<(), String>
```

### 2.2 Tauri 命令

| 命令 | 签名 |
|---|---|
| `peek_paste_stack` | `() -> Vec<PasteStackItem>` |
| `remove_from_paste_stack` | `(index: usize) -> ()` |
| `move_paste_stack_item` | `(from: usize, to: usize) -> ()` |

已有的 `clear_paste_stack` / `paste_stack_status` 不变。

## 3. 前端

### 3.1 FilterTabs 加「队列」tab

`FilterTabs.tsx` 的 `TAB_DEFS` 数组末尾加：
```ts
{ value: "queue", icon: Layers, labelKey: "clipboard.filter.queue", svg: undefined }
```

### 3.2 队列 tab 内容（index.tsx）

当 `filter === "queue"` 时，渲染队列列表而非历史列表：

```tsx
{filter === "queue" ? (
  <PasteQueueView
    items={queueItems}
    onRefresh={refreshQueue}
    onRemove={handleRemove}
    onMove={handleMove}
    onClear={handleClear}
  />
) : (
  /* 正常历史列表 */
)}
```

### 3.3 PasteQueueView 组件

- 每行：序号 + 类型图标 + 内容预览 + 删除按钮（×）
- 拖拽排序：**@dnd-kit/sortable**（DndContext + SortableContext + useSortable）→ `move_paste_stack_item(from, to)`
- 底部：「清空队列」按钮
- 空队列：显示「队列为空」占位
- Cmd+Shift+V 粘贴后 `listen("paste-stack://updated")` 触发刷新
- **hover 详情 overlay**：独立 hoverIndex state，hover 时按 historyId 查完整 ClipboardItem（`get_clipboard_item` 命令），显示 content 前 500 字符 / image 缩略图，与 history tab overlay 一致（复用 HoverOverlay 组件）
- **入栈后自动切 tab**：`handlePushToStack` 末尾 `setFilter("queue")`

### 3.4 拖拽实现（⚠️ 已从 HTML5 DnD 改为 @dnd-kit）

**初版用 HTML5 drag events（`draggable`/`onDragStart`/`onDrop`），e2e 验证发现完全不生效**——AGENTS.md gotcha 明确记载「HTML5 DnD 在 WKWebView 不可靠，xterm canvas 场景 bubble+capture 都试过 onDrop 不触发」。队列 tab 虽无 xterm canvas，但 WKWebView 对 HTML5 DnD 支持普遍有问题。

**改用 `@dnd-kit/sortable`**（业界主流 React DnD 库，~14KB gzip）：
```tsx
<DndContext sensors={[PointerSensor({activationConstraint:{distance:8}})]}
            collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
  <SortableContext items={ids} strategy={verticalListSortingStrategy}>
    {items.map((item, i) => <SortableQueueItem key={item.historyId} item={item} index={i} />)}
  </SortableContext>
</DndContext>

// SortableQueueItem 用 useSortable 拿 transform/transition/listeners
const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
```

关键点：
- `PointerSensor` 设 8px 激活距离避免点击误判拖拽
- 删除按钮 `onPointerDown stopPropagation`——dnd-kit listeners 拦截整个 li 的 pointer 事件，按钮必须 stopPropagation 才能让 onClick 触发
- `onDragEnd` 用 findIndex 定位 from/to → `move_paste_stack_item`

### 3.5 hover 详情 overlay（独立数据源，不复用父 previewItem）

**已踩坑**：初版复用父组件的 `selectedIndex`/`previewItem`/`items`（history 数据源），但 queue 渲染的是 `queueItems`，两套 index 不对应 → overlay 永远盖第一个条目（selectedIndex=0 但 queue 列表没有 `data-clip-index` 属性，定位 fallback 到 `'0px'`）。

**修复**：
1. `useClipboardHistory` 在 `filter === "queue"` 时不查后端（后端 `build_where` 把未知 filter 退化为 "all" 会返回全部历史，污染状态）
2. QueueListView 内部独立 `hoverIndex` state，基于 queueItems
3. hover 变化时按 historyId 调 `get_clipboard_item` 查完整 ClipboardItem（PasteStackItemDto 只有 preview 前 50 字符，overlay 需 500 字符与 history 一致）
4. image 类型再查 `get_image_thumb` 缩略图
5. 父组件 overlay 渲染条件加 `filter !== "queue"`（queue overlay 在 QueueListView 内部渲染，避免双 overlay）

### 3.6 Tab 切换时的数据源

`filter === "queue"` 时：
- `useClipboardHistory` 早退（不查后端，items 保持空）
- QueueListView 调 `invoke("peek_paste_stack")`
- `listen("paste-stack://updated")` → 重新 `invoke("peek_paste_stack")`

### 3.7 FilterTabs 文字显隐（动态阈值）

`COMPACT_THRESHOLD = 6`：tab 数 > 6 时所有 tab 只显图标（靠 `title` 出 tooltip），不再硬编码「全部」tab 总显文字。2026-08-05 tab 数达 8 个后「全部 + 图标」组合挤到换行。

## 4. 不变量

1. 队列 tab 的顺序 = paste stack 的 FIFO 顺序（index 0 = 下一个弹出）
2. 拖拽后立即调 `move_paste_stack_item` → 后端 VecDeque 重排
3. 单条删除后剩余条目顺序不变
4. 清空后队列 tab 显示「空」占位 + 栈计数归零
5. queue tab 的 hover overlay 数据源 = queueItems（独立于 history tab 的 items/selectedIndex/previewItem）

## 5. i18n

`zh-CN.yaml` / `en.yaml` 加：
```yaml
clipboard:
  filter:
    queue: 队列 # Queue
  queue:
    empty: 队列为空 # Queue is empty
    clear: 清空队列 # Clear queue
```

## 6. 实现注记（2026-08-05，与初始设计的偏差）

### 6.1 拖拽：HTML5 DnD → @dnd-kit（§3.4）

初版按本 spec §3.4 用 HTML5 drag events，e2e 验证完全不生效。AGENTS.md gotcha 已记载 WKWebView HTML5 DnD 不可靠（xterm canvas 场景 bubble+capture 都试过）。改为 `@dnd-kit/core` + `@dnd-kit/sortable` + `@dnd-kit/utilities`（共 ~14KB gzip）。spec §3.4 已更新为 @dnd-kit 实际实现。

### 6.2 hover overlay：独立数据源（新增 §3.5）

初版没有 hover overlay。第一版尝试复用父组件 previewItem，但 queue 数据源（queueItems）与 history 数据源（items）index 不对应，导致 overlay 永远盖第一个条目。最终 QueueListView 内部独立 hoverIndex + 新增 `get_clipboard_item` 命令按 id 查完整 ClipboardItem。spec §3.5 已记录。

### 6.3 tab 文字：动态阈值（新增 §3.7）

初版「全部」tab 硬编码总显文字。8 个 tab 时挤到换行。改为 `COMPACT_THRESHOLD = 6` 动态阈值。spec §3.7 已记录。

### 6.4 入栈后自动切 tab

`handlePushToStack` 末尾加 `setFilter("queue")`——用户反馈需求，入栈后立即聚焦到队列 tab 看入栈结果。
