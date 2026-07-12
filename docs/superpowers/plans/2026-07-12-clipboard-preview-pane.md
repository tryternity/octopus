# 剪贴板预览面板 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为剪贴板浮窗新增独立预览窗口，跟随 hover / 键盘选中条目实时展示完整内容。

**Architecture:** 独立 Tauri 窗口（无边框圆角透明置顶），浮窗有焦点时常驻，失焦隐藏。Rust 侧根据浮窗屏幕位置计算预览窗口左/右弹出坐标。选中条目变化时 Rust 从 DB 读完整内容 emit 给预览窗口前端。

**Tech Stack:** Tauri 2 + Rust (后端), React 19 + TypeScript (前端)

## Global Constraints

- 预览窗口与浮窗同风格：无边框、圆角、透明、置顶
- 预览窗口不抢焦点
- 坐标计算统一逻辑坐标（物理 ÷ scale_factor）
- 预览内容只读（不可编辑）
- 不改变剪贴板浮窗现有布局和尺寸

---

## File Structure

| 文件 | 职责 | 操作 |
|------|------|------|
| `crates/desktop/src/clipboard_preview_window.rs` | 预览窗口创建/显示/隐藏/定位/更新 | 创建 |
| `crates/desktop/src/main.rs` | 注册模块 + 命令 + 窗口事件 | 修改 |
| `crates/desktop/capabilities/default.json` | 预览窗口权限 | 修改 |
| `crates/desktop/frontend/src/pages/ClipboardPreview/index.tsx` | 预览窗口前端组件 | 创建 |
| `crates/desktop/frontend/src/App.tsx` | 路由 | 修改 |
| `crates/desktop/frontend/src/pages/Clipboard/index.tsx` | 选中变化时触发预览更新 | 修改 |
| `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | hover 时触发预览更新 | 修改 |

---

## Task 1: 预览窗口 Rust 模块 + 定位逻辑

**Files:**
- Create: `crates/desktop/src/clipboard_preview_window.rs`
- Modify: `crates/desktop/src/main.rs`
- Modify: `crates/desktop/capabilities/default.json`

**Interfaces:**
- Produces: `clipboard_preview_window::create_preview_window(app)`, `show_preview_window(app)`, `hide_preview_window(app)`, `update_clipboard_preview(app, id)`, `compute_preview_position(clip_win) -> (f64, f64)`

- [ ] **Step 1: 创建 clipboard_preview_window.rs**

```rust
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use octopus_infra::db;

pub const WINDOW_LABEL: &str = "clipboard_preview_window";
const PREVIEW_W: f64 = 360.0;
const PREVIEW_H: f64 = 500.0;
const GAP: f64 = 8.0;

/// 创建预览窗口（透明无边框置顶，初始不可见）
pub fn create_preview_window(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(WINDOW_LABEL).is_some() {
        return Ok(());
    }
    let _window = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("预览")
    .inner_size(PREVIEW_W, PREVIEW_H)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .transparent(true)
    .shadow(false)
    .visible(false)
    .build()?;
    Ok(())
}

/// 根据剪贴板浮窗位置计算预览窗口坐标（逻辑坐标）
pub fn compute_preview_position(
    clip_x: f64, clip_y: f64, clip_w: f64, clip_h: f64,
    screen_w: f64,
) -> (f64, f64) {
    let right_space = screen_w - (clip_x + clip_w);
    if right_space >= PREVIEW_W + GAP {
        (clip_x + clip_w + GAP, clip_y)
    } else if clip_x >= PREVIEW_W + GAP {
        (clip_x - PREVIEW_W - GAP, clip_y)
    } else if right_space >= clip_x {
        (clip_x + clip_w + 4.0, clip_y)
    } else {
        (clip_x - PREVIEW_W - 4.0, clip_y)
    }
}

/// 显示预览窗口（定位 + show）
pub fn show_preview_window(app: &AppHandle) {
    let Some(clip_win) = app.get_webview_window("clipboard_window") else { return };
    let Some(preview_win) = app.get_webview_window(WINDOW_LABEL) else { return };

    // 计算位置（逻辑坐标）
    let scale = clip_win.scale_factor().unwrap_or(1.0);
    let clip_pos = clip_win.outer_position().ok();
    let clip_size = clip_win.outer_size().ok();
    let monitor = clip_win.current_monitor().ok().flatten();
    let (px, py) = match (clip_pos, clip_size, monitor) {
        (Some(pos), Some(size), Some(mon)) => {
            let cx = pos.x as f64 / scale;
            let cy = pos.y as f64 / scale;
            let cw = size.width as f64 / scale;
            let ch = size.height as f64 / scale;
            let sw = mon.size().width as f64 / scale;
            compute_preview_position(cx, cy, cw, ch, sw)
        }
        _ => (0.0, 0.0),
    };

    let _ = preview_win.set_position(tauri::Position::Logical(
        tauri::LogicalPosition::new(px, py),
    ));

    // 预览窗口高度跟随浮窗高度
    if let Some(size) = clip_win.outer_size().ok() {
        let h = size.height as f64 / scale;
        let _ = preview_win.set_size(tauri::Size::Logical(
            tauri::LogicalSize::new(PREVIEW_W, h),
        ));
    }

    let _ = preview_win.show();
}

/// 隐藏预览窗口
pub fn hide_preview_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(WINDOW_LABEL) {
        let _ = win.hide();
    }
}

/// Tauri 命令：前端选中/hover 变化时调用，从 DB 读完整内容 emit 给预览窗口
#[tauri::command]
pub fn update_clipboard_preview(app: AppHandle, id: i64) {
    let Ok(item) = db::with_db(|conn| {
        db::get_clipboard_item_by_id(conn, id)
    }) else { return };
    let Some(item) = item else { return };

    let payload = serde_json::json!({
        "id": item.id,
        "itemType": item.item_type,
        "content": item.content,
        "refData": item.ref_data,
        "createdAt": item.created_at,
    });
    let _ = app.emit_to(WINDOW_LABEL, "clipboard-preview://update", payload);
}
```

> **注意**：`db::get_clipboard_item_by_id` 需要确认是否已存在。如果不存在，用 `db::with_db` 查询 `clipboard_history WHERE id = ?`。

- [ ] **Step 2: main.rs 注册模块 + 命令**

在 `crates/desktop/src/main.rs` 中：

模块声明区域加入：
```rust
mod clipboard_preview_window;
```

`tauri::generate_handler!` 宏中加入：
```rust
clipboard_preview_window::update_clipboard_preview,
```

在 clipboard_window 的 `Focused` 事件处理中加入预览窗口联动。找到 `clipboard_window.rs` 中 `WindowEvent::Focused(false)` 分支（约第 151 行），在已有的失焦处理之前加入：

```rust
tauri::WindowEvent::Focused(focused) => {
    if *focused {
        // 浮窗获焦：显示预览窗口
        clipboard_preview_window::show_preview_window(app_clone);
    } else {
        // 失焦：延迟隐藏预览（防焦点抖动）
        let app_for_preview = app_clone.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            clipboard_preview_window::hide_preview_window(&app_for_preview);
        });
    }
}
```

> **注意**：当前 clipboard_window.rs 的 `on_window_event` 可能没有显式处理 `Focused` 事件（只有 `Focused(false)` 的 dock 收缩逻辑）。需要将其改为显式处理 `Focused(true)` 和 `Focused(false)` 两个分支，在已有逻辑之外加入预览窗口联动。

同时在 `toggle_clipboard_window`（clipboard_window.rs 约第 225 行 `hide_clipboard_window`）中隐藏预览：

```rust
pub fn hide_clipboard_window(app: &AppHandle) {
    clipboard_preview_window::hide_preview_window(app);
    // ... 已有隐藏逻辑
}
```

- [ ] **Step 3: capabilities/default.json 加入预览窗口**

在 `windows` 数组中加入 `"clipboard_preview_window"`：

```json
"windows": [
    "main",
    "result_window",
    "settings_window",
    "clipboard_window",
    "clipboard_preview_window",
    "compact_editor_window",
    "action_bar_window",
    "screenshot_*"
]
```

- [ ] **Step 4: 创建预览窗口（main.rs setup 阶段）**

在 `main.rs` 的 `setup` 闭包中，`create_clipboard_window` 之后加入：

```rust
// 创建预览窗口（初始不可见，浮窗获焦时显示）
if let Err(e) = clipboard_preview_window::create_preview_window(app.handle()) {
    log::error!("Failed to create clipboard preview window: {}", e);
}
```

- [ ] **Step 5: 验证编译**

```bash
cargo build -p octopus-desktop
```

Expected: 编译通过，无 error

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(clipboard-preview): Rust 预览窗口模块 + 定位逻辑 + 生命周期联动"
```

---

## Task 2: 预览窗口前端组件

**Files:**
- Create: `crates/desktop/frontend/src/pages/ClipboardPreview/index.tsx`
- Modify: `crates/desktop/frontend/src/App.tsx`

- [ ] **Step 1: 创建 ClipboardPreview 组件**

写入 `crates/desktop/frontend/src/pages/ClipboardPreview/index.tsx`：

```tsx
import { useState, useEffect } from "react";
import { invoke } from "@/lib/tauri";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useT } from "@/lib/i18n";
import { detectUrl } from "@/types/clipboard";

interface PreviewItem {
  id: number;
  itemType: string;
  content: string;
  refData?: string;
  createdAt: string;
}

export default function ClipboardPreview() {
  const t = useT();
  const [item, setItem] = useState<PreviewItem | null>(null);
  const [thumbSrc, setThumbSrc] = useState<string | null>(null);

  useTauriEvent("clipboard-preview://update", (payload) => {
    setItem(payload as PreviewItem);
  });

  // 图片类型拉缩略图
  useEffect(() => {
    if (item?.itemType === "image") {
      invoke<string>("get_image_thumb", { id: item.id })
        .then(setThumbSrc)
        .catch(() => setThumbSrc(null));
    } else {
      setThumbSrc(null);
    }
  }, [item]);

  // 透明窗口：html/body 不设背景色（由外层 div 的 bg 控制）
  return (
    <div className="flex flex-col h-screen overflow-hidden rounded-xl border border-border bg-background shadow-2xl shadow-black/8">
      {!item ? (
        <div className="flex-1 flex items-center justify-center text-muted-foreground/50 text-xs">
          {t("clipboardPreview.empty")}
        </div>
      ) : (
        <>
          {/* 类型标签 + 时间 */}
          <div className="flex items-center gap-2 px-3 py-1.5 border-b border-border/60 flex-shrink-0">
            <span className="text-[10px] font-medium text-muted-foreground uppercase tracking-wide">
              {item.itemType === "voice" ? "ASR" : item.itemType}
            </span>
            <span className="text-[10px] text-muted-foreground/60">
              {item.createdAt}
            </span>
          </div>
          {/* 内容区 */}
          <div className="flex-1 overflow-y-auto thin-scrollbar min-h-0">
            {item.itemType === "image" ? (
              <div className="flex items-center justify-center p-4 min-h-full">
                {thumbSrc ? (
                  <img
                    src={thumbSrc}
                    alt="preview"
                    className="max-w-full max-h-full rounded-md object-contain"
                  />
                ) : (
                  <span className="text-xs text-muted-foreground">Loading...</span>
                )}
              </div>
            ) : item.itemType === "file" ? (
              <pre className="px-3 py-2 text-xs text-muted-foreground whitespace-pre-wrap break-all font-mono">
                {formatFilePaths(item.refData)}
              </pre>
            ) : (
              <pre className="px-3 py-2 text-xs text-foreground whitespace-pre-wrap break-words font-mono leading-relaxed">
                {item.content}
              </pre>
            )}
          </div>
        </>
      )}
    </div>
  );
}

function formatFilePaths(refData?: string): string {
  if (!refData) return "";
  try {
    const paths: string[] = JSON.parse(refData);
    return paths.map((p) => {
      const stripped = p.replace(/^file:\/\//, "");
      const path = p.startsWith("file://") ? decodeURIComponent(stripped) : stripped;
      return path;
    }).join("\n");
  } catch {
    return refData;
  }
}
```

- [ ] **Step 2: App.tsx 加入路由**

在 `crates/desktop/frontend/src/App.tsx` 中：

import 加入：
```tsx
import ClipboardPreview from "@/pages/ClipboardPreview";
```

switch 加入：
```tsx
case "clipboard_preview_window":
    return <ClipboardPreview />;
```

- [ ] **Step 3: 添加 i18n key**

在 `crates/desktop/frontend/src/locales/zh-CN.yaml` 末尾追加：

```yaml
# ════════ ClipboardPreview 预览面板 ════════
clipboardPreview:
  empty: 选择条目查看完整内容
```

在 `crates/desktop/frontend/src/locales/en.yaml` 末尾追加：

```yaml
# ════════ ClipboardPreview ════════
clipboardPreview:
  empty: Select an item to preview
```

- [ ] **Step 4: tsc 验证**

```bash
cd crates/desktop/frontend && npx tsc --noEmit
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(clipboard-preview): 前端预览组件 + 路由 + i18n"
```

---

## Task 3: 剪贴板浮窗选中联动预览

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/index.tsx`
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`

- [ ] **Step 1: index.tsx — 选中变化时触发预览更新**

在 `crates/desktop/frontend/src/pages/Clipboard/index.tsx` 中，已有的 `selectedIndex` scroll-into-view useEffect 之后，新增：

```tsx
// 选中变化时更新预览窗口
useEffect(() => {
  if (selectedIndex === null) return;
  const item = items[selectedIndex];
  if (item) {
    invoke("update_clipboard_preview", { id: item.id }).catch(() => {});
  }
}, [selectedIndex, items]);
```

确保 `invoke` 已导入（文件顶部应有 `import { invoke, listen } from "@/lib/tauri";`）。

- [ ] **Step 2: ClipboardItem.tsx — hover 时触发预览更新**

在 `ClipboardItem.tsx` 的根 div（`onClick={handleClick}` 的同一个 div）上加入 `onMouseEnter`：

```tsx
onMouseEnter={() => onSelect(index)}
```

这样鼠标 hover 时自动调用 `onSelect(index)` → 更新 `selectedIndex` → 触发预览更新。

> **注意**：`onSelect` 已是 `useCallback` 稳定引用，不影响 memo。`onMouseEnter` 触发 `setSelectedIndex` 会导致两行 re-render（旧选中行 + 新选中行），与现有点击选中行为一致。

- [ ] **Step 3: 构建验证**

```bash
cd crates/desktop/frontend && npx tsc --noEmit
cd .. && npm run build
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(clipboard-preview): 浮窗选中/hover 联动预览窗口"
```

---

## Task 4: 最终验证 + 文档同步

- [ ] **Step 1: 全量构建**

```bash
cargo build -p octopus-desktop --features embedded
cd crates/desktop/frontend && npm run build
```

- [ ] **Step 2: 全量测试**

```bash
cd crates/desktop/frontend && npm test
cargo test -p octopus-desktop
```

- [ ] **Step 3: 检查遗漏**

```bash
# 搜索前端残留硬编码（预览组件中不应有中文字符串）
grep -rn '[\x{4e00}-\x{9fff}]' crates/desktop/frontend/src/pages/ClipboardPreview/ | grep -v '//'
```

- [ ] **Step 4: 更新 architecture.md**

在 architecture.md 的窗口表中加入 `clipboard_preview_window` 行，描述预览窗口的定位逻辑和生命周期。

- [ ] **Step 5: 更新 spec 状态**

将 spec 顶部状态改为「✅ 已实现」。

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "docs: 剪贴板预览面板完成，更新 architecture + spec"
```

---

## Self-Review

### Spec coverage
- [x] 预览窗口创建（无边框圆角透明置顶） → Task 1
- [x] 位置计算（左/右自动选择） → Task 1 `compute_preview_position`
- [x] 生命周期（有焦点常驻、失焦隐藏） → Task 1 `Focused` 事件
- [x] 选中变化实时更新 → Task 3
- [x] hover 触发预览 → Task 3 Step 2
- [x] 键盘 ↑↓ 触发预览 → Task 3 Step 1（selectedIndex 变化）
- [x] 文本/OCR/语音展示 → Task 2
- [x] 图片缩略图 → Task 2 `get_image_thumb`
- [x] 文件路径 → Task 2 `formatFilePaths`
- [x] capabilities 权限 → Task 1 Step 3
- [x] 不改变浮窗布局 → 独立窗口，无修改

### 风险点
- `db::get_clipboard_item_by_id` 可能不存在——需确认或改用 `db::with_db` 内联查询
- clipboard_window.rs 的 `on_window_event` 结构需要适配——当前 `Focused(false)` 分支已有 dock 逻辑，需在其同级加入 `Focused(true)` 分支
- 焦点抖动——延迟 150ms 隐藏可能不够，实际测试需调整


---

## 实施偏差记录（2026-07-12）

| 计划描述 | 实际实施 | 原因 |
|----------|---------|------|
| 独立 Tauri 预览窗口 + Rust 定位逻辑 | 改为 hover overlay（单窗口内 absolute div） | 双窗口方案焦点/层级冲突无法根治（3 轮修复仍失败）；overlay 在现有 300×600 窗口内，无穿透/dock/焦点问题 |
| 预览窗口生命周期（Focused 事件） | 改为 Eye/EyeOff 开关 + hover 触发 | overlay 不需要窗口级生命周期管理 |
| 位置计算（左/右自动选择） | 改为智能上下定位（选中在上半→弹下方，下半→弹上方，与条目重叠 2px） | overlay 在列表内部，只需要上下定位 |
| 预览窗口尺寸 360px | 改为 200×200px | 在 300px 窗口内，200px 宽已足够；高度 1/3 避免遮挡过多 |
| 默认开启 | 改为默认关闭，localStorage 记住选择 | 用户可按需开启，避免默认遮挡列表 |
| capabilities + Rust 模块 | 不需要 | overlay 纯前端，无 Rust 改动 |
| 预览无截断 | 长文本 >500 字截断 + … | 万字级文本一次性渲染 DOM 卡顿 |
| hover 与键盘导航冲突 | 键盘 ↑↓ 时 keyboardNavRef 屏蔽 mouseEnter 300ms | scrollIntoView 滚动误触 onMouseEnter 抢回旧选中位置 |
| en.yaml previewOn/Off 放错段 | 移到 clipboard: 段 | 英文 tooltip 显示 raw key（screenshot 段误放） |
| 缩略图 IPC 竞态 | cancelled 守卫 + cleanup | 快速切图时旧 invoke 结果覆盖新图 |
| previewTop clamp 坐标系 | clamp 上下界加 listEl.scrollTop | abs 子元素 top 是内容坐标，视口常量 clamp 致滚动后预览消失 |
| 浮窗失焦隐藏预览 | onFocusChanged 失焦 setPreviewItem(null) | 预览应随浮窗一起消失 |
| keyboardNavRef setTimeout | timer ref + 卸载清理 | 连按方向键堆叠 timer 未清理 |

