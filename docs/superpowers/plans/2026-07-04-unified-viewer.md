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

- [ ] architecture.md 更新
- [ ] spec/plan 回写
