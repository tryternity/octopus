# 统一内容查看器实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development 或 superpowers:executing-plans 逐任务实施。

**Goal:** CompactEditor 升级为统一内容查看器——tab 切换文本/图片/语音，取代独立 ImagePreview 窗口。

**Architecture:** Tab key=`source:itemId`；图片 tab 嵌入 ImagePreview 组件（hidden 保持挂载 ≤5）；语音 tab 只读；窗口 880×620 默认+可调记忆。

**Tech Stack:** Rust（Tauri 2 命令/窗口）、React 19 + TypeScript。

---

## Task 1: 后端 — 窗口尺寸 + 记忆 + get_transcription_text

**Files:**
- Modify: `crates/desktop/src/compact_editor_window.rs`（尺寸 880×620 + 记忆）
- Modify: `crates/desktop/src/compact_editor_commands.rs`（open 加 source 参数）
- Create/Modify: `crates/desktop/src/main.rs`（注册新命令 + Destroyed 记忆）
- Modify: `crates/infra/src/db.rs`（窗口记忆读写）

- [ ] **Step 1：compact_editor_window.rs 尺寸改 880×620 + 读记忆**

- [ ] **Step 2：compact_editor_commands.rs open_compact_editor_tab 加 source 参数**

```rust
pub fn open_compact_editor_tab(
    item_id: i64,
    source: Option<String>,
    app_handle: AppHandle,
)
```

source 写入 PENDING_TAB，前端 mount 读。

- [ ] **Step 3：get_transcription_text 命令**

```rust
#[tauri::command]
pub fn get_transcription_text(id: i64) -> Result<String, String>
```

读 transcriptions 表的 text 列（segments 合并后的全文）。

- [ ] **Step 4：窗口记忆持久化（Destroyed 时写 DB）**

- [ ] **Step 5：编译 + 提交**

---

## Task 2: 前端 — Tab 模型升级 + 内容区渲染

**Files:**
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`

- [ ] **Step 1：Tab 接口升级**

```ts
interface Tab {
  key: string;           // `${source}:${itemId}`
  source: 'clipboard' | 'transcription';
  itemId: number;
  itemType?: 'text' | 'image';
  text?: string;
}
```

- [ ] **Step 2：loadAndAddTab 升级**

接受 source + itemId，查 item_type（clipboard 时调 get_item_type 或从 get_clipboard_item_text 返回值推断），transcription 时调 get_transcription_text。

- [ ] **Step 3：图片 tab ≤5 限制**

新增图片 tab 时检查图片 tab 数量，超 5 删最旧。

- [ ] **Step 4：内容区渲染（hidden 挂载）**

```tsx
{tabs.map((tab, i) => (
  <div key={tab.key} style={{ display: i === activeIdx ? 'flex' : 'none', flex: 1 }}>
    {tab.source === 'transcription' ? (
      <textarea readOnly value={tab.text} />
    ) : tab.itemType === 'image' ? (
      <ImagePreviewComponent imageId={tab.itemId} />
    ) : (
      <textarea value={tab.text} onChange={...} />
    )}
  </div>
))}
```

- [ ] **Step 5：tab 栏图标（文字/图片/语音区分）**

- [ ] **Step 6：构建 + 提交**

---

## Task 3: 前端 — ImagePreview 改为可控组件

**Files:**
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/index.tsx`
- Modify: `crates/desktop/frontend/src/App.tsx`（删 image_preview_window 路由）

- [ ] **Step 1：ImagePreview 接受 props**

```tsx
export default function ImagePreview({ imageId: propImageId }: { imageId: number }) {
```

去掉 `get_pending_image` / `listen("image-preview://load")`，用 propImageId 驱动 imageId state。

- [ ] **Step 2：App.tsx 删除 image_preview_window 路由 case**

- [ ] **Step 3：保留 listen("ocr-screenshot://result")**

- [ ] **Step 4：构建 + 提交**

---

## Task 4: 后端 — ocr_screenshot + 入口统一

**Files:**
- Modify: `crates/desktop/src/screenshot_commands.rs`
- Modify: `crates/desktop/src/clipboard_commands.rs`（open_image_preview 改路由到 compact_editor）
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`
- Modify: `crates/desktop/frontend/src/lib/compactEditor.ts`

- [ ] **Step 1：ocr_screenshot 改为 open_compact_editor_tab（不再开预览窗）**

- [ ] **Step 2：ClipboardItem 图片「预览」改为 openCompactEditorTab**

- [ ] **Step 3：openCompactEditorTab helper 加 source 参数**

- [ ] **Step 4：编译 + 构建 + 提交**

---

## Task 5: 废弃 ImagePreview 窗口

**Files:**
- Modify: `crates/desktop/src/main.rs`（移除 image_preview 命令注册 + Destroyed 路由）
- Modify: `crates/desktop/capabilities/default.json`（移除 image_preview_window）

- [ ] **Step 1：移除 image_preview 命令注册**

- [ ] **Step 2：移除 image_preview_window ACL**

- [ ] **Step 3：编译 + 提交**

---

## Task 6: 文档同步

- [x] architecture.md 更新
- [x] spec/plan 回写

---

## 实施记录（回写）

| Task | 状态 | commit | 说明 |
|------|------|--------|------|
| 1. 后端窗口+命令 | ✅ | `c1c4d8f` | 880×620 + source + get_transcription_text + get_clipboard_item_type |
| 2. 前端 Tab 模型 | ✅ | `776bf74` | source/itemType/图片≤5/hidden挂载/tab图标 |
| 3. ImagePreview 可控组件 | ✅ | `684e156` `1e9288a` | props imageId + fixed→absolute + h-full + bg-background |
| 4. 入口统一 | ✅ | `3286885` `83a6548` | ClipboardItem 预览→compactEditor + 删 OCR 按钮 |
| — 截图 OCR 防重复 | ✅ | `bf8fc92` | ocrDoneRef |
| — 工具栏 top 6px | ✅ | `11ff198` `e10f846` | 黑边对称 |
| 5. 废弃 ImagePreview 窗口 | ✅ | `1928e62` | image_preview 命令注册/ACL/activation 移除 |
| — 窗口尺寸+位置记忆 | ✅ | `3d27e53` `0e9a042` `c4eca38` | CloseRequested 保存 + DPR 缩放 + maximized |
| — 语音管理查看入口 | ✅ | `0266534` | HistoryPanel 加查看按钮→openCompactEditorTab(id,"transcription") |
| — 截图 OCR tab 顺序 | ✅ | `0266534` | 图片 tab 在前，文本 tab 活跃 |
| 6. 文档同步 | ✅ 本 z-sync | — | — |

**架构决策**：图片 tab 背景从暗灯箱 `#18181b` 改为 `bg-background`（与文本 tab 白底统一）。ImagePreview 不再用 fixed/Esc/暗区关闭（嵌入 tab 内不需要）。剪贴板图片条目删除独立 OCR 按钮（统一在图片预览 tab 工具栏 OCR）。

### 后续优化（z-sync 补记）

| 改动 | commit |
|------|--------|
| 语音识别记录文本截断 200 字 + …… 省略 | `dde0529` |
| 识别记录 + 剪贴板管理页全选 header sticky 固定 | `41c35b1` |

### VAD 驱动波纹（#3.4 后续优化）

| 改动 | commit |
|------|--------|
| PipelineEvent::Speaking(bool) + pipeline emit + 前端 200ms 防抖 | `b8fe71f` |
| 删 renderResultNow 里残留的 setIsSpeaking(true) | `fd71550` |
| VadSegmented has_speech 门控（开口后才亮） | `7d0fdb6` |
| Speaking 事件单元测试 | `3257ddc` |
| 前端 payload 提取修复（Tauri bool 被 wrap） | `9d6071d` |

**经验教训（Tauri bool 事件 payload 被 wrap）**：
- 后端 `emit("event", true)` → Tauri v2 在 WKWebView 中把 bool 序列化时多包一层 `{ event, payload: true, id }`
- 前端 `listen` 的 `rawListen` callback `e.payload` 拿到的是整个 Event 对象而非裸 bool
- **防御性提取**：`typeof payload === "boolean" ? payload : (payload as any)?.payload ?? false`
- 后续 Tauri 事件传 bool 时统一用此模式，或改用 string/number（`emit("event", speaking ? 1 : 0)`）

### 代码审查修复（第二轮）

| 改动 | commit |
|------|--------|
| CompactEditor 查找模式打字光标拽回顶部 | `54c508f` |
| CompactEditor 查找匹配 debounce 150ms | `56af793` |
| CancelEdit 清 pending_delete（防幽灵删除） | `c92955e` |
| result_window 多屏不同缩放率穿透失效 | `2f4690b` |

**动机**：识别记录管理页单条文本可达数百字，全文铺开致列表过长、不便浏览；截断至 200 字 +「……」后，靠条目「查看」按钮经统一查看器 transcription 只读 tab（`openCompactEditorTab(id,'transcription')`）看全文兜底（截断仅识别记录管理页 HistoryPanel，剪贴板管理页 ClipboardPanel 不截断）。sticky header 让全选复选框在长列表滚动时始终可见。
