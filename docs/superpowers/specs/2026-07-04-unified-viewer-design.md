# 统一内容查看器设计（CompactEditor 升级）

> 日期：2026-07-04
> 状态：📋 设计中
> 前置：`2026-07-03-image-viewer-perf-design.md`（图片预览视口渲染 + SVG overlay）、`2026-07-04-ocr-text-blocks-design.md`（OCR 文本块可视化）
> 分支：`image-viewer-perf`

## 1. 背景与目标

当前三个独立窗口：
- **CompactEditor**（720×560）：多 tab 文本编辑器，绑定 clipboard item_id
- **ImagePreview**（880×620）：图片预览 + 标注 + OCR
- **Result**（语音结果窗）：语音识别实时显示

OCR 后弹出两个窗口（编辑器 + 预览）信息分散；图片预览和文本编辑功能重叠（标注/OCR/保存/复制都两边都有）。

**目标**：将 CompactEditor 升级为**统一内容查看器**——同一窗口内 tab 切换文本/图片/语音条目。取代独立的 ImagePreview 窗口。

## 2. 范围

**做：**
- CompactEditor tab 模型升级：`{ key: source+itemId, source: 'clipboard'|'transcription', itemId }`
- 图片 tab：嵌入 ImagePreview 组件（非独立路由），hidden 保持挂载，≤5 个，超过替换最旧
- 语音 tab：只读 textarea（读 transcriptions 表）
- 窗口尺寸默认 880×620，可调 + 记忆（WindowState）
- 后端：`get_transcription_text(id)` 命令、窗口记忆持久化
- 入口统一：剪贴板预览、OCR 结果、截图 OCR、语音识别管理条目 → 全部 `open_compact_editor_tab`
- `image_preview_window.rs` / `image_preview_commands.rs` 废弃（功能合并到 CompactEditor）
- `ocr_screenshot` 改为 `open_compact_editor_tab` + emit blocks（不再开预览窗）

**不做（YAGNI）：**
- 语音 tab 编辑（只读）
- Result 窗口合并（实时识别窗保持独立）
- 语音 tab 同步 Result 窗（下次打开自然读新内容）
- 文本 tab 拖拽排序

## 3. 架构

### 3.1 Tab 模型

```ts
interface Tab {
  key: string;           // `${source}:${itemId}` 唯一标识
  source: 'clipboard' | 'transcription';
  itemId: number;
  itemType?: 'text' | 'image';  // 仅 source=clipboard 有，决定渲染 textarea 还是 ImagePreview
  text?: string;         // 文本 tab 的内容缓存
}
```

**tab 栏渲染**：
- 文本 tab：Type 图标 + 文本前 5 字
- 图片 tab：Eye 图标 + "图片" 标签（或缩略图）
- 语音 tab：Mic 图标 + 文本前 5 字 + 🔒 只读标记

**图片 tab 限制**：≤5 个，超过时替换最旧的图片 tab（新 tab 加入后检查并 close 最旧的）。

**内容区渲染**：
```tsx
{tabs.map((tab, i) => (
  <div key={tab.key} style={{ display: i === activeIdx ? 'block' : 'none' }}>
    {tab.source === 'transcription' ? (
      <textarea readOnly value={tab.text} />
    ) : tab.itemType === 'image' ? (
      <ImagePreview imageId={tab.itemId} />
    ) : (
      <textarea value={tab.text} onChange={...} />
    )}
  </div>
))}
```

**hidden 保持挂载**：用 `display: none` 切换，不 mount/unmount。图片 tab 切换回来时 canvas/SVG 状态保持。

### 3.2 ImagePreview 改为组件

当前 ImagePreview 是 `App.tsx` 路由 `image_preview_window` 的页面组件。改为：

1. **保留** `pages/ImagePreview/index.tsx` 作为**组件**（非路由页面）
2. **Props**：`imageId: number`（父传入，替代内部 PENDING/load 逻辑）
3. **去掉** mount 时 `get_pending_image` / `listen("image-preview://load")` —— 由父组件控制 imageId
4. **保留** `listen("ocr-screenshot://result")` —— 截图 OCR 推送 blocks
5. App.tsx 删除 `case "image_preview_window"` 路由

```tsx
// CompactEditor 内嵌入
{tab.itemType === 'image' && (
  <ImagePreviewComponent imageId={tab.itemId} />
)}
```

### 3.3 后端改动

**窗口**（`compact_editor_window.rs`）：
- 尺寸改为 880×620（min 400×320）
- 加 WindowState 记忆（位置/大小，持久化到 DB app_config 或独立表）
- 开窗时读记忆，无记忆用默认值

**新命令**：
```rust
#[tauri::command]
pub fn get_transcription_text(id: i64) -> Result<String, String> {
    // 读 transcriptions 表的 text/segments 合并文本
}
```

**open_compact_editor_tab 升级**：
```rust
pub fn open_compact_editor_tab(
    item_id: i64,
    source: Option<String>,  // "clipboard"（默认）| "transcription"
    app_handle: AppHandle,
)
```

source=transcription 时前端 tab 只读。

**废弃**：
- `image_preview_window.rs` / `image_preview_commands.rs`（open/get/close_image_preview 不再调用）
- `ocr_screenshot` 不再调 `open_image_preview`，改为 `open_compact_editor_tab(image_id, "clipboard")` + emit blocks

### 3.4 入口统一

| 入口 | 当前 | 改后 |
|------|------|------|
| 剪贴板文本「编辑」 | `open_compact_editor_tab(id)` | 不变 |
| 剪贴板图片「预览」 | `open_image_preview(id)` | `open_compact_editor_tab(id)` |
| 图片预览 OCR | 独立 ImagePreview 窗内 | CompactEditor 图片 tab 内 |
| 截图 OCR | 关窗→开编辑器+开预览+emit | 关窗→`open_compact_editor_tab(image_id)`+emit |
| 语音管理「查看」 | （新） | `open_compact_editor_tab(id, "transcription")` |
| OCR 结果入库 | `insert_ocr_clipboard_item` → `openCompactEditorTab` | 不变 |

### 3.5 ocr_screenshot 流程更新

```
截图点 OCR → ocr_screenshot
  → 识别 + 入库（clipboard_history image + ocr text）
  → 关截图窗
  → open_compact_editor_tab(image_id, "clipboard")  // 开图片 tab
  → open_compact_editor_tab(ocr_id, "clipboard")    // 开文本 tab
  → emit("ocr-screenshot://result", { text, blocks })  // 推送 OCR blocks
  → 图片 tab 的 ImagePreviewComponent listen 收到 → 显示叠加层
```

## 4. 窗口记忆

**WindowState**（`app_state.rs` 已有结构）：
```rust
pub struct WindowState {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub maximized: bool,
    pub fullscreen: bool,
}
```

**持久化**：存到 `app_config` 表（key = `compact_editor_window_state`，value = JSON）。

**开窗时**：读记忆 → `.inner_size(w, h).position(x, y)`，无记忆用 880×620 默认值。

**关窗时**：`on_window_event(Destroyed)` → 读当前窗口位置/大小 → 写 DB。

## 5. 边界情况

- **两表 ID 重复**：tab key = `source:itemId`，不冲突
- **图片 tab 超 5 个**：close 最旧的图片 tab（不是文本/语音 tab）
- **同一 item 重复打开**：tab key 匹配 → 切到该 tab（不开新）
- **语音 tab 关闭**：仅关视图，不删 DB 记录
- **截图 OCR 双 tab**：图片 tab + 文本 tab 同时开，文本 tab 在前（最近打开）

## 6. 不变量

1. Tab key = `${source}:${itemId}` 全局唯一
2. 图片 tab ≤ 5（hidden 保持挂载）
3. 语音 tab 只读（textarea readOnly）
4. 窗口尺寸可调 + 记忆（首次用 880×620 默认）
5. ImagePreviewComponent 由父控制 imageId（不再自主 PENDING/load）
