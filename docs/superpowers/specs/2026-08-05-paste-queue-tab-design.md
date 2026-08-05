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
- 拖拽排序：HTML5 drag events（mousedown → drag → drop → `move_paste_stack_item(from, to)`）
- 底部：「清空队列」按钮
- 空队列：显示「队列为空」占位
- Cmd+Shift+V 粘贴后 `listen("paste-stack://updated")` 触发刷新

### 3.4 拖拽实现

用原生 HTML5 drag events（不引第三方库）：
```tsx
<li
  draggable
  onDragStart={() => setDragIndex(i)}
  onDragOver={(e) => e.preventDefault()}
  onDrop={() => { onMove(dragIndex, i); setDragIndex(null); }}
>
```

### 3.5 Tab 切换时的数据源

`filter === "queue"` 时：
- 不调 `useClipboardHistory`，改调 `invoke("peek_paste_stack")`
- `listen("paste-stack://updated")` → 重新 `invoke("peek_paste_stack")`

## 4. 不变量

1. 队列 tab 的顺序 = paste stack 的 FIFO 顺序（index 0 = 下一个弹出）
2. 拖拽后立即调 `move_paste_stack_item` → 后端 VecDeque 重排
3. 单条删除后剩余条目顺序不变
4. 清空后队列 tab 显示「空」占位 + 栈计数归零

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
