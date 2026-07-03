# 已归档实施计划（2026-06-25 ~ 2026-06-28）

> 以下功能均已实现并合并 main。本文件由 9 份原独立 plan 文件合并归档，原文件已删除。
> plan↔spec 旧路径交叉引用随归档失效，按主题在 specs/2026-06-28-archived-specs.md 内查同名章节。

## 目录

- 2026-06-25-clipboard-history.md
- 2026-06-26-asr-rename-to-local.md
- 2026-06-27-asr-streaming-token-diagnostic.md
- 2026-06-27-global-edit-shortcut.md
- 2026-06-27-image-storage-blob.md
- 2026-06-27-ocr-module.md
- 2026-06-28-polish-global-shortcut.md
- 2026-06-28-screenshot.md
- 2026-06-28-settings-model-selection.md


---

## 来自原文件 `2026-06-25-clipboard-history.md`

# 剪贴板历史管理功能 Implementation Plan

> **状态: ✅ Phase 0-3 已实现**（commit `a30306e` 及之前全部完成）。Phase 0（React 骨架）/ Phase 1（三页面迁移）/ Phase 2（octopus-clipboard crate + paste.rs 迁移）/ Phase 3（剪贴板 UI + Tauri 集成）全部落地。额外实现：SVG 图标恢复、窗口位置记忆、编辑取消按钮、单击选中/双击粘贴交互、app_config category 语义化。
>
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 octopus desktop 新增剪贴板历史管理功能（监听、存储、搜索、UI），同时迁移现有 paste 插件到 clipboard-rs，迁移前端到 React + shadcn/ui。

**Architecture:** 新增 `octopus-clipboard` crate 承载核心能力（clipboard-rs 监听 + SQLite 存储 + 图片去重 + 自动清理）。前端从纯 HTML 迁移到 React + shadcn/ui，新增剪贴板历史窗口。paste.rs 从 tauri-plugin-clipboard-manager 迁移到 clipboard-rs。

**Tech Stack:** Rust（clipboard-rs / image / sha2 / rusqlite）、React 18 + TypeScript + Vite 6 + Tailwind 4 + shadcn/ui + @tanstack/react-virtual

**Spec:** `docs/superpowers/specs/2026-06-25-clipboard-history-design.md`

> **worktree**: `.worktrees/clipboard-research`（分支 `feature/clipboard-research`）。所有文件操作用 worktree 内路径。

> **TDD 说明**：octopus-clipboard crate 的纯逻辑模块（store/image/cleanup/model）用 TDD；涉及系统剪贴板 + GUI + Tauri 的模块（watcher/handle/paste.rs 迁移/前端）无法离线隔离测试，用 `cargo check` + 集成测试 + 手动验证。

---

## File Structure

### 新增文件

| 文件 | 责任 |
|---|---|
| `crates/clipboard/Cargo.toml` | crate 定义 + 依赖 |
| `crates/clipboard/src/lib.rs` | 公开 API + 模块导出 |
| `crates/clipboard/src/model.rs` | ClipboardItem / ItemType / Source / AsrMeta / FileMeta / ImageMeta |
| `crates/clipboard/src/store.rs` | DB CRUD: insert/query/delete/clear + FTS5 搜索 |
| `crates/clipboard/src/handle.rs` | ClipboardHandle: Mutex<ClipboardContext> + SUPPRESS_FLAG |
| `crates/clipboard/src/watcher.rs` | clipboard-rs Watcher 封装 + 后台线程 + callback |
| `crates/clipboard/src/image.rs` | PNG 编码 + SHA-256 去重 + 缩略图 + blob 回收 |
| `crates/clipboard/src/cleanup.rs` | 自动清理策略 |
| `crates/desktop/frontend/` | React 前端项目（全部页面） |
| `crates/desktop/src/clipboard_commands.rs` | Tauri 命令层 |
| `crates/desktop/src/clipboard_window.rs` | 剪贴板历史窗口管理 |

### 修改文件

| 文件 | 变更 |
|---|---|
| `Cargo.toml`（workspace） | 加 `crates/clipboard` member |
| `crates/desktop/Cargo.toml` | 加 `octopus-clipboard` 依赖；移除 `tauri-plugin-clipboard-manager` |
| `crates/desktop/src/main.rs` | 注册 clipboard 命令；移除 clipboard-manager 插件；启动 watcher |
| `crates/desktop/src/paste.rs` | 迁移到 clipboard-rs（替换 `tauri_plugin_clipboard_manager::ClipboardExt`） |
| `crates/desktop/src/coordinator.rs` | `do_paste()` 追加 `insert_asr_item()` 调用 |
| `crates/infra/src/db.sql` | 追加 clipboard_history 表 + FTS5 + 触发器 + app_config seed |
| `crates/infra/src/db.rs` | `init_schema` 加 v4→v5 迁移 |
| `crates/desktop/tauri.conf.json` | `frontendDist` 改为 React 构建产物；加 `beforeBuildCommand` |
| `crates/desktop/capabilities/default.json` | 移除 clipboard-manager 权限；加 clipboard 窗口权限 |

---

## Phase 0：React 前端基础设施

### Task 0.1: 创建 React + Vite + Tailwind 项目骨架

**Files:**
- Create: `crates/desktop/frontend/package.json`
- Create: `crates/desktop/frontend/vite.config.ts`
- Create: `crates/desktop/frontend/tsconfig.json`
- Create: `crates/desktop/frontend/tailwind.config.ts`
- Create: `crates/desktop/frontend/index.html`
- Create: `crates/desktop/frontend/src/main.tsx`
- Create: `crates/desktop/frontend/src/App.tsx`
- Create: `crates/desktop/frontend/src/index.css`

- [x] **Step 1: 创建 Vite + React + TypeScript 项目**

```bash
cd crates/desktop
npm create vite@latest frontend -- --template react-ts
cd frontend
npm install
```

- [x] **Step 2: 安装 Tailwind CSS 4 + Vite 插件**

```bash
cd crates/desktop/frontend
npm install tailwindcss @tailwindcss/vite
```

- [x] **Step 3: 配置 vite.config.ts**

```typescript
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    outDir: "../dist",
    emptyOutDir: true,
  },
});
```

- [x] **Step 4: 配置 src/index.css（Tailwind 入口）**

```css
@import "tailwindcss";

@theme {
  --color-background: hsl(0 0% 100%);
  --color-foreground: hsl(240 10% 3.9%);
  --color-primary: hsl(240 5.9% 10%);
  --color-primary-foreground: hsl(0 0% 98%);
  --color-muted: hsl(240 4.8% 95.9%);
  --color-muted-foreground: hsl(240 3.8% 46.1%);
  --color-accent: hsl(240 4.8% 95.9%);
  --color-accent-foreground: hsl(240 5.9% 10%);
  --color-border: hsl(240 5.9% 90%);
  --color-ring: hsl(240 5.9% 10%);
  --radius: 0.5rem;
}

.dark {
  --color-background: hsl(240 10% 3.9%);
  --color-foreground: hsl(0 0% 98%);
  --color-primary: hsl(0 0% 98%);
  --color-primary-foreground: hsl(240 5.9% 10%);
  --color-muted: hsl(240 3.7% 15.9%);
  --color-muted-foreground: hsl(240 5% 64.9%);
  --color-accent: hsl(240 3.7% 15.9%);
  --color-accent-foreground: hsl(0 0% 98%);
  --color-border: hsl(240 3.7% 15.9%);
  --color-ring: hsl(240 4.9% 83.9%);
}
```

- [x] **Step 5: 配置 tsconfig.json 的 path alias**

确保 `tsconfig.json` 有：
```json
{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@/*": ["./src/*"] }
  }
}
```

- [x] **Step 6: 写一个最小 App.tsx 验证骨架**

```tsx
function App() {
  const label = (window as any).__TAURI__?.window?.getCurrentWindow?.()?.label ?? "unknown";
  return <div className="p-4 text-foreground">Window: {label}</div>;
}
export default App;
```

- [x] **Step 7: 验证构建**

```bash
cd crates/desktop/frontend
npm run build
```
Expected: `../dist/` 下生成 `index.html` + `assets/`。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/frontend/
git commit -m "feat(desktop): React + Vite + Tailwind 前端骨架"
```

---

### Task 0.2: 配置 shadcn/ui

**Files:**
- Create: `crates/desktop/frontend/components.json`
- Create: `crates/desktop/frontend/src/lib/utils.ts`
- Create: `crates/desktop/frontend/src/components/ui/button.tsx`（验证 shadcn 工作流）

- [x] **Step 1: 安装 shadcn/ui CLI 依赖**

```bash
cd crates/desktop/frontend
npm install class-variance-authority clsx tailwind-merge
npm install -D @types/node
```

- [x] **Step 2: 创建 components.json**

```json
{
  "style": "new-york",
  "rsc": false,
  "tsx": true,
  "tailwind": {
    "config": "",
    "css": "src/index.css",
    "baseColor": "zinc",
    "cssVariables": true
  },
  "aliases": {
    "components": "@/components",
    "utils": "@/lib/utils",
    "ui": "@/components/ui"
  }
}
```

- [x] **Step 3: 创建 lib/utils.ts**

```typescript
import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
```

- [x] **Step 4: 用 shadcn CLI 添加 button 组件验证**

```bash
cd crates/desktop/frontend
npx shadcn@latest add button
```
Expected: `src/components/ui/button.tsx` 生成。

- [x] **Step 5: 在 App.tsx 验证 Button 渲染**

```tsx
import { Button } from "@/components/ui/button";

function App() {
  return (
    <div className="p-4">
      <Button variant="outline">shadcn works</Button>
    </div>
  );
}
export default App;
```

- [x] **Step 6: 验证构建**

```bash
npm run build
```
Expected: PASS。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/frontend/
git commit -m "feat(desktop): shadcn/ui 配置 + 基础组件"
```

---

### Task 0.3: Tauri 构建链集成 + IPC 封装

**Files:**
- Modify: `crates/desktop/tauri.conf.json`
- Create: `crates/desktop/frontend/src/lib/tauri.ts`
- Create: `crates/desktop/frontend/src/hooks/useTauriEvent.ts`

- [x] **Step 1: 更新 tauri.conf.json 构建配置**

old:
```json
  "build": {
    "frontendDist": "dist"
  },
```

new:
```json
  "build": {
    "frontendDist": "dist",
    "beforeBuildCommand": "cd frontend && npm run build",
    "beforeDevCommand": "cd frontend && npm run dev",
    "devUrl": "http://localhost:1420"
  },
```

同时更新 `vite.config.ts` 加 dev server 端口：
```typescript
export default defineConfig({
  server: { port: 1420 },
  // ... existing
});
```

- [x] **Step 2: 安装 Tauri 前端 API**

```bash
cd crates/desktop/frontend
npm install @tauri-apps/api @tauri-apps/plugin-global-shortcut
```

- [x] **Step 3: 创建 src/lib/tauri.ts 封装**

```typescript
import { invoke as rawInvoke } from "@tauri-apps/api/core";
import { listen as rawListen, type UnlistenFn } from "@tauri-apps/api/event";

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return rawInvoke<T>(cmd, args);
}

export async function listen(event: string, handler: (payload: unknown) => void): Promise<UnlistenFn> {
  return rawListen(event, (e) => handler(e.payload));
}
```

- [x] **Step 4: 创建 src/hooks/useTauriEvent.ts**

```typescript
import { useEffect } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export function useTauriEvent(event: string, handler: (payload: unknown) => void) {
  useEffect(() => {
    let unlisten: UnlistenFn;
    let cancelled = false;
    listen(event, (e) => handler(e.payload)).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [event, handler]);
}
```

- [x] **Step 5: 验证 `cargo build` 能正确触发前端构建**

```bash
cargo build -p octopus-desktop 2>&1 | head -20
```
Expected: 构建过程中执行 `npm run build`，`dist/` 下有 React 产物。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/frontend/ crates/desktop/tauri.conf.json
git commit -m "feat(desktop): Tauri 构建链集成 React + IPC 封装"
```

---

## Phase 1：现有页面迁移到 React

> **说明**：Phase 1 是机械翻译——功能不变，仅从 HTML+JS 改为 React 组件。每个页面一个 Task。所有 `#[tauri::command]` 不变，前端调用同样的 invoke。

### Task 1.1: 迁移 Overlay 页面

**Files:**
- Create: `crates/desktop/frontend/src/pages/Overlay.tsx`
- Reference: `crates/desktop/dist/overlay/index.html`（旧实现，迁移后删除）

- [x] **Step 1: 读旧 overlay/index.html，理解全部逻辑**

Run: `cat crates/desktop/dist/overlay/index.html`

关键逻辑：监听 `show-overlay` / `partial-result` / `hide-overlay` 事件，显示录音/识别状态指示器。

- [x] **Step 2: 实现 Overlay.tsx**

将 124 行 HTML + 内联 JS 翻译为 React 组件：
- `useTauriEvent('show-overlay', ...)` 替代 `listen('show-overlay', ...)`
- `useTauriEvent('partial-result', ...)` 替代 `listen('partial-result', ...)`
- `useTauriEvent('hide-overlay', ...)` 替代 `listen('hide-overlay', ...)`
- CSS 用 Tailwind 类替代 `<style>` 块（保持视觉一致）
- 动画用 Tailwind `animate-pulse` + 自定义 keyframe

- [x] **Step 3: 更新 App.tsx 路由**

```tsx
import Overlay from "@/pages/Overlay";

function App() {
  const label = (window as any).__TAURI__?.window?.getCurrentWindow?.()?.label ?? "";
  switch (label) {
    case "overlay": return <Overlay />;
    default: return <div>Unknown window: {label}</div>;
  }
}
```

- [x] **Step 4: 验证构建**

```bash
cd crates/desktop/frontend && npm run build
```
Expected: PASS。

- [x] **Step 5: 手动验证（需 GUI）**

启动应用，触发录音，确认 overlay 状态指示器正常显示/隐藏。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/frontend/src/
git commit -m "feat(desktop): 迁移 overlay 页面到 React"
```

---

### Task 1.2: 迁移 Result 页面

**Files:**
- Create: `crates/desktop/frontend/src/pages/Result.tsx`
- Reference: `crates/desktop/dist/result/index.html`（651 行，迁移后删除）

- [x] **Step 1: 读旧 result/index.html，梳理全部功能模块**

关键功能：
- 结果文本展示（committed + incremental）
- 工具栏 hover 展开（mousemove/mouseleave + setSize）
- 浮层菜单（popup open/close）
- 编辑模式（Cmd+Enter toggle）
- 润色模式切换（3 选项）
- 快捷键解析（parseShortcut / matchShortcut）
- Tauri 事件监听（show-result / partial-result / polish-done / edit-state 等）
- invoke 调用（enter_edit_mode / update_edit_buffer / commit_edit / polish_now / open_settings 等）

- [x] **Step 2: 拆分为子组件**

```
Result/
├── index.tsx          ← 主组件（状态管理 + 事件监听）
├── Toolbar.tsx        ← 工具栏（hover 展开 + 浮层菜单）
├── EditArea.tsx       ← 编辑/展示区域
└── PolishToggle.tsx   ← 润色模式切换
```

- [x] **Step 3: 实现 Result/index.tsx**

翻译核心逻辑：
- `useState` 替代 DOM 状态变量
- `useTauriEvent` 替代 `listen`
- `invoke()` 替代 `window.__TAURI__.core.invoke`
- JSX 替代 `innerHTML` 拼接
- shadcn `Popover` / `Button` / `Tooltip` 替代手工浮层

- [x] **Step 4: 实现子组件**

按拆分实现 Toolbar / EditArea / PolishToggle。

- [x] **Step 5: 更新 App.tsx 路由**

```tsx
case "result": return <Result />;
```

- [x] **Step 6: 验证构建 + 手动测试**

验证：结果展示、编辑模式、润色切换、工具栏 hover、快捷键。

- [x] **Step 7: Commit**

```bash
git add crates/desktop/frontend/src/
git commit -m "feat(desktop): 迁移 result 页面到 React"
```

---

### Task 1.3: 迁移 Settings 页面

**Files:**
- Create: `crates/desktop/frontend/src/pages/Settings/index.tsx`
- Create: `crates/desktop/frontend/src/pages/Settings/GeneralSettings.tsx`
- Create: `crates/desktop/frontend/src/pages/Settings/HistoryPanel.tsx`
- Create: `crates/desktop/frontend/src/pages/Settings/PromptsPanel.tsx`
- Create: `crates/desktop/frontend/src/pages/Settings/ModelsPanel.tsx`
- Reference: `crates/desktop/dist/settings/index.html`（1002 行）+ `models.js`（217 行）

- [x] **Step 1: 读旧 settings/index.html + models.js，梳理全部功能**

关键功能：
- 侧边栏导航（通用/历史/模型/prompts）
- 配置表单（引擎/快捷键/润色/音频等，21 个 invoke 调用）
- 识别历史列表（分页加载 + 多选 + 删除 + 复制）
- 模型管理（可下载模型列表 + 下载/验证）
- Prompts 管理（CRUD + 激活）

- [x] **Step 2: 实现 Settings/index.tsx（侧边栏 + 路由）**

用 shadcn `Tabs` 或自定义 nav 实现侧边栏切换。

- [x] **Step 3: 实现 GeneralSettings.tsx**

用 `react-hook-form` + `zod` 管理配置表单，替代手工 DOM 读写。每个配置项用 shadcn `Switch` / `Input` / `Select`。

- [x] **Step 4: 实现 HistoryPanel.tsx**

翻译历史列表逻辑：分页加载 + 多选 + 删除 + 复制。`innerHTML` 拼接 → JSX map。

- [x] **Step 5: 实现 ModelsPanel.tsx**

翻译 `models.js` 逻辑：模型列表 + 下载进度 + 验证。

- [x] **Step 6: 实现 PromptsPanel.tsx**

CRUD 界面：prompt 列表 + 编辑器 + 激活。

- [x] **Step 7: 更新 App.tsx 路由**

```tsx
case "settings": return <Settings />;
```

- [x] **Step 8: 验证构建 + 手动测试**

验证：配置读写、历史列表、模型管理、prompts CRUD。

- [x] **Step 9: Commit**

```bash
git add crates/desktop/frontend/src/
git commit -m "feat(desktop): 迁移 settings 页面到 React"
```

---

### Task 1.4: 清理旧 HTML + 更新窗口配置

**Files:**
- Delete: `crates/desktop/dist/overlay/`（旧 HTML，已被 React 替代）
- Delete: `crates/desktop/dist/result/`
- Delete: `crates/desktop/dist/settings/`
- Note: `dist/` 现在由 Vite 构建产物输出（`index.html` + `assets/`）

- [x] **Step 1: 确认所有窗口路由正常**

手动测试：overlay 窗口、result 窗口、settings 窗口全部正常加载 React 版。

- [x] **Step 2: 更新窗口创建代码（如有硬编码 dist 路径）**

检查 `result_window.rs` / `settings_window.rs` / overlay 窗口创建代码，确保 `url` 参数指向新的 `index.html`（而非 `dist/result/index.html` 等子路径）。

Tauri v2 单 HTML 模式：所有窗口加载 `index.html`，React 根据 `window.label` 渲染。

- [x] **Step 3: 删除旧 dist 子目录**

```bash
rm -rf crates/desktop/dist/overlay/
rm -rf crates/desktop/dist/result/
rm -rf crates/desktop/dist/settings/
```

- [x] **Step 4: 验证完整构建**

```bash
cargo build -p octopus-desktop
```
Expected: PASS，`dist/` 下只有 Vite 产物。

- [x] **Step 5: Commit**

```bash
git add -A crates/desktop/dist/
git commit -m "refactor(desktop): 清理旧 HTML，统一为 React 单 HTML 入口"
```

---

## Phase 2：octopus-clipboard crate

### Task 2.1: 创建 crate 骨架 + workspace 注册

**Files:**
- Create: `crates/clipboard/Cargo.toml`
- Create: `crates/clipboard/src/lib.rs`
- Modify: `Cargo.toml`（workspace root）

- [x] **Step 1: 创建 Cargo.toml**

```toml
[package]
name = "octopus-clipboard"
version = "0.1.0"
edition = "2021"

[dependencies]
octopus-infra = { path = "../infra" }
clipboard-rs = { version = "0.3", features = ["image", "wayland"] }
image = { version = "0.25", features = ["png", "jpeg"] }
sha2 = "0.10"
rusqlite = { version = "0.31", features = ["bundled"] }
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
log = "0.4"
```

- [x] **Step 2: 创建 src/lib.rs（最小骨架）**

```rust
pub mod model;
pub mod store;

pub use model::{ClipboardItem, ItemType, Source};
```

- [x] **Step 3: 注册到 workspace**

old (`Cargo.toml` line 2):
```
members = ["crates/infra", "crates/asr", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download"]
```

new:
```
members = ["crates/infra", "crates/asr", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download", "crates/clipboard"]
```

- [x] **Step 4: 验证编译**

```bash
cargo check -p octopus-clipboard
```
Expected: PASS（model/store 模块还没实现，先建空文件让 lib.rs 编译）。

创建 `src/model.rs` 和 `src/store.rs` 为空文件（`pub fn _placeholder() {}`），后续 Task 填充。

- [x] **Step 5: Commit**

```bash
git add crates/clipboard/ Cargo.toml
git commit -m "feat(clipboard): 创建 octopus-clipboard crate 骨架"
```

---

### Task 2.2: 数据结构（model.rs）

**Files:**
- Create: `crates/clipboard/src/model.rs`

- [x] **Step 1: 定义全部数据结构**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    Text,
    Image,
    File,
}

impl ItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemType::Text => "text",
            ItemType::Image => "image",
            ItemType::File => "file",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "image" => ItemType::Image,
            "file" => ItemType::File,
            _ => ItemType::Text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Clipboard,
    Asr,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Clipboard => "clipboard",
            Source::Asr => "asr",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "asr" => Source::Asr,
            _ => Source::Clipboard,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMeta {
    pub blob_hash: String,
    pub width: u32,
    pub height: u32,
    pub has_thumbnail: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub file_count: usize,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrMeta {
    pub transcription_id: i64,
    pub polish_status: String,
    pub engine: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub id: i64,
    pub item_type: ItemType,
    pub source: Source,
    pub content: String,
    pub is_favorite: bool,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_meta: Option<ImageMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_meta: Option<FileMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asr_meta: Option<AsrMeta>,
    pub is_rich: bool,
}

/// 查询过滤条件
#[derive(Debug, Clone, Default)]
pub struct QueryFilter {
    /// "all" | "asr" | "text" | "image" | "file" | "favorite"
    pub filter: String,
    pub search: Option<String>,
    pub page: u32,
    pub size: u32,
}

impl Default for ItemType {
    fn default() -> Self {
        ItemType::Text
    }
}

impl Default for Source {
    fn default() -> Self {
        Source::Clipboard
    }
}
```

- [x] **Step 2: 写单元测试**

在 model.rs 底部加 `#[cfg(test)] mod tests`，测试 `ItemType::from_str` / `as_str` 往返、`Source` 同理。

- [x] **Step 3: 运行测试**

```bash
cargo test -p octopus-clipboard --lib model
```
Expected: PASS。

- [x] **Step 4: Commit**

```bash
git add crates/clipboard/src/model.rs
git commit -m "feat(clipboard): 数据结构定义（ItemType/Source/ClipboardItem/QueryFilter）"
```

---

### Task 2.3: DB schema 迁移（db.sql + db.rs v5）

**Files:**
- Modify: `crates/infra/src/db.sql`
- Modify: `crates/infra/src/db.rs`

- [x] **Step 1: 追加 clipboard_history 建表到 db.sql**

在 db.sql 末尾追加（全文见 spec §2.3）：
- `clipboard_history` 表（全字段）
- 5 个索引（idx_clip_created / type / source / hash / favorite）
- FTS5 虚表 `clipboard_history_fts`
- 3 个触发器（clip_fts_ai / ad / au）
- app_config seed（5 个 clipboard_ 配置项）

- [x] **Step 2: db.rs 加 v4→v5 迁移分支**

old（db.rs `init_schema` 函数末尾，`v == 3` 分支之后）:
```rust
    } else if v == 3 {
        // v3 → v4：prompts 表 + app_config.active_polish_prompt seed（INIT_SQL 幂等补建）
        log::info!("DB migrating v3 → v4: adding prompts table + active_polish_prompt seed...");
        conn.execute_batch(INIT_SQL).context("v3→v4: 重跑 db.sql 幂等补建 prompts 表 + seed")?;
        conn.execute("PRAGMA user_version = 4", [])?;
        log::info!("DB migrated to v4: prompts table + active_polish_prompt seed added");
    }
    Ok(())
```

new:
```rust
    } else if v == 3 {
        // v3 → v4：prompts 表 + app_config.active_polish_prompt seed（INIT_SQL 幂等补建）
        log::info!("DB migrating v3 → v4: adding prompts table + active_polish_prompt seed...");
        conn.execute_batch(INIT_SQL).context("v3→v4: 重跑 db.sql 幂等补建 prompts 表 + seed")?;
        conn.execute("PRAGMA user_version = 4", [])?;
        log::info!("DB migrated to v4: prompts table + active_polish_prompt seed added");
    } else if v == 4 {
        // v4 → v5：clipboard_history 表 + FTS5 + 触发器 + app_config seed
        log::info!("DB migrating v4 → v5: adding clipboard_history table...");
        conn.execute_batch(INIT_SQL).context("v4→v5: 建 clipboard_history 表 + FTS5")?;
        conn.execute("PRAGMA user_version = 5", [])?;
        log::info!("DB migrated to v5: clipboard_history + FTS5");
    }
    Ok(())
```

同时更新 `v < 2` 分支的 version 跳转（v0/v1 → v5）和注释：
old: `conn.execute("PRAGMA user_version = 4", [])?;`
new: `conn.execute("PRAGMA user_version = 5", [])?;`

同理 `v == 2` 分支末尾。

- [x] **Step 3: 验证 FTS5 可用**

```bash
cargo test -p octopus-infra --lib
```
Expected: PASS。如果 FTS5 不可用，需检查 rusqlite bundled feature。

- [x] **Step 4: 手动验证迁移（删除旧 DB 触发重建）**

```bash
rm ~/.octopus/octopus.db
cargo run -p octopus-cli -- config
```
Expected: 日志显示 "DB initialized (v5)" 或类似，DB 文件重新创建含 clipboard_history 表。

验证表存在：
```bash
sqlite3 ~/.octopus/octopus.db ".tables"
```
Expected: 含 `clipboard_history` 和 `clipboard_history_fts`。

- [x] **Step 5: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): clipboard_history 表 + FTS5 + v5 迁移"
```

---

### Task 2.4: Store CRUD + FTS5 搜索（store.rs）

**Files:**
- Create: `crates/clipboard/src/store.rs`

- [x] **Step 1: 写 insert_clipboard_item 的失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use octopus_infra::db;

    fn setup() {
        // 用内存 DB 或临时文件 DB
        // 注意：infra 的 ensure_db 用全局单例，测试需用独立连接或临时 DB 路径
        // 方案：store 函数接受 &Connection 参数，测试传入临时连接
    }

    #[test]
    fn test_insert_and_query_text() {
        // insert 一条 text → query → 验证字段
    }
}
```

**设计决策**：store.rs 的函数接受 `&Connection` 参数（不从全局单例取），方便测试。调用方（Tauri 命令层）负责从 infra 取全局连接传入。

- [x] **Step 2: 实现 insert / query / delete / clear**

核心函数签名：

```rust
use rusqlite::{params, Connection};
use crate::model::*;

pub fn insert_clipboard_item(conn: &Connection, item: &NewClipboardItem) -> anyhow::Result<i64>;
pub fn insert_asr_item(conn: &Connection, text: &str, asr_meta: AsrMeta) -> anyhow::Result<i64>;
pub fn query_history(conn: &Connection, filter: &QueryFilter) -> anyhow::Result<Vec<ClipboardItem>>;
pub fn toggle_favorite(conn: &Connection, id: i64) -> anyhow::Result<()>;
pub fn delete_item(conn: &Connection, id: i64) -> anyhow::Result<()>;
pub fn clear_history(conn: &Connection, keep_favorite: bool) -> anyhow::Result<usize>;
pub fn find_by_content_hash(conn: &Connection, blob_hash: &str) -> anyhow::Result<Option<i64>>;
```

- [x] **Step 3: 实现 FTS5 搜索**

`query_history` 中当 `filter.search` 非空时，走 FTS5 join：

```sql
SELECT ch.* FROM clipboard_history ch
JOIN clipboard_history_fts fts ON ch.id = fts.rowid
WHERE fts.search_text MATCH ?
ORDER BY rank
LIMIT ? OFFSET ?
```

否则走普通分页：
```sql
SELECT * FROM clipboard_history
WHERE [filter conditions]
ORDER BY created_at DESC
LIMIT ? OFFSET ?
```

- [x] **Step 4: 实现过滤条件构建**

根据 `filter.filter` 值构建 WHERE：
- `all` → 无额外条件
- `asr` → `source = 'asr'`
- `text` → `item_type = 'text' AND source = 'clipboard'`
- `image` → `item_type = 'image'`
- `file` → `item_type = 'file'`
- `favorite` → `is_favorite = 1`

- [x] **Step 5: 写全部 CRUD 测试**

测试用例：
- `test_insert_and_query_text` — 插入文本 → 查询验证
- `test_insert_and_query_image` — 插入图片（content=hash）→ 查询验证
- `test_fts_search_chinese` — 插入中文文本 → FTS5 搜索验证
- `test_filter_by_type` — 插入多种类型 → 按 filter 过滤
- `test_filter_by_source` — 插入 clipboard + asr → 按 source 过滤
- `test_toggle_favorite` — 收藏 toggle
- `test_delete_item` — 删除单条
- `test_clear_history_keep_favorite` — 清空但保留收藏
- `test_dedup_by_hash` — 相同 blob_hash 不重复插入

- [x] **Step 6: 运行测试**

```bash
cargo test -p octopus-clipboard --lib store
```
Expected: 全部 PASS。

- [x] **Step 7: Commit**

```bash
git add crates/clipboard/src/store.rs
git commit -m "feat(clipboard): Store CRUD + FTS5 搜索 + 过滤"
```

---

### Task 2.5: 图片处理（image.rs）

**Files:**
- Create: `crates/clipboard/src/image.rs`

- [x] **Step 1: 实现 PNG 编码 + SHA-256 去重**

```rust
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use image::ImageReader;

/// 图片存储根目录
pub fn clipboard_images_dir() -> PathBuf {
    octopus_infra::paths::octopus_config_home().join("clipboard_images")
}

/// 编码 RGBA 像素为 PNG bytes，计算 SHA-256
pub fn encode_and_hash(rgba: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, String)> {
    // 用 image crate 编码 PNG
    // 计算 SHA-256
}

/// 保存原图 + 生成缩略图
pub fn save_image(png_bytes: &[u8], hash: &str) -> Result<ImageSaveResult> {
    let dir = clipboard_images_dir();
    std::fs::create_dir_all(&dir)?;
    let orig_path = dir.join(format!("{}.png", hash));
    let thumb_path = dir.join(format!("{}_thumb.png", hash));
    std::fs::write(&orig_path, png_bytes)?;
    // 生成 240×240 缩略图
    generate_thumbnail(&orig_path, &thumb_path, 240)?;
    Ok(ImageSaveResult { orig_path, thumb_path })
}

/// 删除无引用的孤立 blob
pub fn cleanup_orphaned_blobs(referenced_hashes: &std::collections::HashSet<String>) -> Result<usize> {
    // 遍历 clipboard_images/ 目录，删除不在 referenced_hashes 中的 .png 文件
}
```

- [x] **Step 2: 实现缩略图生成**

用 `image` crate 的 `resize`：
```rust
fn generate_thumbnail(orig: &Path, thumb: &Path, max_size: u32) -> Result<()> {
    let img = image::open(orig)?;
    let thumbnail = img.resize(max_size, max_size, image::imageops::FilterType::Lanczos3);
    thumbnail.save(thumb)?;
    Ok(())
}
```

- [x] **Step 3: 写测试**

测试用例：
- `test_encode_and_hash` — 已知像素 → 验证 hash
- `test_save_and_thumbnail` — 保存图片 → 缩略图存在
- `test_dedup_same_image` — 相同像素 → 相同 hash
- `test_cleanup_orphaned` — 创建 3 个文件，2 个有引用 → 删除 1 个孤立文件

- [x] **Step 4: 运行测试**

```bash
cargo test -p octopus-clipboard --lib image
```
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add crates/clipboard/src/image.rs
git commit -m "feat(clipboard): 图片 PNG 编码 + SHA-256 去重 + 缩略图"
```

---

### Task 2.6: ClipboardHandle（读写 + suppress flag）

**Files:**
- Create: `crates/clipboard/src/handle.rs`
- Modify: `crates/clipboard/src/lib.rs`（导出 handle 模块）

- [x] **Step 1: 实现 ClipboardHandle**

```rust
use anyhow::Result;
use clipboard_rs::{ClipboardContext, Clipboard};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub struct ClipboardHandle {
    ctx: Mutex<ClipboardContext>,
    suppress_flag: AtomicBool,
}

impl ClipboardHandle {
    pub fn new() -> Result<Self> {
        let ctx = ClipboardContext::new().map_err(|e| anyhow::anyhow!("Clipboard init failed: {}", e))?;
        Ok(Self {
            ctx: Mutex::new(ctx),
            suppress_flag: AtomicBool::new(false),
        })
    }

    /// 检查并消费 suppress flag（watcher 调用）
    pub fn check_and_clear_suppress(&self) -> bool {
        self.suppress_flag.swap(false, Ordering::SeqCst)
    }

    /// 写文本（设置 suppress flag）
    pub fn write_text(&self, text: &str) -> Result<()> {
        self.suppress_flag.store(true, Ordering::SeqCst);
        let ctx = self.ctx.lock().unwrap();
        ctx.set_text(text.to_string()).map_err(|e| anyhow::anyhow!("Clipboard write failed: {}", e))?;
        Ok(())
    }

    /// 读文本
    pub fn read_text(&self) -> Result<String> {
        let ctx = self.ctx.lock().unwrap();
        ctx.get_text().map_err(|e| anyhow::anyhow!("Clipboard read failed: {}", e))
    }

    /// 读图片
    pub fn read_image(&self) -> Result<clipboard_rs::common::RustImageData> {
        let ctx = self.ctx.lock().unwrap();
        ctx.get_image().map_err(|e| anyhow::anyhow!("Clipboard read image failed: {}", e))
    }

    /// 读文件列表
    pub fn read_files(&self) -> Result<Vec<String>> {
        let ctx = self.ctx.lock().unwrap();
        ctx.get_files().map_err(|e| anyhow::anyhow!("Clipboard read files failed: {}", e))
    }

    /// 判断当前有哪些格式
    pub fn has(&self, format: clipboard_rs::common::ContentFormat) -> bool {
        let ctx = self.ctx.lock().unwrap();
        ctx.has(format)
    }
}
```

- [x] **Step 2: 更新 lib.rs 导出**

```rust
pub mod model;
pub mod store;
pub mod handle;
pub mod image;

pub use model::{ClipboardItem, ItemType, Source, QueryFilter};
pub use handle::ClipboardHandle;
```

- [x] **Step 3: 编译验证**

```bash
cargo check -p octopus-clipboard
```
Expected: PASS。

- [x] **Step 4: Commit**

```bash
git add crates/clipboard/src/handle.rs crates/clipboard/src/lib.rs
git commit -m "feat(clipboard): ClipboardHandle 读写 + suppress flag"
```

---

### Task 2.7: Watcher 封装

**Files:**
- Create: `crates/clipboard/src/watcher.rs`
- Modify: `crates/clipboard/src/lib.rs`

- [x] **Step 1: 实现 Watcher**

```rust
use anyhow::Result;
use clipboard_rs::{ClipboardWatcher, ClipboardWatcherContext, ClipboardHandler};
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;

pub struct ClipboardWatcher {
    shutdown: Option<clipboard_rs::WatcherShutdown>,
}

impl ClipboardWatcher {
    pub fn start<F>(handle: Arc<crate::ClipboardHandle>, on_change: F) -> Result<Self>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut watcher = ClipboardWatcherContext::new()
            .map_err(|e| anyhow::anyhow!("Watcher init failed: {}", e))?;

        let handler = ChangeHandler {
            handle,
            on_change: Arc::new(on_change),
        };

        let shutdown = watcher.add_handler(handler);

        std::thread::spawn(move || {
            watcher.start_watch();
        });

        Ok(Self { shutdown: Some(shutdown) })
    }

    pub fn stop(&mut self) {
        if let Some(s) = self.shutdown.take() {
            drop(s); // 发送停止信号
        }
    }
}

impl Drop for ClipboardWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ChangeHandler<F: Fn() + Send + Sync> {
    handle: Arc<crate::ClipboardHandle>,
    on_change: Arc<F>,
}

impl<F: Fn() + Send + Sync + 'static> ClipboardHandler for ChangeHandler<F> {
    fn on_clipboard_change(&mut self) {
        // 检查 suppress flag
        if self.handle.check_and_clear_suppress() {
            return; // 我们自己写的，跳过
        }
        (self.on_change)();
    }
}
```

- [x] **Step 2: 在 lib.rs 导出 + 加 start_watcher 便捷函数**

```rust
pub mod watcher;
pub use watcher::ClipboardWatcher;

/// 处理剪贴板变化：读内容 → 去重 → 存 DB
/// 此函数在 watcher 回调线程中调用，由调用方传入 DB 连接
pub fn handle_clipboard_change(handle: &ClipboardHandle) -> anyhow::Result<()> {
    // 1. 判断类型（files > image > text）
    // 2. 读内容
    // 3. 去重（hash 或文本比对）
    // 4. 存 DB（store::insert_clipboard_item）
    // 5. 通知前端（由调用方的 on_change 回调处理）
    todo!() // 实现见 Step 3
}
```

- [x] **Step 3: 实现 handle_clipboard_change**

按 spec §2.2 优先级逻辑：
- `handle.has(ContentFormat::Files)` → read_files → JSON stringify → 去重 → insert
- `handle.has(ContentFormat::Image)` → read_image → encode_and_hash → 去重 → save + insert
- else → read_text → hash 去重 → insert

- [x] **Step 4: 编译验证**

```bash
cargo check -p octopus-clipboard
```

- [x] **Step 5: Commit**

```bash
git add crates/clipboard/src/watcher.rs crates/clipboard/src/lib.rs
git commit -m "feat(clipboard): Watcher 封装 + 变化处理逻辑"
```

---

### Task 2.8: 自动清理（cleanup.rs）

**Files:**
- Create: `crates/clipboard/src/cleanup.rs`
- Modify: `crates/clipboard/src/lib.rs`

- [x] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_cleanup_by_age() {
        // 插入 3 条：1 条 31 天前、1 条 29 天前、1 条收藏(31 天前)
        // 执行 cleanup(max_age_days=30)
        // 验证：31 天前非收藏被删，29 天前保留，收藏保留
    }

    #[test]
    fn test_cleanup_by_count() {
        // 插入 5 条非收藏 + 1 条收藏
        // 执行 cleanup(max_items=3)
        // 验证：非收藏按时间 ASC 删到 3 条，收藏保留
    }

    #[test]
    fn test_cleanup_blob_reclaim() {
        // 插入图片条目 + 创建图片文件
        // 删除条目 → cleanup → 孤立图片文件被删除
    }
}
```

- [x] **Step 2: 实现 cleanup**

```rust
use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashSet;

pub fn run_cleanup(
    conn: &Connection,
    max_age_days: u32,
    max_items: u32,
) -> Result<CleanupResult> {
    let mut deleted = 0;

    // 1. 按天数删除
    deleted += delete_by_age(conn, max_age_days)?;

    // 2. 按数量删除
    deleted += delete_by_count(conn, max_items)?;

    // 3. 孤立 blob 回收
    let reclaimed = crate::image::cleanup_orphaned_blobs(&get_referenced_hashes(conn)?)?;

    // 4. FTS5 重建
    conn.execute("INSERT INTO clipboard_history_fts(clipboard_history_fts) VALUES('rebuild')", [])?;

    Ok(CleanupResult { deleted_items: deleted, reclaimed_blobs: reclaimed })
}

fn delete_by_age(conn: &Connection, max_age_days: u32) -> Result<usize> { ... }
fn delete_by_count(conn: &Connection, max_items: u32) -> Result<usize> { ... }
fn get_referenced_hashes(conn: &Connection) -> Result<HashSet<String>> { ... }

pub struct CleanupResult {
    pub deleted_items: usize,
    pub reclaimed_blobs: usize,
}
```

- [x] **Step 3: 运行测试**

```bash
cargo test -p octopus-clipboard --lib cleanup
```
Expected: PASS。

- [x] **Step 4: Commit**

```bash
git add crates/clipboard/src/cleanup.rs crates/clipboard/src/lib.rs
git commit -m "feat(clipboard): 自动清理（天数+数量+收藏豁免+blob 回收）"
```

---

### Task 2.9: paste.rs 迁移到 clipboard-rs

**Files:**
- Modify: `crates/desktop/Cargo.toml`（加 octopus-clipboard 依赖）
- Modify: `crates/desktop/src/paste.rs`
- Modify: `crates/desktop/src/main.rs`（移除 clipboard-manager 插件）
- Modify: `crates/desktop/capabilities/default.json`（移除 clipboard-manager 权限）

- [x] **Step 1: desktop Cargo.toml 加 octopus-clipboard**

在 `[dependencies]` 中加：
```toml
octopus-clipboard = { path = "../clipboard" }
```

移除：
```toml
tauri-plugin-clipboard-manager = "2"
```

- [x] **Step 2: paste.rs 替换 ClipboardExt**

old:
```rust
use tauri_plugin_clipboard_manager::ClipboardExt;
```

new（移除 tauri Runtime 泛型，改用全局 ClipboardHandle）:
```rust
use octopus_clipboard::ClipboardHandle;
use std::sync::Arc;
```

`write_to_clipboard` 改为接收 `&ClipboardHandle`：
```rust
fn write_to_clipboard(text: &str, handle: &ClipboardHandle) -> Result<()> {
    handle.write_text(text)?;
    Ok(())
}
```

`paste_via_clipboard` / `paste_direct` 同理——把所有 `app_handle.clipboard()` 调用替换为 `handle.write_text()` / `handle.read_text()`。

paste 函数签名改为接收 `Arc<ClipboardHandle>` 而非 `AppHandle`：
```rust
pub fn paste(text: &str, handle: &ClipboardHandle, config: &AppConfig) -> Result<()> { ... }
```

- [x] **Step 3: main.rs 移除 clipboard-manager 插件 + 初始化 ClipboardHandle**

old:
```rust
        .plugin(tauri_plugin_clipboard_manager::init())
```

删除这行。

在 setup 中初始化全局 ClipboardHandle 并 manage：
```rust
let clipboard_handle = Arc::new(octopus_clipboard::ClipboardHandle::new()?);
app.manage(clipboard_handle.clone());
```

- [x] **Step 4: coordinator.rs 更新 paste 调用**

old（do_paste 函数内）:
```rust
paste::paste(&text_to_paste, &handle_for_closure, &config)
```

new:
```rust
paste::paste(&text_to_paste, &clipboard_handle, &config)
```

需要把 `clipboard_handle: Arc<ClipboardHandle>` 传入 do_paste 的调用链。由于 coordinator 已有 `app_handle`，可以用 `app.state::<Arc<ClipboardHandle>>()` 获取。

- [x] **Step 5: capabilities/default.json 移除权限**

old:
```json
    "clipboard-manager:allow-read-text",
    "clipboard-manager:allow-write-text",
```

删除这两行。

- [x] **Step 6: 编译验证**

```bash
cargo check -p octopus-desktop
```
Expected: PASS，零 error。

- [x] **Step 7: 手动回归测试**

录音 → 识别 → 确认 paste 行为不变（写剪贴板 + Cmd+V + 恢复原剪贴板）。

- [x] **Step 8: Commit**

```bash
git add crates/desktop/ crates/clipboard/
git commit -m "refactor(desktop): paste.rs 从 tauri-plugin-clipboard-manager 迁移到 clipboard-rs"
```

---

## Phase 3：剪贴板历史 UI + Tauri 集成

### Task 3.1: Tauri 命令层

**Files:**
- Create: `crates/desktop/src/clipboard_commands.rs`
- Modify: `crates/desktop/src/main.rs`（注册命令）

- [x] **Step 1: 实现 clipboard_commands.rs**

```rust
use octopus_clipboard::{ClipboardItem, QueryFilter};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn query_clipboard_history(
    filter: String,
    search: Option<String>,
    page: u32,
    size: u32,
) -> Result<Vec<ClipboardItem>, String> {
    let conn = octopus_infra::db::with_db(|c| { /* ... */ }).map_err(|e| e.to_string())?;
    // 调 store::query_history
    todo!()
}

#[tauri::command]
pub async fn toggle_clipboard_favorite(id: i64) -> Result<(), String> { todo!() }

#[tauri::command]
pub async fn delete_clipboard_item(id: i64) -> Result<(), String> { todo!() }

#[tauri::command]
pub async fn clear_clipboard_history(keep_favorite: bool) -> Result<(), String> { todo!() }

#[tauri::command]
pub async fn copy_clipboard_item(
    id: i64,
    handle: State<'_, Arc<octopus_clipboard::ClipboardHandle>>,
) -> Result<(), String> {
    // 从 DB 读 content → handle.write_text() → 返回
    todo!()
}
```

- [x] **Step 2: main.rs 注册命令**

在 `invoke_handler` 中追加：
```rust
clipboard_commands::query_clipboard_history,
clipboard_commands::toggle_clipboard_favorite,
clipboard_commands::delete_clipboard_item,
clipboard_commands::clear_clipboard_history,
clipboard_commands::copy_clipboard_item,
```

- [x] **Step 3: main.rs setup 中启动 watcher**

```rust
// 启动剪贴板监听
let watcher_handle = clipboard_handle.clone();
let app_handle_for_watcher = app.handle().clone();
let mut watcher = octopus_clipboard::ClipboardWatcher::start(watcher_handle, move || {
    // 剪贴板变化 → 处理（读内容+存DB） → 通知前端
    let _ = octopus_clipboard::handle_clipboard_change_globally();
    let _ = app_handle_for_watcher.emit("clipboard://changed", ());
})?;
app.manage(watcher);
```

- [x] **Step 4: 编译验证**

```bash
cargo check -p octopus-desktop
```

- [x] **Step 5: Commit**

```bash
git add crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): 剪贴板历史 Tauri 命令层 + watcher 启动"
```

---

### Task 3.2: 剪贴板历史窗口管理

**Files:**
- Create: `crates/desktop/src/clipboard_window.rs`
- Modify: `crates/desktop/src/main.rs`

- [x] **Step 1: 实现 clipboard_window.rs**

```rust
use tauri::{AppHandle, Manager, WebviewWindowBuilder, WebviewUrl};

pub fn create_clipboard_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("clipboard") {
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(
        app,
        "clipboard",
        WebviewUrl::default(), // 加载 index.html
    )
    .title("剪贴板历史")
    .inner_size(420.0, 600.0)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(true)
    .visible(false) // 先隐藏，ready 后再 show
    .build()?;
    Ok(())
}

pub fn toggle_clipboard_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        // toggle 方向按"焦点"而非"可见性"判断：always-on-top 窗口失焦后仍 visible，
        // 若用 is_visible() 决定方向，失焦时按一次会被先藏掉、第二次才弹出。
        // 仅当"可见且有焦点"才收起；失焦（或不可见）一律 show + set_focus 激活。
        let visible = window.is_visible().unwrap_or(false);
        let focused = window.is_focused().unwrap_or(false);
        if visible && focused {
            window.hide()?;
        } else {
            window.show()?;
            window.set_focus()?;
        }
    } else {
        create_clipboard_window(app)?;
        if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
            window.show()?;
            window.set_focus()?;
        }
    }
    Ok(())
}
```

- [x] **Step 2: main.rs 注册全局快捷键 + 失焦隐藏**

在 setup 中注册 `Alt+V` 快捷键：
```rust
// clipboard 快捷键
let app_handle_for_clipboard = app.handle().clone();
app.global_shortcut().on_shortcut("Alt+V", move |_, _| {
    let _ = clipboard_window::toggle_clipboard_window(&app_handle_for_clipboard);
})?;
```

失焦隐藏（在 clipboard 窗口创建后绑定）：
```rust
window.on_window_event(move |event| {
    if let tauri::WindowEvent::Focused(false) = event {
        // 隐藏（除非 pinned —— pinned 状态由前端管理，通过 invoke 查询）
        let _ = window.hide();
    }
});
```

- [x] **Step 3: App.tsx 加 clipboard 路由**

```tsx
case "clipboard": return <Clipboard />;
```

- [x] **Step 4: 创建最小 Clipboard 页面验证窗口能弹出**

```tsx
// src/pages/Clipboard/index.tsx（最小版）
function Clipboard() {
  return <div className="p-4">剪贴板历史（开发中）</div>;
}
```

- [x] **Step 5: 手动验证**

启动应用 → 按 Alt+V → 窗口弹出 → 失焦隐藏。

- [x] **Step 6: Commit**

```bash
git add crates/desktop/src/clipboard_window.rs crates/desktop/src/main.rs crates/desktop/frontend/src/
git commit -m "feat(desktop): 剪贴板历史窗口 + Alt+V 快捷键"
```

---

### Task 3.3: FilterTabs 组件

**Files:**
- Create: `crates/desktop/frontend/src/pages/Clipboard/FilterTabs.tsx`
- Create: `crates/desktop/frontend/src/pages/Clipboard/index.tsx`

- [x] **Step 1: 安装 shadcn tabs 组件**

```bash
cd crates/desktop/frontend
npx shadcn@latest add tabs badge
```

- [x] **Step 2: 实现 FilterTabs**

6 个 tab（全部/ASR/文本/图片/文件/收藏），单选互斥，Lucide 图标。见 spec §4.4。

- [x] **Step 3: 实现 Clipboard/index.tsx 主框架**

```tsx
function Clipboard() {
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  return (
    <div className="flex flex-col h-screen">
      <SearchBar value={search} onChange={setSearch} />
      <FilterTabs value={filter} onChange={setFilter} />
      <HistoryList filter={filter} search={search} />
      <Footer />
    </div>
  );
}
```

- [x] **Step 4: 验证构建**

```bash
npm run build
```

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Clipboard/
git commit -m "feat(desktop): 剪贴板 FilterTabs 组件 + 主框架"
```

---

### Task 3.4: HistoryList + 虚拟滚动

**Files:**
- Create: `crates/desktop/frontend/src/pages/Clipboard/HistoryList.tsx`
- Create: `crates/desktop/frontend/src/hooks/useClipboardHistory.ts`

- [x] **Step 1: 安装虚拟滚动依赖**

```bash
npm install @tanstack/react-virtual
```

- [x] **Step 2: 实现 useClipboardHistory hook**

```typescript
export function useClipboardHistory(filter: string, search: string) {
  const [items, setItems] = useState<ClipboardItem[]>([]);
  const [loading, setLoading] = useState(false);

  const debouncedSearch = useDebouncedValue(search, 300);

  useEffect(() => {
    setLoading(true);
    invoke<ClipboardItem[]>('query_clipboard_history', {
      filter, search: debouncedSearch || null, page: 1, size: 50
    }).then(setItems).finally(() => setLoading(false));
  }, [filter, debouncedSearch]);

  // 监听变化事件 → 重新查询
  useTauriEvent('clipboard://changed', () => {
    invoke<ClipboardItem[]>('query_clipboard_history', {
      filter, search: debouncedSearch || null, page: 1, size: 50
    }).then(setItems);
  });

  return { items, loading };
}
```

- [x] **Step 3: 实现 HistoryList 虚拟滚动**

用 `@tanstack/react-virtual` 的 `useVirtualizer`，见 spec §4.5。

- [x] **Step 4: 验证构建 + 手动测试**

复制几段文本 → 窗口弹出 → 列表显示。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/
git commit -m "feat(desktop): 剪贴板 HistoryList 虚拟滚动 + useClipboardHistory hook"
```

---

### Task 3.5: ClipboardItem 按类型渲染

**Files:**
- Create: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`
- Create: `crates/desktop/frontend/src/types/clipboard.ts`

- [x] **Step 1: 定义 TypeScript 类型**

```typescript
// src/types/clipboard.ts
export type ItemType = 'text' | 'image' | 'file';
export type Source = 'clipboard' | 'asr';

export interface ClipboardItem {
  id: number;
  item_type: ItemType;
  source: Source;
  content: string;
  is_favorite: boolean;
  created_at: string;
  image_meta?: { blob_hash: string; width: number; height: number; has_thumbnail: boolean };
  file_meta?: { file_count: number; paths: string[] };
  asr_meta?: { transcription_id: number; polish_status: string; engine: string; model: string };
  is_rich: boolean;
}
```

- [x] **Step 2: 实现 ClipboardItem 组件**

按 spec §4.6 渲染：text/image/file/asr 四种，Lucide 图标，收藏按钮，右键菜单（shadcn ContextMenu）。

安装依赖：
```bash
npx shadcn@latest add context-menu
```

- [x] **Step 3: 实现操作行为**

- 单击：`invoke('copy_clipboard_item', { id })` → 关闭窗口
- 双击：`invoke('copy_clipboard_item', { id })` → 模拟粘贴 → 关闭窗口
- 收藏：`invoke('toggle_clipboard_favorite', { id })` → 乐观更新
- 右键删除：`invoke('delete_clipboard_item', { id })`

- [x] **Step 4: 验证构建 + 手动测试**

复制文本/截图 → 列表正确渲染 → 点击复制 → 收藏切换。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/
git commit -m "feat(desktop): ClipboardItem 按类型渲染 + 操作行为"
```

---

### Task 3.6: 搜索 + 底栏

**Files:**
- Create: `crates/desktop/frontend/src/pages/Clipboard/SearchBar.tsx`
- Create: `crates/desktop/frontend/src/pages/Clipboard/Footer.tsx`
- Create: `crates/desktop/frontend/src/hooks/useDebouncedValue.ts`

- [x] **Step 1: 实现 useDebouncedValue**

```typescript
import { useEffect, useState } from "react";
export function useDebouncedValue<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(timer);
  }, [value, delay]);
  return debounced;
}
```

- [x] **Step 2: 实现 SearchBar**

shadcn `Input` + Lucide `Search` 图标。

- [x] **Step 3: 实现 Footer**

显示总条数 + "清空历史" 按钮（shadcn `Button` + 确认弹窗 shadcn `AlertDialog`）。

安装依赖：
```bash
npx shadcn@latest add input alert-dialog
```

- [x] **Step 4: 验证构建 + 手动测试**

搜索中文文本 → FTS5 正确返回 → 清空历史 → 列表空。

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/
git commit -m "feat(desktop): 剪贴板搜索 + 底栏 + 清空历史"
```

---

### Task 3.7: ASR 写入历史集成

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`

- [x] **Step 1: do_paste 中追加 insert_asr_item 调用**

在 `do_paste` 函数内，`paste::paste()` 调用前插入：
```rust
// 写入剪贴板历史（source=asr），失败不阻断
if let Err(e) = octopus_infra::db::with_db(|conn| {
    octopus_clipboard::store::insert_asr_item(
        conn,
        &text_to_paste,
        octopus_clipboard::model::AsrMeta {
            transcription_id: id,
            polish_status: polish_status.to_string(),
            engine: config.asr_engine.clone(),
            model: String::new(),
        },
    )
}) {
    log::warn!("Clipboard history ASR insert failed: {}", e);
}
```

- [x] **Step 2: 编译验证**

```bash
cargo check -p octopus-desktop
```

- [x] **Step 3: 手动端到端测试**

录音 → 识别 → 粘贴 → 打开剪贴板历史 → 确认 ASR 条目出现（🎤 图标 + source=asr）。
同时确认不会出现重复的 clipboard 来源条目（suppress flag 生效）。

- [x] **Step 4: Commit**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "feat(desktop): ASR 识别完成写入剪贴板历史"
```

---

### Task 3.8: 配置项（设置页 clipboard 面板）

**Files:**
- Create: `crates/desktop/frontend/src/pages/Settings/ClipboardSettings.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/index.tsx`（加导航项）

- [x] **Step 1: 实现 ClipboardSettings 面板**

配置项（已在 app_config seed）：
- 启用/禁用监听（`clipboard_enabled`）→ Toggle（设置页「交互」Card + 浮窗 title bar 快捷按钮）。**注**：v1 此项仅 seed，实为死配置；v4 起真正落地——纳入 `AppConfig` + watcher `recording_enabled` 运行时 gate，热重载生效
- 快捷键（`clipboard_shortcut`）→ Input + 录制
- 最大条数（`clipboard_max_items`）→ Select（500/1000/2000/5000）
- 清理天数（`clipboard_max_age_days`）→ Select（7/30/90）
- ~~点击行为（`clipboard_auto_paste`）~~ → **v4 已移除**：双击列表项固定粘贴（`paste_clipboard_item`），不再可配

- [x] **Step 2: 更新 Settings 侧边栏**

加"剪贴板"导航项 + 对应面板。

- [x] **Step 3: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/
git commit -m "feat(desktop): 剪贴板设置面板"
```

---

## Phase 4/5：二期与可选增强

> Phase 4（file 类型渲染 + 富文本）和 Phase 5（NSPanel / Paste Stack / 内容变换）在首期完成后再规划。DB schema 和数据结构已在 Phase 2 预留扩展位，二期不需要改表结构。

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1.1 octopus-clipboard crate | Task 2.1 |
| §1.2 clipboard-rs 选型 | Task 2.1（Cargo.toml）+ Task 2.6 |
| §1.3 替换 tauri-plugin-clipboard-manager | Task 2.9 |
| §1.4 React + shadcn/ui 前端 | Task 0.1-0.3 + Phase 1 |
| §2.1 类型判定优先级 | Task 2.7（handle_clipboard_change） |
| §2.2 各类型数据流 | Task 2.4（store）+ Task 2.5（image）+ Task 2.7 |
| §2.3 DB schema | Task 2.3 |
| §2.4 Rust 数据结构 | Task 2.2 |
| §2.5 文件系统布局 | Task 2.5 |
| §3 监听架构 + suppress flag | Task 2.6 + Task 2.7 + Task 3.1 |
| §3.3 ASR 写入路径 | Task 3.7 |
| §4 UI 架构 | Task 0.1-0.3 + Phase 1 + Phase 3 |
| §4.4 FilterTabs | Task 3.3 |
| §4.5 虚拟滚动 | Task 3.4 |
| §4.6 按类型渲染 | Task 3.5 |
| §4.7 操作行为 | Task 3.5 |
| §4.8 窗口属性 | Task 3.2 |
| §4.9 Tauri 命令层 | Task 3.1 |
| §5.1 自动清理 | Task 2.8 |
| §5.2 错误处理 | Task 2.7 + Task 2.9（重试逻辑） |
| §5.3 边界 case | Task 2.6（suppress）+ Task 2.7（去重/空内容） |
| §5.4 并发安全 | Task 2.6（Mutex + AtomicBool） |
| §5.5 DB 迁移 | Task 2.3 |
| §6 实施分期 | Phase 0-3 对应 |
| §7 依赖变更 | Task 2.1 + Task 2.9 + Task 0.1 |
| §8 风险缓解 | Task 2.3（FTS5 验证）+ Task 2.9（重试） |

---

## Phase 3 后迭代记录（2026-06-27）

Phase 3 完成后的持续迭代，按实际实施顺序记录。

### 迭代 1：设置窗口标题简化

- `settings_window.rs:34` — `.title("Octopus 设置")` → `.title("Octopus")`

### 迭代 2：图片保存格式选择（三轮演进）

**第一轮**：系统对话框 + filter 选格式（WebP/PNG），quality 硬编码 90

**第二轮**：自定义浮层（SaveImagePopover）+ 格式按钮 + 质量滑块（默认 90）+ 系统保存对话框

**第三轮（最终）**：去掉系统对话框，直接落盘到 `~/Downloads/octopus/`
- 新增 `infra::image_util::save_as_jpeg()`（PNG→RGB→JPEG，quality 参数）
- `save_image_item` 命令改为接受 `format` + `quality` + `open_folder` 参数
- 默认 JPEG quality=85
- 文件名冲突自动加序号（`unique_path`）
- 勾选「打开文件夹」时 `reveal_in_file_manager`（macOS `open -R` / Windows `explorer /select,` / Linux `xdg-open`）
- Cargo.toml 新增 `dirs = "5"` 依赖
- 前端 SaveImagePopover 按 frontend-design skill 设计（纯白卡片+强阴影、分段控件、细线滑轨）

### 迭代 3：FTS5 索引自动维护

**问题**：FTS5 external content table 的 DELETE 触发器只移除逻辑索引，`_data` 表 b-tree 页不收缩，删除越多空洞越大（实测 8 条数据 _data 表 183 行、DB 2.1M）。

**实现**：
- `store.rs` 新增 `rebuild_fts_index()` + `track_deletes()`（`AtomicU32`，阈值 10）
- `delete_item` 每次累加 1，`clear_history` 累加删除行数，达 10 rebuild + 清零
- `main.rs` setup 阶段调用 `rebuild_fts_index`，清理上次运行遗留
- 手动 `INSERT INTO clipboard_history_fts(...) VALUES('rebuild')` + `VACUUM` 清理已膨胀的 DB（2.1M → 168K）

**注意**：`run_cleanup`（按天数/数量清理 + blob 回收）已实现但**尚未接入定时调用**。

### 迭代 4：Toolbar 精简

- Result 窗口 toolbar 去掉「语音模型」和「润色模型」入口（模型太多，下拉空间有限）
- 模型切换移至 Settings 页面
- 清理 `openAsrPopup`、`openLlmPopup` 函数和 `handlePopupSelect` 中的 asr/llm 分支
- 保留 5 个图标：关闭 / 系统设置 / 降噪模式 / 润色模式 / 立即润色

### 迭代 5：弹窗适配

- 降噪/润色弹窗 3 个选项被 `overflow-hidden` 裁剪（窗口高 100px）
- 弹窗字号 `13px→12px`、行间距 `py-1.5→py-1`、起始位 `30px→28px`、`z-10→z-30`
- `result_window.rs` 窗口高度 `100→116px`

### 迭代 6：剪贴板管理入口 + 管理页重设计

**剪贴板浮窗**：
- 底部「清空」按钮改为「管理」按钮（齿轮图标），点击 `open_settings({ initialPage: "clipboard" })`
- 移除 `handleClear` 函数和 `invoke` import

**open_settings 导航**：
- `settings_window.rs` 新增 `initial_page: Option<String>` 参数
- `PENDING_PAGE`（`Mutex<Option<String>>`）暂存目标页面
- 新增 `get_initial_page` 命令，前端 mount 时拉取并清除
- 窗口已打开时走 `settings://navigate` 事件即时切换
- `tray.rs` 调用处补 `None` 参数

**ClipboardPanel 重设计**（frontend-design skill）：
- 布局对调：搜索/过滤置顶 → 列表 → 底部状态 + 批量操作浮现
- stone 暖灰色系，过滤标签选中 stone-800 深墨
- 提取 `ClipboardRow` 子组件：行操作与浮窗一致（收藏/保存图片/打开文件/单条二次确认删除；复制改为左侧类型图标单击——放大回弹 + 闪绿触效 + 「已复制」气泡 1.5s，2026-07-01 调整）
- 复用 `SaveImagePopover` 组件

**HistoryPanel 同风格重设计**：
- 搜索置顶，列表 header 全选
- 提取 `HistoryRow` 子组件：复制 + 单条二次确认删除 + 原始文本折叠展开
- 已润色条目左侧 amber-600 竖线（签名元素）

### 迭代 7：分页与删除竞态（三轮演进）

**第一轮：无限滚动**
- 新增 `useInfiniteScroll` hook（`IntersectionObserver`，rootMargin 100px 预加载）
- 两页面去掉「加载更多」按钮，sentinel 自动触发加载
- 底部「— 没有更多了 —」提示

**第二轮：竞态修复（失败）**
- 问题：无限滚动正在加载（`loading=true`）时，删除完成调 `loadHistory(true)` 被 `if (loading) return` 挡住，列表不刷新
- 尝试 `pendingResetRef`（useRef）标记「待重置」，加载完成后补执行
- **闭包陷阱**：`useCallback` 依赖 `loading`，递归补执行捕获的是旧闭包（`loading` 仍为 `true`），永远跳不出

**第三轮（最终）：回退为手动加载更多**
- 删除 `useInfiniteScroll` hook + sentinel
- 删除 `pendingResetRef` 和 loading 守卫
- 恢复手动「加载更多」按钮——手动点击不会与删除并发
- `loadHistory`/`fetchData` 无需守卫，删除后直接刷新
- 保留「— 没有更多了 —」提示

### 迭代 8：级联删除验证与 transcription_id 修复

**问题**：用户删除识别记录后，剪贴板中的 ASR 条目未被联动删除。

**排查**：DB 中 4 条 ASR 记录的 `transcription_id` 全是 NULL。代码正确（`coordinator.rs` 传 `transcript.id` → `insert_asr_item` → SQL INSERT），但旧二进制产生的数据无关联。

**验证**：`test_insert_asr` 测试加断言 `transcription_id == 12345`——INSERT + query 正确写入读回，通过。

**修复**：
- 清理 DB 中 `transcription_id IS NULL` 的旧 ASR 记录
- 重新运行后新 ASR 记录正确关联，级联删除生效
- 反向不级联：`delete_clipboard_item` 只删 `clipboard_history` 行，`transcriptions` 不受影响（`ON DELETE SET NULL`）

### 迭代 9：OCR 模块（详见 `2026-06-27-ocr-module.md`）

- 新增 `octopus-ocr` crate（ocr-rs/MNN + PP-OCRv6）
- `clipboard_commands.rs` 新增 `ocr_image` 命令 + `get_image_thumb` 命令
- 前端 ClipboardItem + ClipboardRow 新增 OCR 按钮（ScanText 三态：idle → spin → ✓）
- OCR 结果写入 `search_text` + 系统剪贴板 + osascript 新建 TextEdit 文档

### 迭代 10：图片存储迁移 文件系统 → DB BLOB（详见 `2026-06-27-image-storage-blob.md`）

- 新增 `image_data` 表（DB v7）
- `image.rs` 重写：WebP 无损编码 + 20% 缩略图
- `store.rs` 新增 image_data CRUD + 引用计数删除
- `watcher.rs` 图片编码改为 WebP → DB BLOB
- `image_migration.rs` 启动时迁移旧文件到 DB
- 前端图片条目内联缩略图展示（base64 WebP）

### 迭代 11：设置页配置暴露

- AppConfig 新增 `clipboard_shortcut` / `clipboard_max_items` / `clipboard_max_age_days`
- **关键 bug 修复**：`save_app_config_at` / `load_app_config_at` 字段列表漏了新字段（22→25），导致内存生效但 DB 不持久化
- `set_config` 新增 `clipboard_shortcut` 热重载（unregister 旧 + register 新）
- `apply_config_value` 新增三个字段的校验
- 快捷键 section 新增「剪贴板浮窗」行
- 剪贴板 section（新 Card）：保留条数 + 清理天数
- ShortcutButton 组件 kbd 标签风格（⌘/⌥/⇧ 符号）

### 迭代 12：快捷键捕获修复

- 快捷键捕获过滤纯修饰键（Alt/Shift/Control/Meta），避免 `Alt+AltLeft` 错误
- 曾尝试 suspend/restore 方案（unregister_all → 太暴力；suspend_shortcut → 时序问题），最终回退到与 `asr_shortcut` 完全一致的简单流程（check_shortcut → setVal → set_config 热重载）

### 迭代 13：设置页 UI 优化

- 「引擎接入」section 移除
- 润色 section label 加「润色」前缀（润色模型/润色模式/润色提示词/润色间隔/润色停顿阈值）
- 润色模型 select 用 `llm_models.find(m => m.current)?.name` 匹配当前选中（修 3-part spec 与裸名不匹配）

### 迭代 14：启动同步修复 + 浮窗监听按钮样式（2026-06-29）

**启动同步修复**（v5 审计 Issue 1.1，核实属实）：
- 问题：`ClipboardHandle::new()` 默认 `recording_enabled = true`，而热重载只在运行时 `set_config` 路径触发——用户关掉「剪贴板监听」并重启后 flag 复活（DB 仍 `false`），watcher 又开始记录，设置形同虚设
- 修复：`main.rs` setup 创建 handle 后、watcher 启动（同一 `Arc`，`main.rs:303`）前，按 `config.clipboard_enabled` 调一次 `set_recording_enabled`，让运行时 flag 与 DB 持久值在启动即一致

**浮窗监听按钮样式**：
- 原：低调灰 `Circle`（启用）/ amber `CircleOff`（禁用）——反馈"太低调"
- 改：`CircleCheck`（绿圆+勾=监听中）/ `CircleX`（红圆+叉=已关闭），强对比状态符号；import 由 `Circle, CircleOff` 换为 `CircleCheck, CircleX`（lucide-react ^1.21.0）


---

## 来自原文件 `2026-06-26-asr-rename-to-local.md`

# octopus-asr → octopus-asr-local 重命名 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `octopus-asr` crate 重命名为 `octopus-asr-local`，与 `octopus-asr-cloud` 命名对称。

**Architecture:** 纯机械重命名（package + lib + 目录 + 依赖 + `use` + docs），零行为/接口变更。关键风险是朴素替换会把 `octopus-asr-cloud` 误改成 `octopus-asr-local-cloud`，故全部用 **perl 负向 lookahead**（`(?!-)` / `(?!_)`）排除 cloud。macOS BSD sed 不支持 `\b`，故用 perl。

**Tech Stack:** Rust workspace、Cargo、perl。

**验证策略（TDD 不适用）：** 本次无新逻辑，不写新测试。靠**现有 workspace 测试全绿**证明零行为变更 + **grep 复核无残留/无误伤**证明替换完整。

**前置：** 所有命令在 worktree 根 `/Users/wudarui/workspace/agent/octopus/.claude/worktrees/model-mgmt-ui` 执行。起点 HEAD `6b47da5`（领先 main 1 commit = spec）。

---

## Task 1: 代码侧重命名（Cargo + 源码 + 验证）

**Files:**
- Rename: `crates/asr` → `crates/asr-local`（`git mv`，保 history）
- Modify: `crates/asr-local/Cargo.toml`（package name）
- Modify: `Cargo.toml`（workspace members）
- Modify: `crates/asr-cloud/Cargo.toml`、`crates/cli/Cargo.toml`、`crates/desktop/Cargo.toml`、`crates/server/Cargo.toml`、`crates/llm/Cargo.toml`（依赖名 + path + desktop feature）
- Modify: ~17 个 `.rs` 源文件（`octopus_asr` → `octopus_asr_local`）
- Auto: `Cargo.lock`（cargo check 自动更新，不手改）

- [x] **Step 1: git mv 目录（保 rename history）**

```bash
git mv crates/asr crates/asr-local
```
Expected: 无输出（成功）。`ls crates/asr-local/Cargo.toml` 存在。

- [x] **Step 2: 改 asr-local 的 package name**

```bash
perl -pi -e 's/^name = "octopus-asr"$/name = "octopus-asr-local"/' crates/asr-local/Cargo.toml
head -3 crates/asr-local/Cargo.toml
```
Expected: `[package]` / `name = "octopus-asr-local"` / `version = "0.1.0"`。

- [x] **Step 3: 改 workspace members**

```bash
perl -pi -e 's{"crates/asr"}{"crates/asr-local"}g' Cargo.toml
grep -n 'crates/asr' Cargo.toml
```
Expected: members 行显示 `"crates/asr-local"`（与 `"crates/asr-cloud"` 并列）；无裸 `"crates/asr"`。

- [x] **Step 4: 改 5 个依赖 Cargo.toml（依赖名 + path + desktop feature）**

依赖名 `octopus-asr`→`octopus-asr-local`（`(?!-)` 排除 `octopus-asr-cloud`）；path `"../asr"`→`"../asr-local"`（带引号精确匹配，排除 `"../asr-cloud"`）。desktop 的 `embedded = ["octopus-asr"]` 同步被改。

```bash
perl -pi -e 's/octopus-asr(?!-)/octopus-asr-local/g; s{"../asr"}{"../asr-local"}g' \
  crates/asr-cloud/Cargo.toml crates/cli/Cargo.toml crates/desktop/Cargo.toml \
  crates/server/Cargo.toml crates/llm/Cargo.toml
echo "--- 复核：裸 octopus-asr 应消失（只剩 -local / -cloud）---"
grep -rn 'octopus-asr' crates/*/Cargo.toml | grep -vE 'octopus-asr-local|octopus-asr-cloud' || echo "✓ 无残留"
```
Expected: 末行 `✓ 无残留`。`grep 'octopus-asr' crates/desktop/Cargo.toml` 应见 `octopus-asr-local = { path = "../asr-local", optional = true }` + `embedded = ["octopus-asr-local"]`。

- [x] **Step 5: 改源码 use/path（octopus_asr → octopus_asr_local）**

`(?!_)` 排除 `octopus_asr_cloud`。覆盖所有 `.rs`（含 asr-local 自身 lib.rs doc、asr-cloud 引用本地零件的 `use octopus_asr::`）。

```bash
find crates -name '*.rs' -print0 | xargs -0 perl -pi -e 's/octopus_asr(?!_)/octopus_asr_local/g'
echo "--- 复核：裸 octopus_asr 应消失（只剩 _local / _cloud）---"
grep -rn 'octopus_asr' crates/ --include='*.rs' | grep -vE 'octopus_asr_local|octopus_asr_cloud' || echo "✓ 无残留"
```
Expected: 末行 `✓ 无残留`。

- [x] **Step 6: cargo check（验证编译 + 自动更新 Cargo.lock）**

```bash
cargo check --workspace --all-targets 2>&1 | tail -5
```
Expected: `Finished` 无 error。Cargo.lock 自动含 `octopus-asr-local`（`grep 'name = "octopus-asr-local"' Cargo.lock` 命中）。若报 `unresolved import octopus_asr` → 有遗漏，回 Step 5 grep 找漏文件。

- [x] **Step 7: cargo test --workspace（零行为变更验证）**

```bash
cargo test --workspace 2>&1 | tail -15
```
Expected: 全绿（lib + 各 crate 单测全 passed，0 failed）。测试数应与改名前一致（无新增/丢失）。

- [x] **Step 8: cargo clippy（0 新 warning）**

```bash
cargo clippy --workspace --all-targets 2>&1 | grep -E 'warning|error' | head || echo "✓ 零 warning/error"
```
Expected: 仅 pre-existing warning（如 desktop `dead_code` current_partial/is_cloud），**无新** warning。

- [x] **Step 9: grep 复核代码侧无残留 + 无 cloud 误伤**

```bash
echo "--- 残留（应空）---"
grep -rnE 'octopus[-_]asr' crates/ --include='*.rs' --include='Cargo.toml' | grep -vE 'octopus[-_]asr-local|octopus[-_]asr-cloud' || echo "✓ 无残留"
echo "--- 误伤 octopus-asr-local-cloud / octopus_asr_local_cloud（应空）---"
grep -rnE 'octopus[-_]asr[-_]local[-_]cloud' . --exclude-dir=target --exclude-dir=.git || echo "✓ 无误伤"
echo "--- workspace members + Cargo.lock 确认 ---"
grep 'asr-local\|asr-cloud\|"crates/asr"' Cargo.toml | head
grep -c 'octopus-asr-local' Cargo.lock
```
Expected: 三处均 `✓`；members 含 `crates/asr-local`；Cargo.lock 命中 ≥1。

- [x] **Step 10: Commit**

```bash
git add -A
git commit -m "refactor: octopus-asr→octopus-asr-local 重命名（代码侧）

与 octopus-asr-cloud 命名对称。package+lib(Octopus_asr_local)+目录 crates/asr-local。
5 依赖 Cargo.toml + ~17 源文件 use + workspace members + Cargo.lock。
零行为变更，workspace 测试全绿，clippy 0 新 warning。
perl 负向 lookahead 排除 -cloud，防误伤。"
```

---

## Task 2: docs 重命名（33 文件含 archived）+ 复核

**Files:**
- Modify: 所有 `.md`（`docs/superpowers/specs/*`、`docs/superpowers/plans/*` 含 `*-archived-*`、`docs/architecture.md`、`docs/asr_archiveture_opt.md`、`AGENTS.md`、`usage.md`、`crates/dlp/docs/architecture.md`）

- [x] **Step 1: docs 全量替换（连字符 + 下划线，排除 cloud）**

对仓库内所有 `.md`（排除构建产物 / 其他 worktree），一条 perl 跑两个表达式：

```bash
find . -name '*.md' \
  -not -path './target/*' -not -path './node_modules/*' \
  -not -path './.git/*' -not -path './.worktrees/*' \
  -not -path './.claude/worktrees/*' -not -path './crates/*/node_modules/*' \
  -print0 | xargs -0 perl -pi -e 's/octopus-asr(?!-)/octopus-asr-local/g; s/octopus_asr(?!_)/octopus_asr_local/g'
echo "--- 替换后 docs 里 octopus-asr-local 命中文件数 ---"
grep -rl 'octopus-asr-local' . --include='*.md' -z 2>/dev/null | grep -vE 'target|node_modules|\.git|\.worktrees|\.claude/worktrees' | wc -l | tr -d ' '
```
Expected: 命中文件数 > 0（与原 33 文件量级吻合）。

- [x] **Step 2: grep 复核 docs 无残留 + 无误伤**

```bash
echo "--- docs 残留裸 octopus-asr/octopus_asr（应空，只剩 -local/-cloud）---"
find . -name '*.md' \
  -not -path './target/*' -not -path './node_modules/*' -not -path './.git/*' \
  -not -path './.worktrees/*' -not -path './.claude/worktrees/*' \
  -print0 | xargs -0 grep -n 'octopus[-_]asr' 2>/dev/null \
  | grep -vE 'octopus[-_]asr-local|octopus[-_]asr-cloud' || echo "✓ 无残留"
echo "--- 误伤 octopus-asr-local-cloud（应空）---"
find . -name '*.md' -not -path './target/*' -not -path './.git/*' -print0 \
  | xargs -0 grep -n 'octopus[-_]asr[-_]local[-_]cloud' 2>/dev/null || echo "✓ 无误伤"
```
Expected: 两处均 `✓`。

- [x] **Step 3: Commit**

```bash
git add -A
git commit -m "docs: octopus-asr→octopus-asr-local 重命名同步（含 archived）

33 个 docs 文件（specs/plans 含 archived + architecture + AGENTS + usage + dlp/docs）
统一 octopus-asr→octopus-asr-local。与代码侧重命名（前一 commit）对齐。"
```

---

## 完成判据

- `cargo check --workspace --all-targets`：0 error
- `cargo test --workspace`：全绿，测试数与改名前一致
- `cargo clippy --workspace --all-targets`：0 新 warning
- 仓库内（排除 `target`/`node_modules`/`.git`/`.worktrees`）grep `octopus[-_]asr` 仅剩 `*-local`/`*-cloud`，无裸 `octopus-asr`/`octopus_asr`，无 `*-local-cloud` 误伤
- 两个 commit（代码侧 + docs）


---

## 来自原文件 `2026-06-27-asr-streaming-token-diagnostic.md`

# 流式 ASR 首/尾字诊断与修复实施计划

> 配套 spec：[`2026-06-27-asr-streaming-token-diagnostic-design.md`](../specs/2026-06-27-asr-streaming-token-diagnostic-design.md)
> 状态：**全部完成**（Phase 1–4 + 收尾）。本文档为事后归档，记录实际执行路径与决策。

## 总览

流式 ASR（`StreamingPipeline`）首字缺失 / 启动 spurious「嗯」/ 停顿后丢字 / 尾字中段重复，按「**丢字 > 叠字**」优先级修复。4 个 Phase，10 个 commit。

```
Phase 1  zipformer 首字（确凿，先行）              041e678
Phase 2  诊断日志（[asr-diag]，驱动后续）           c73af9c 54a0636
Phase 3  日志驱动精准修（paraformer 5 项）          1105798 73e350d 8f32f2f e802e98 3930dbf 968b7e5 a9b55ab
Phase 4  文档 + 诊断日志清理                       本 plan + 日志清理 commit
```

## Phase 1 — zipformer 首字（确凿）

**症结**：`accept_samples` Zipformer 分支 `if was_silent { finish+reset }`，`was_silent` 取更新前 `silence_duration`；开口前静音 > 0.5s + 开口瞬间 `has_speech=false` → 每 tick 反复 reset → 清空 `token_ids` 冲首字。

**步骤**（commit `041e678`）：
1. `step_silence` / `detect_silence_gap` 额外返回 `has_speech` → `(was_silent_for_punct, should_flush, has_speech)`。
2. `StreamingEngine::accept_samples` trait 加 `has_speech` 参数。
3. `streaming_engine.rs` ZipformerCtc / Transducer 分支条件改 `if was_silent && !has_speech`；Paraformer 分支忽略。
4. mock 同步签名：`streaming_runner::FakeStreamingEngine`、server `pipeline.rs::FakeEngine`。
5. 单测：`streaming_runner` 加 `has_speech` 路径用例（过渡 tick 不 reset、持续静音仍分段）。

**改动是机械签名传播**（约 7 处），不触碰底层 `ZipformerStreamOps` trait。

## Phase 2 — 诊断日志（驱动后续修复）

**步骤**（commit `c73af9c` + `54a0636`）：
- `log::debug!`（热路径）/ `log::info!`（reset / force-fire 一次性），统一 `[asr-diag]` 前缀。
- **paraformer**：`process_chunk_at` mask 决策、CIF `fired`/`alpha_cache`、force-fire、跨边界去重命中、fresh_segment 消费；`run_cif` / `run_cif_final`。
- **zipformer**：reset 前段文本快照、CTC / Transducer token emit。
- **文本层哨兵**（`73e350d`，commit 于 Phase 3）：`diag_text_dup_sentinel`——decode 后扫描相邻 CJK 叠字，验证 token 层去重是否漏网。附 `scan_cjk_dups` / `is_cjk_char` 单测。

> **为什么不走文本层去重**：paraformer 重复在 `all_token_ids` / `full_text` 内部，`prefix|delta` 拼接边界永不触发（prefix 空或 commit 后逗号隔开）；且全文折叠分不清「别别」artifact vs「爸爸」合法叠字。安全去重只能在 token 层（有 chunk 边界），文本层改观测哨兵。

## Phase 3 — 日志驱动精准修（paraformer）

复现「我想说话 / 开始语音识别」后据日志定位，逐项修：

### 3.1 跨边界 token 去重（`1105798`）
`process_chunk_at` step 8：本 chunk 首个有效 token == 上 chunk 末 token → CIF 双 fire，`continue` 跳过。不影响单 chunk 内合法重复。

### 3.2 mask 策略迭代（`8f32f2f` → `e802e98` → `3930dbf`）
e2e 三轮收敛（见 spec §4.2）：
- `8f32f2f`：首 chunk 不 mask left（frame0 fired 0→1）。
- `e802e98`：首 chunk 不 mask right（过度，中段退化）。
- `3930dbf`：`mask_right = !(is_first || is_final)`，仅中段 mask right。
- 最终：`mask_left = !(is_first || fresh)`，`mask_right = !is_first && !is_final`。

### 3.3 启动「嗯」门控（`968b7e5`）
`streaming_runner` 加 `seen_speech` 锁存：VAD 检出首个语音前不喂 engine（丢弃启动噪声）；VAD=None 不门控；`finish_with_tail` / `reset` 同步。

### 3.4 停顿后丢字 `fresh_segment`（`a9b55ab`）
`flush()` 末尾置 `fresh_segment=true`（零 padding 已把 `feat_cache` 冲成静音 → 新段首 chunk 不 mask left 安全）；锁存到新段首个 fire 的 chunk 才清。`flush()` 开头先清避免误 mask 段尾。

## Phase 4 — 文档 + 诊断日志清理（本步）

### 4.1 文档（CLAUDE.md 要求）
- 新建 spec：`2026-06-27-asr-streaming-token-diagnostic-design.md` ✓
- 新建 plan：`2026-06-27-asr-streaming-token-diagnostic.md`（本文档）✓
- `docs/architecture.md`：流式章节为宏观模块描述，本次为内部状态机细节（`seen_speech` / `fresh_segment` / mask 策略）+ bug fix，**不涉及接口 / 架构变更**（`has_speech` 是 Phase 1 已纳入的 trait 参数，module 表已涵盖 `StreamingEngine` trait），故不改。

### 4.2 清理诊断日志
诊断已完成、修复已 e2e 验证，删除全部诊断期临时代码：

- **paraformer**（12 处 + 哨兵）：`accept chunk` / `flush chunk` / `mask` / `decode` / `run_cif` / `force-fire` / `cif-final` / `fresh_segment 消费` / `跨边界去重` 共 9 处 `log!`；`diag_text_dup_sentinel` 调用 ×2；哨兵函数 `is_cjk_char` + `scan_cjk_dups` + `diag_text_dup_sentinel` + 注释整块；配套 2 个单测（`scan_cjk_dups_*` / `is_cjk_char_*`）。
  - **注意**：`fresh_segment 消费` / `跨边界去重` / `force-fire` / `cif-final` 4 处 log 在 `if` 块内，块内含实际副作用语句（`self.fresh_segment = false;` / `continue;` / `acoustic.extend…; alpha_cache=0; fill(0);` / `alpha_cache = integrate;`）——**只删 log，保留副作用**。
- **zipformer**（4 处）：CTC / Transducer 各 `reset` + `emit` log。
  - **注意**：两处 `let snap = self.decode_tokens/current(false);` 仅服务于 log（纯查询无副作用），删 log 须连 `let snap` 一起删，否则 unused 警告。
- **runner**（1 处）：`if !feed { log! }` 整块删（`feed` 变量在后续 `if feed {` 仍用）。

### 4.3 验证
- `cargo test -p octopus-asr-local`：全绿（哨兵单测随函数删除）。
- `cargo build -p octopus-server`：mock 签名同步编译通过。
- grep 复核：`grep -rn '\[asr-diag\]' crates/` 应为 0。

## 收尾 — 合并 main

`worktree-model-mgmt-ui` 相对 main 超前 10 commit + 文档 + 日志清理，**全量 ff-merge** 到 main（线性历史，合并与推送独立命令）。

## 验证清单

1. `cargo test -p octopus-asr-local`（streaming 单测绿）+ `cargo build -p octopus-server`。
2. paraformer e2e：连说「开始语音识别」，首字「开」在、启动无「嗯」、停顿后段间首字不丢；查日志确认 `seen_speech` 门控 / `fresh_segment` 消费 / 跨边界去重命中（清理前）。
3. 回归：长静音分段、停顿逗号行为不变。


---

## 来自原文件 `2026-06-27-global-edit-shortcut.md`

# 全局编辑快捷键（edit_global_shortcut）实施计划

> 日期：2026-06-27
> 状态：已实施（代码 + 编译验证通过；e2e 待用户桌面环境验证）
> 关联 spec：[2026-06-27-global-edit-shortcut-design.md](../specs/2026-06-27-global-edit-shortcut-design.md)

## 目标

新增全局快捷键 `edit_global_shortcut`（默认 CmdOrCtrl+Shift+E），任意应用聚焦时唤起 `result_window` 并 toggle 编辑（进入/保存），与窗口内 Cmd+Enter 并存。**用户约束：保留 Cmd+Enter 不动。**

## 任务分解（均已实施）

### Task 1：配置层 `edit_global_shortcut` 字段
- [x] `crates/infra/src/config.rs`：`AppConfig` 加 `edit_global_shortcut: String` + `#[serde(default = "default_edit_global_shortcut")]` + `default_edit_global_shortcut()` 返回 `"CmdOrCtrl+Shift+E"` + `impl Default` 同步 + 单测断言。
- [x] `crates/infra/src/db.sql`：`app_config` seed 加 `('edit_global_shortcut', 'CmdOrCtrl+Shift+E', ...)`。
- [x] `crates/infra/src/db.rs`：`load_app_config_at` + `save_app_config_at` 补 `edit_global_shortcut`（显式字段列表，漏则不存 DB / 设置页回退默认；save 长度 25→26）。

### Task 2：后端 handler + 注册
- [x] `crates/desktop/src/result_window.rs`：加 `trigger_global_edit(app)`（show + set_focus + emit `"global-edit-toggle"`）+ `register_edit_global_shortcut(app, str)`（`on_shortcut` → `trigger_global_edit`）。
- [x] `crates/desktop/src/main.rs`：setup 阶段 `asr_shortcut` 注册后加 `register_edit_global_shortcut(app.handle(), &config.edit_global_shortcut)`。

### Task 3：热重载 + 校验
- [x] `crates/desktop/src/settings_commands.rs`：
  - `apply_config_value` 加 `edit_global_shortcut` 分支（字符串校验）。
  - `set_config` 加 `edit_global_shortcut` 热重载（unregister old + register new + 失败恢复 + 持久化）；`old_shortcut` 拆为 `old_asr` + `old_edit_global`。

### Task 4：前端
- [x] `Result/index.tsx`：`toggleEdit` 声明后加独立 `useEffect` listen `"global-edit-toggle"` → `toggleEdit()`（独立 useEffect 规避 TDZ / TS2448）。
- [x] `Settings/GeneralPanel.tsx`：「快捷键」卡片加「全局编辑」行（`ShortcutButton` 组件）；**移除**「编辑模式」（`edit_shortcut`）配置行——Cmd+Enter 固定默认，不再在设置页管理。

### Task 5：构建 + 文档
- [x] `cargo check -p octopus-desktop -p octopus-infra` 通过（仅 1 个 pre-existing dead_code warning，与本次无关）。
- [x] 前端 `npm run build`（tsc + vite）通过，dist 换 bundle（`index-DyUJGfnE.js`）。
- [x] 同步文档：本计划 + spec + `architecture.md`（`result_window` 编辑入口 / `settings_commands` 26 字段 + 热重载 / 快捷键卡片移除编辑模式）。

## 验证清单（e2e，待用户在桌面环境跑）

1. 按默认 `CmdOrCtrl+Shift+E`：`result_window` 唤起到前台 + 进入编辑态（有识别结果时）。
2. 编辑态再按 `CmdOrCtrl+Shift+E`：保存（commit）。
3. 无识别结果时按：只唤起窗口，不进空编辑。
4. 窗口内 `Cmd+Enter` 仍正常（进入/保存 toggle，未受影响）。
5. 设置 → 快捷键 → 全局编辑：键盘捕获改键，热重载即时生效；**改后设置页显示新值**（DB 持久化）；冲突键报错恢复。
6. 重启应用：配置持久化，全局键仍生效（验证 DB 存取修复）。
7. 编辑态按 ESC：放弃编辑（`cancelEdit`，还原原文、不保存）；非编辑态 ESC 放弃录音（原行为）——编辑态需按 2 次 ESC 才放弃录音。保存走 Cmd+Enter 或工具栏「保存编辑」。

## e2e 调试记录：DB category 漏读 bug（2026-06-28）

**现象**：设置页「全局编辑」始终显示默认 `CmdOrCtrl+Shift+E`，但 DB 存的是改后的值（`CmdOrCtrl+Shift+Z`）、改键热重载也生效正确——「更新对、显示错」。

**根因**（纯 DB 数据层，与代码逻辑无关）：
- `app_config.category` 列在**老库**的 DEFAULT 是 `'default'`（`db.sql` 后改为 `'setting'`，但 `CREATE TABLE IF NOT EXISTS` 不更新已存在表的列 DEFAULT；`PRAGMA user_version=7` 的 migration v5→v6 只一次性改了当时的数据行，没改列 DEFAULT）。
- `load_app_config_at` 用 `WHERE category='setting'` 过滤；`save_app_config_at` 的 INSERT 不指定 category（吃列 DEFAULT）+ `ON CONFLICT(config_key) DO UPDATE SET config_value` 只改值不改 category。
- `edit_global_shortcut` 是新加字段，老库无 seed 行 → 首次 `set_config` 以列 DEFAULT=`'default'` 写入 → 被 load 的 `'setting'` 过滤漏读 → 回退 serde 默认 `CmdOrCtrl+Shift+E`。
- 写路径（save 按 `config_key` 匹配，无视图分类）+ 热重载注册（用前端传入的内存值，不经 load）都不受影响 → 所以「更新对、显示错」。

**修复**：手动修开发库——`category` 列 DEFAULT 改 `'setting'` + 既有 `default` 行改回 `setting`。**代码层零改动**（load 严格 `'setting'` 过滤 + `db.sql` DEFAULT `'setting'` 对新库本就正确）。

**教训（未来给 `AppConfig` 加字段必读）**：老库里该字段的 `app_config` 行 `category` 必须是 `'setting'`，否则被 load 漏读 → 设置页回退默认（且「改键生效但显示错」极具迷惑性）。最稳妥：确保 DB schema 列 DEFAULT=`'setting'`，或 seed/migration 显式写 `category='setting'`。

## 不改动

- 窗口内 `edit_shortcut`（Cmd+Enter）**功能**保留（前端 keydown + 字段 default），但**设置页配置行已移除**（固定 Cmd+Enter 不可改）。
- 后端编辑态命令（`enter_edit_mode` / `commit_edit`）+ `handle_enter_edit_mode` / `commit_edit_apply` 逻辑不变。


---

## 来自原文件 `2026-06-27-image-storage-blob.md`

# 图片存储迁移：文件系统 → DB BLOB 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 将剪贴板图片从文件系统迁移到 SQLite DB BLOB（WebP 无损 + 缩略图），消除文件不一致风险。

**Architecture:** 新增 `image_data` 表存 WebP BLOB。clipboard crate 的 image.rs 全面重写（文件 I/O → DB 操作）。watcher 编码流程改为 WebP。desktop 命令从 DB 读 BLOB。启动时一次性迁移旧文件。

**Tech Stack:** Rust + webp 0.3 + image 0.25 + rusqlite + Tauri

**Spec:** `docs/superpowers/specs/2026-06-27-image-storage-blob-design.md`

---

## 文件结构

| 文件 | 变更类型 | 责任 |
|---|---|---|
| `crates/infra/src/db.sql` | Modify | 新增 image_data 表 CREATE + DB v7 迁移 |
| `crates/infra/src/db.rs` | Modify | init_schema v6→v7 分支 |
| `crates/clipboard/src/image.rs` | **Rewrite** | 删文件 I/O，改为 DB BLOB 编码/读取/删除 |
| `crates/clipboard/src/store.rs` | Modify | 新增 image_data CRUD + delete_item/clear_history 引用计数 |
| `crates/clipboard/src/watcher.rs` | Modify | 图片编码流程改为 WebP → DB |
| `crates/clipboard/src/cleanup.rs` | Modify | 删除文件系统 blob 回收，改为 DB 清理 |
| `crates/desktop/src/clipboard_commands.rs` | Modify | save_image_item/ocr_image 从 DB 读 BLOB + 新增 get_image_thumb |
| `crates/desktop/src/main.rs` | Modify | 注册 get_image_thumb + 迁移调用 |
| `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | Modify | 图片条目内联缩略图 |
| `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx` | Modify | 管理页图片条目缩略图 |

---

### Task 1: DB — image_data 表 + v7 迁移

**Files:**
- Modify: `crates/infra/src/db.sql`
- Modify: `crates/infra/src/db.rs`

- [ ] **Step 1: db.sql 新增 image_data 表**

在 clipboard_history 表块之后（FTS5 之前）添加：

```sql

-- ── 图片 BLOB 存储（image_data 表）─────────────────────────────────────────
-- 替代文件系统 clipboard_images/，WebP 无损 + 缩略图存 DB，引用计数回收。
CREATE TABLE IF NOT EXISTS image_data (
    hash       TEXT PRIMARY KEY,     -- SHA-256(PNG bytes)，去重键
    blob       BLOB NOT NULL,        -- WebP 100% 无损原图
    thumb      BLOB NOT NULL,        -- WebP 20% 缩略图（240×240 Lanczos resize）
    width      INTEGER NOT NULL,
    height     INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
```

- [ ] **Step 2: db.rs init_schema 新增 v6→v7 迁移**

在 `init_schema` 函数的 `v == 5` 分支之后，添加：

```rust
    } else if v == 6 {
        // v6 → v7：image_data 表
        log::info!("DB migrating v6 → v7: adding image_data table...");
        conn.execute_batch(INIT_SQL).context("v6→v7: 建 image_data 表")?;
        conn.execute("PRAGMA user_version = 7", [])?;
        log::info!("DB migrated to v7: image_data");
    }
```

同时把 `v < 2` 分支的 `PRAGMA user_version = 6` 改为 `= 7`，以及 v2/v3/v4/v5 各分支末尾的 `= 6` 也改为 `= 7`（新用户直接到 v7，中间版本跳到 v7）。

- [ ] **Step 3: 验证编译**

```bash
cargo build -p octopus-infra 2>&1 | tail -3
```

- [ ] **Step 4: 手动验证迁移**

```bash
sqlite3 ~/.octopus/octopus.db "PRAGMA user_version;"
cargo run -p octopus-infra 2>/dev/null; # 或直接启动应用
sqlite3 ~/.octopus/octopus.db "PRAGMA user_version;"
sqlite3 ~/.octopus/octopus.db ".schema image_data"
```

Expected: user_version 从 6 → 7，image_data 表存在。

- [ ] **Step 5: Commit**

```bash
git add crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): image_data 表 + DB v7 迁移"
```

---

### Task 2: image.rs 全面重写

**Files:**
- Rewrite: `crates/clipboard/src/image.rs`

- [ ] **Step 1: 重写 image.rs**

```rust
//! 图片编码：RGBA → PNG → SHA-256 → WebP 无损 + 缩略图 → DB BLOB。
//! 替代旧文件系统方案，不再写 ~/.octopus/clipboard_images/。

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// RGBA 像素 → PNG bytes + SHA-256 hash。
/// hash 用于去重（同一张图只存一份 BLOB）。
pub fn encode_and_hash(rgba: &[u8], width: u32, height: u32) -> Result<(Vec<u8>, String)> {
    let img = ::image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .context("Failed to create RgbaImage from raw pixels")?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
        .context("Failed to encode PNG")?;
    let hash = sha256_hex(&png_bytes);
    Ok((png_bytes, hash))
}

/// 编码结果：WebP 无损原图 + WebP 缩略图。
pub struct EncodedImage {
    pub webp_blob: Vec<u8>,
    pub thumb_blob: Vec<u8>,
}

/// PNG bytes → WebP 100% 无损 + 缩略图 WebP 20%（240×240 Lanczos）。
pub fn encode_to_webp(png_bytes: &[u8], width: u32, height: u32) -> Result<EncodedImage> {
    let img = ::image::load_from_memory_with_format(png_bytes, ::image::ImageFormat::Png)
        .context("Failed to decode PNG for WebP encoding")?;
    let rgba = img.to_rgba8();

    // 无损 WebP 原图
    let encoder = webp::Encoder::from_rgba(&rgba, rgba.width(), rgba.height());
    let webp_blob = encoder.encode_lossless();
    let webp_blob = webp_blob.to_vec();

    // 缩略图：resize 240×240 → WebP 20%
    let thumb_img = img.resize(240, 240, ::image::imageops::FilterType::Lanczos3);
    let thumb_rgba = thumb_img.to_rgba8();
    let thumb_encoder = webp::Encoder::from_rgba(&thumb_rgba, thumb_rgba.width(), thumb_rgba.height());
    let thumb_blob = thumb_encoder.encode(20.0);
    let thumb_blob = thumb_blob.to_vec();

    Ok(EncodedImage { webp_blob, thumb_blob })
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in result {
        hex.push_str(&format!("{:02x}", byte));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_and_hash() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
        let (png, hash) = encode_and_hash(&rgba, 2, 2).unwrap();
        assert!(!png.is_empty());
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_dedup_same_image() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let (_, hash1) = encode_and_hash(&rgba, 2, 1).unwrap();
        let (_, hash2) = encode_and_hash(&rgba, 2, 1).unwrap();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_encode_to_webp() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
        let (png, _) = encode_and_hash(&rgba, 2, 2).unwrap();
        let encoded = encode_to_webp(&png, 2, 2).unwrap();
        assert!(!encoded.webp_blob.is_empty());
        assert!(!encoded.thumb_blob.is_empty());
        // WebP 文件头：RIFF
        assert_eq!(&encoded.webp_blob[..4], b"RIFF");
        assert_eq!(&encoded.thumb_blob[..4], b"RIFF");
    }
}
```

- [ ] **Step 2: 验证编译 + 测试**

```bash
cargo test -p octopus-clipboard --lib image 2>&1 | tail -10
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/clipboard/src/image.rs
git commit -m "feat(clipboard): image.rs 重写为 WebP DB BLOB 编码"
```

---

### Task 3: store.rs — image_data CRUD + 删除引用计数

**Files:**
- Modify: `crates/clipboard/src/store.rs`

- [ ] **Step 1: 新增 image_data CRUD 函数**

在 `get_referenced_blob_hashes` 函数之后添加：

```rust
/// ── image_data 表 CRUD ──

/// 插入图片 BLOB（WebP 无损 + 缩略图）。
pub fn insert_image_data(
    conn: &Connection,
    hash: &str,
    webp_blob: &[u8],
    thumb_blob: &[u8],
    width: i64,
    height: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO image_data (hash, blob, thumb, width, height, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
        params![hash, webp_blob, thumb_blob, width, height, iso_now()],
    )?;
    Ok(())
}

/// 读取图片 WebP 无损 BLOB。
pub fn get_image_blob(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT blob FROM image_data WHERE hash = ?")?;
    let row = stmt.query_row(params![hash], |r| r.get::<_, Vec<u8>>(0));
    match row {
        Ok(blob) => Ok(Some(blob)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 读取缩略图 WebP BLOB。
pub fn get_image_thumb(conn: &Connection, hash: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare("SELECT thumb FROM image_data WHERE hash = ?")?;
    let row = stmt.query_row(params![hash], |r| r.get::<_, Vec<u8>>(0));
    match row {
        Ok(blob) => Ok(Some(blob)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 删除 image_data 中无引用的 BLOB（引用计数为 0）。
pub fn cleanup_unreferenced_images(conn: &Connection) -> Result<usize> {
    let deleted = conn.execute(
        "DELETE FROM image_data WHERE hash NOT IN (
            SELECT DISTINCT blob_hash FROM clipboard_history WHERE blob_hash IS NOT NULL
        )",
        [],
    )?;
    Ok(deleted)
}

/// 删除指定 hash 的 image_data（如果无其他条目引用）。
fn delete_image_if_unreferenced(conn: &Connection, hash: &str) {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM clipboard_history WHERE blob_hash = ?",
            params![hash],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if count == 0 {
        let _ = conn.execute(
            "DELETE FROM image_data WHERE hash = ?",
            params![hash],
        );
    }
}
```

- [ ] **Step 2: 修改 delete_item 使用引用计数**

```rust
/// 删除单条。若被删的是图片且无其他条目引用同一 blob，顺带删除 image_data 行。
pub fn delete_item(conn: &Connection, id: i64) -> Result<()> {
    let blob_hash: Option<String> = conn
        .query_row(
            "SELECT blob_hash FROM clipboard_history WHERE id = ?",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();

    conn.execute("DELETE FROM clipboard_history WHERE id = ?", params![id])?;
    track_deletes(conn, 1);

    if let Some(hash) = blob_hash {
        delete_image_if_unreferenced(conn, &hash);
    }

    Ok(())
}
```

- [ ] **Step 3: 修改 clear_history 使用 cleanup_unreferenced_images**

```rust
/// 清空历史（可选保留收藏）。删除后回收无引用的 image_data BLOB。
pub fn clear_history(conn: &Connection, keep_favorite: bool) -> Result<usize> {
    let rows = if keep_favorite {
        conn.execute("DELETE FROM clipboard_history WHERE is_favorite = 0", [])?
    } else {
        conn.execute("DELETE FROM clipboard_history", [])?
    };
    if rows > 0 {
        track_deletes(conn, rows as u32);
        cleanup_unreferenced_images(conn)?;
    }
    Ok(rows)
}
```

- [ ] **Step 4: 删除旧的 get_referenced_blob_hashes + 文件系统引用**

删除 `get_referenced_blob_hashes` 函数（不再需要——cleanup_unreferenced_images 用 SQL 子查询替代）。

删除 `delete_item` 和 `clear_history` 中对 `crate::image::delete_blob_files` / `crate::image::cleanup_orphaned_blobs` 的调用（如果存在）。

- [ ] **Step 5: 验证编译 + 测试**

```bash
cargo test -p octopus-clipboard 2>&1 | tail -8
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/clipboard/src/store.rs
git commit -m "feat(clipboard): image_data CRUD + 引用计数删除"
```

---

### Task 4: watcher.rs — 图片编码流程改为 WebP → DB

**Files:**
- Modify: `crates/clipboard/src/watcher.rs`

- [ ] **Step 1: 修改 image 分支**

将 image 分支（约 line 109-157）替换为：

```rust
        } else if handle.has(ContentFormat::Image) {
            // image 类型
            let img_data = handle.read_image()?;
            let (w, h) = img_data.get_size();

            // 超过 40MB 跳过
            let estimated_size = (w as usize) * (h as usize) * 4;
            if estimated_size > 40 * 1024 * 1024 {
                log::info!("Skipping large image ({}x{} ~{}MB)", w, h, estimated_size / 1024 / 1024);
                return Ok(());
            }

            let rgba_img = img_data.to_rgba8()
                .map_err(|e| anyhow::anyhow!("to_rgba8 failed: {}", e))?;
            let rgba = rgba_img.to_vec();
            let (png_bytes, hash) = image::encode_and_hash(&rgba, w, h)?;

            // 去重
            let existing = octopus_infra::db::with_db(|conn| {
                store::find_by_content_hash(conn, &hash)
            })?;

            if let Some(id) = existing {
                octopus_infra::db::with_db(|conn| store::touch_created_at(conn, id))?;
            } else {
                // 编码 WebP 无损 + 缩略图
                let encoded = image::encode_to_webp(&png_bytes, w, h)?;

                // 存 image_data BLOB
                octopus_infra::db::with_db(|conn| {
                    store::insert_image_data(conn, &hash, &encoded.webp_blob, &encoded.thumb_blob, w as i64, h as i64)
                })?;

                // 存 clipboard_history 条目
                octopus_infra::db::with_db(|conn| {
                    store::insert_clipboard_item(conn, &store::NewClipboardItem {
                        id: store::chrono_millis(),
                        item_type: ItemType::Image,
                        content: hash.clone(),
                        search_text: String::new(),
                        created_at: store::iso_now(),
                        blob_hash: Some(hash),
                        width: Some(w as i64),
                        height: Some(h as i64),
                        has_thumbnail: Some(1),
                        file_count: None,
                        is_rich: false,
                    })
                })?;
            }
```

- [ ] **Step 2: 删除不再使用的 import**

检查文件头，删除 `use crate::image;` 如果不再直接引用（现在通过 `image::encode_and_hash` 和 `image::encode_to_webp` 调用，仍需保留）。

- [ ] **Step 3: 验证编译**

```bash
cargo build -p octopus-clipboard 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/clipboard/src/watcher.rs
git commit -m "feat(clipboard): watcher 图片编码改为 WebP → DB BLOB"
```

---

### Task 5: cleanup.rs — 删除文件系统 blob 回收

**Files:**
- Modify: `crates/clipboard/src/cleanup.rs`

- [ ] **Step 1: 修改 run_cleanup**

将步骤 3（孤立 blob 回收）改为 DB 清理：

```rust
    // 3. 无引用 image_data BLOB 清理
    let reclaimed = crate::store::cleanup_unreferenced_images(conn)?;
```

删除原来调用 `crate::image::cleanup_orphaned_blobs` 和 `crate::store::get_referenced_blob_hashes` 的代码。

- [ ] **Step 2: 验证编译 + 测试**

```bash
cargo test -p octopus-clipboard 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
git add crates/clipboard/src/cleanup.rs
git commit -m "refactor(clipboard): cleanup 改为 DB BLOB 引用计数清理"
```

---

### Task 6: desktop — save_image_item / ocr_image 从 DB 读 + get_image_thumb

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs`
- Modify: `crates/desktop/src/main.rs`

- [ ] **Step 1: 修改 save_image_item 从 DB 读 WebP BLOB**

将 `save_image_item` 中读文件的部分：

```rust
    // 旧代码：
    let orig_path = octopus_clipboard::image::clipboard_images_dir()
        .join(format!("{}.png", blob_hash));
    let png_bytes = std::fs::read(&orig_path).map_err(|e| e.to_string())?;
```

替换为：

```rust
    // 新代码：从 DB 读 WebP 无损 BLOB
    let webp_blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;
```

然后修改保存逻辑——WebP 格式直接写 BLOB，JPEG/PNG 先解码再编码：

```rust
    // 按扩展名保存
    match ext {
        "png" => {
            // WebP → 解码 → PNG
            let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
                .map_err(|e| e.to_string())?;
            img.save_with_format(save_path, ::image::ImageFormat::Png)
                .map_err(|e| e.to_string())?;
        }
        "webp" => {
            // 直接写原始 WebP bytes（已是无损）
            std::fs::write(save_path, &webp_blob).map_err(|e| e.to_string())?;
        }
        _ => {
            // JPEG：WebP → 解码 → JPEG
            let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
                .map_err(|e| e.to_string())?;
            octopus_infra::image_util::save_as_jpeg_from_image(&img, save_path, q)
                .map_err(|e| e.to_string())?;
        }
    }
```

注意：infra::image_util 需要新增 `save_as_jpeg_from_image(img: &DynamicImage, ...)` 函数，或直接在 clipboard_commands 中内联 JPEG 编码。选择内联以减少跨 crate 变更：

```rust
    _ => {
        let img = ::image::load_from_memory_with_format(&webp_blob, ::image::ImageFormat::WebP)
            .map_err(|e| e.to_string())?;
        let rgb = img.to_rgb8();
        let mut buf = std::io::BufWriter::new(
            std::fs::File::create(save_path).map_err(|e| e.to_string())?
        );
        let mut encoder = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
        encoder.encode(&rgb, rgb.width(), rgb.height(), ::image::ExtendedColorType::Rgb8)
            .map_err(|e| e.to_string())?;
    }
```

- [ ] **Step 2: 修改 ocr_image 从 DB 读 WebP BLOB**

将 `ocr_image` 中读文件的部分：

```rust
    // 旧代码：
    let orig_path = octopus_clipboard::image::clipboard_images_dir()
        .join(format!("{}.png", blob_hash));
    let png_bytes = std::fs::read(&orig_path).map_err(|e| e.to_string())?;
```

替换为：

```rust
    // 新代码：从 DB 读 WebP 无损 BLOB
    let webp_blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;
```

然后修改 `engine.recognize` 调用——传入 WebP bytes，OcrEngine::recognize 需要支持 WebP 格式：

修改 `crates/ocr/src/engine.rs` 的 recognize 方法，把 `ImageFormat::Png` 改为自动检测：

```rust
    pub fn recognize(&self, image_bytes: &[u8]) -> Result<String> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;
        // ... 其余不变
    }
```

- [ ] **Step 3: 新增 get_image_thumb 命令**

在 clipboard_commands.rs 末尾添加：

```rust
/// 获取图片缩略图（WebP bytes → 前端 base64 展示）。
#[tauri::command]
pub async fn get_image_thumb(id: i64) -> Result<Vec<u8>, String> {
    let item = octopus_infra::db::with_db(|conn| {
        let items = octopus_clipboard::store::query_history(conn, &QueryFilter {
            filter: "all".into(),
            search: None,
            page: 1,
            size: 1000,
        })?;
        Ok::<_, anyhow::Error>(items.into_iter().find(|i| i.id == id))
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
        .ok_or("图片元数据缺失")?;

    let thumb = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_thumb(conn, &blob_hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("缩略图不存在")?;

    Ok(thumb)
}
```

- [ ] **Step 4: main.rs 注册 get_image_thumb**

在 `ocr_image` 之后添加：

```rust
            clipboard_commands::get_image_thumb,
```

- [ ] **Step 5: 验证编译**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs crates/ocr/src/engine.rs
git commit -m "feat(desktop): 从 DB 读图片 BLOB + get_image_thumb 命令"
```

---

### Task 7: 旧文件迁移

**Files:**
- Create: `crates/desktop/src/image_migration.rs`
- Modify: `crates/desktop/src/main.rs`

- [ ] **Step 1: 创建迁移模块**

```rust
//! 一次性迁移：~/.octopus/clipboard_images/ → image_data DB BLOB。
//! 幂等：已存在的 hash 跳过。迁移完成后删除目录。

use std::path::PathBuf;

fn clipboard_images_dir() -> PathBuf {
    octopus_infra::paths::octopus_config_home().join("clipboard_images")
}

/// 迁移文件系统图片到 DB。成功后删除目录。
pub fn migrate_images_to_db() {
    let dir = clipboard_images_dir();
    if !dir.exists() {
        return;
    }

    log::info!("Migrating clipboard_images/ to DB...");

    let mut migrated = 0;
    let mut skipped = 0;
    let mut errors = 0;

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Failed to read clipboard_images/: {}", e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();

        // 只处理 <hash>.png（跳过 _thumb.png）
        if !filename.ends_with(".png") || filename.contains("_thumb") {
            continue;
        }

        let hash = filename.trim_end_matches(".png").to_string();

        // 检查 DB 是否已有此 hash
        let exists = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::get_image_blob(conn, &hash)
        }).map(|v| v.is_some()).unwrap_or(false);

        if exists {
            skipped += 1;
            continue;
        }

        // 读取 PNG → 编码 WebP → 存 DB
        match std::fs::read(&path) {
            Ok(png_bytes) => {
                match ::image::load_from_memory_with_format(&png_bytes, ::image::ImageFormat::Png) {
                    Ok(img) => {
                        let w = img.width();
                        let h = img.height();
                        match octopus_clipboard::image::encode_to_webp(&png_bytes, w, h) {
                            Ok(encoded) => {
                                let result = octopus_infra::db::with_db(|conn| {
                                    octopus_clipboard::store::insert_image_data(
                                        conn, &hash, &encoded.webp_blob, &encoded.thumb_blob,
                                        w as i64, h as i64,
                                    )
                                });
                                match result {
                                    Ok(_) => migrated += 1,
                                    Err(e) => {
                                        log::warn!("Failed to insert {}: {}", hash, e);
                                        errors += 1;
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to encode {}: {}", hash, e);
                                errors += 1;
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to decode {}: {}", hash, e);
                        errors += 1;
                    }
                }
            }
            Err(e) => {
                log::warn!("Failed to read {}: {}", path.display(), e);
                errors += 1;
            }
        }
    }

    log::info!(
        "Image migration: {} migrated, {} skipped, {} errors",
        migrated, skipped, errors
    );

    // 全部成功（无错误）才删除目录
    if errors == 0 {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            log::warn!("Failed to remove clipboard_images/: {}", e);
        } else {
            log::info!("Removed clipboard_images/ directory");
        }
    }
}
```

- [ ] **Step 2: main.rs 注册模块 + 启动时调用**

在 main.rs 的 mod 声明区添加：

```rust
mod image_migration;
```

在 `setup` 中（FTS5 rebuild 之后）添加：

```rust
            // 迁移旧文件系统图片到 DB BLOB
            image_migration::migrate_images_to_db();
```

注意：main.rs 需要添加 `octopus_clipboard` 和 `image` crate 的引用。检查 desktop Cargo.toml 是否已有 `image` 依赖——可能需要添加。

- [ ] **Step 3: 验证编译**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/image_migration.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): 旧文件系统图片迁移到 DB BLOB"
```

---

### Task 8: 前端 — 图片条目内联缩略图

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx`

- [ ] **Step 1: ClipboardItem.tsx — 图片条目加载缩略图**

在组件内加缩略图状态：

```typescript
  const [thumbSrc, setThumbSrc] = useState<string | null>(null);
```

加 useEffect 加载缩略图：

```typescript
  useEffect(() => {
    if (item.item_type === "image") {
      invoke<number[]>("get_image_thumb", { id: item.id })
        .then((bytes) => {
          const base64 = btoa(bytes.map(b => String.fromCharCode(b)).join(""));
          setThumbSrc(`data:image/webp;base64,${base64}`);
        })
        .catch(() => {});
    }
  }, [item.id, item.item_type]);
```

修改图片条目的内容渲染——替换「图片 WxH」文字为缩略图 + 尺寸：

```tsx
        {item.item_type === "image" && item.image_meta ? (
          <div className="flex items-center gap-2">
            {thumbSrc && (
              <img src={thumbSrc} className="w-10 h-10 rounded object-cover flex-shrink-0" alt="" />
            )}
            <span className="text-xs text-muted-foreground">
              {item.image_meta.width}×{item.image_meta.height}
            </span>
          </div>
        ) : item.item_type === "file" ? (
```

- [ ] **Step 2: ClipboardPanel.tsx — 管理页同样加载缩略图**

在 ClipboardRow 内加同样的 thumbSrc 状态 + useEffect + 渲染。

- [ ] **Step 3: 构建前端**

```bash
cd crates/desktop/frontend && npm run build 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/ crates/desktop/dist/
git commit -m "feat(clipboard): 图片条目内联缩略图展示"
```

---

### Task 9: 清理旧代码

**Files:**
- Modify: `crates/clipboard/src/image.rs`（删除 clipboard_images_dir / save_image / ImageSaveResult / generate_thumbnail / cleanup_orphaned_blobs / delete_blob_files）
- Modify: `crates/clipboard/src/lib.rs`（确认 pub mod image 仍需）
- Modify: `crates/clipboard/src/store.rs`（删除 get_referenced_blob_hashes 如果不再使用）

- [ ] **Step 1: 确认所有旧引用已清除**

```bash
grep -rn "clipboard_images_dir\|save_image\|ImageSaveResult\|cleanup_orphaned_blobs\|delete_blob_files\|get_referenced_blob_hashes" crates/ --include="*.rs"
```

Expected: 无输出（全部已清除）。

- [ ] **Step 2: 验证编译 + 全量测试**

```bash
cargo test -p octopus-clipboard 2>&1 | tail -5
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(clipboard): 清理旧文件系统图片代码"
```

---

### Task 10: 端到端验证

- [ ] **Step 1: 完整构建**

```bash
cd crates/desktop/frontend && npm run build
cd .. && cargo build --features embedded 2>&1 | tail -5
```

- [ ] **Step 2: 运行应用，截图测试**

```bash
./run-octopus.sh
```

验证：
1. 截图 → 剪贴板浮窗显示缩略图（不是纯文字）
2. 点击 OCR → 识别成功
3. 删除图片条目 → image_data 表对应行也删了：`sqlite3 ~/.octopus/octopus.db "SELECT COUNT(*) FROM image_data;"`
4. 导出图片为 JPEG/PNG/WebP → 文件正确
5. `~/.octopus/clipboard_images/` 目录被删除（迁移完成）

- [ ] **Step 3: Commit（如有修复）**

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1 新增 image_data 表 | Task 1 |
| §2 存储策略（WebP 无损 + 20% 缩略图） | Task 2 |
| §3 编码流程 | Task 2 + Task 4 |
| §4.1 OCR 读取 | Task 6 |
| §4.2 前端缩略图 | Task 6 + Task 8 |
| §4.3 导出保存 | Task 6 |
| §5 删除引用计数 | Task 3 |
| §6 删除清单 | Task 9 |
| §7 迁移策略 | Task 7 |
| §8 依赖（已有） | 无需 task |
| §9 DB v7 | Task 1 |

---

## 实施偏差与补充记录

### 偏差 1：image_type 字段

spec 原设计无 `image_type` 列，实施时用户要求新增（预留未来 PNG/JPEG 格式扩展）。DB schema 和 `insert_image_data` 均含 `image_type TEXT NOT NULL DEFAULT 'webp'`。

### 偏差 2：encode_to_webp 参数未使用

`encode_to_webp(png_bytes, _width, _height)` 的 width/height 参数实际未使用（图片尺寸从 PNG 解码内部获取），保留下划线前缀兼容调用方签名（watcher.rs 传入 w/h）。

### 偏差 3：编译 warning 清理

- `store.rs` 删除未使用的 `use std::collections::HashSet`（`get_referenced_blob_hashes` 被删后无引用）
- `image.rs` `encode_to_webp` 参数加 `_` 前缀

### 偏差 4：desktop Cargo.toml 新增 image 依赖

`image_migration.rs` 模块需要 `image` crate 做格式转换，desktop Cargo.toml 新增 `image = { version = "0.25", features = ["png", "webp", "jpeg"] }`。

### 偏差 5：save_image_item 导出逻辑变更

原计划从文件系统读 PNG → `infra::image_util` 转码。实施改为从 DB 读 WebP BLOB → `image` crate 解码 → 按目标格式编码（JPEG/PNG 解码再编码，WebP 直接写原始 BLOB）。不再依赖 `infra::image_util`。

### 偏差 6：端到端验证通过

用户确认：截图 → 缩略图显示 → OCR 识别 → 删除条目 → image_data 引用计数回收 → 导出 JPEG/WebP/PNG 全部正常。


---

## 来自原文件 `2026-06-27-ocr-module.md`

# OCR 模块实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为剪贴板图片条目添加 OCR 识别能力（ocr-rs/MNN + PP-OCRv6），识别文本写入 search_text + 系统剪贴板 + 新建文档。

**Architecture:** 独立 crate `octopus-ocr`（依赖 infra），desktop 层编排调用。ocr-rs 封装 det→crop→rec pipeline，MNN 后端推理。模型三件套（det.mnn/rec.mnn/keys.txt）已手动放置于 `~/.octopus/models/ocr/PP-OCRv6-small/`。DB 零 schema 变更，复用 models 表 + app_config。

**Tech Stack:** Rust + ocr-rs 2.3（MNN）+ image 0.25 + Tauri + React + lucide-react

**Spec:** `docs/superpowers/specs/2026-06-27-ocr-module-design.md`

---

## 文件结构

| 文件 | 责任 |
|---|---|
| **Create:** `crates/ocr/Cargo.toml` | crate 清单，依赖 ocr-rs/image/infra |
| **Create:** `crates/ocr/src/lib.rs` | 模块入口，pub use |
| **Create:** `crates/ocr/src/engine.rs` | OcrEngine 封装：单例 + recognize() |
| **Create:** `crates/ocr/src/model.rs` | 模型路径管理 + is_model_ready |
| **Modify:** `Cargo.toml`（workspace） | members 新增 crates/ocr |
| **Modify:** `crates/desktop/Cargo.toml` | 新增 octopus-ocr 依赖 |
| **Modify:** `crates/desktop/src/clipboard_commands.rs` | 新增 ocr_image 命令 |
| **Modify:** `crates/desktop/src/main.rs` | 注册 ocr_image 命令 |
| **Modify:** `crates/clipboard/src/store.rs` | 新增 update_search_text |
| **Modify:** `crates/infra/src/db.sql` | 新增 OCR models seed + app_config seed |
| **Modify:** `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | OCR 按钮 + 状态机 |
| **Modify:** `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx` | 管理页 OCR 按钮 |

---

### Task 1: octopus-ocr crate 骨架

**Files:**
- Create: `crates/ocr/Cargo.toml`
- Create: `crates/ocr/src/lib.rs`
- Modify: `Cargo.toml`（workspace root，line 2）

- [ ] **Step 1: 创建 crate 目录结构**

```bash
mkdir -p crates/ocr/src
```

- [ ] **Step 2: 写 Cargo.toml**

```toml
[package]
name = "octopus-ocr"
version = "0.1.0"
edition = "2021"

[dependencies]
octopus-infra = { path = "../infra" }
ocr-rs = "2.3"
image = "0.25"
anyhow = "1"
log = "0.4"
```

- [ ] **Step 3: 写 lib.rs（最小骨架）**

```rust
pub mod engine;
pub mod model;
```

- [ ] **Step 4: 写 engine.rs 占位（后续 Task 填充）**

```rust
use anyhow::Result;
use std::sync::Arc;

pub struct OcrEngine {
    inner: ocr_rs::OcrEngine,
}

impl OcrEngine {
    pub fn instance() -> Result<Arc<OcrEngine>> {
        anyhow::bail!("not implemented yet")
    }

    pub fn recognize(&self, _png_bytes: &[u8]) -> Result<String> {
        anyhow::bail!("not implemented yet")
    }
}
```

- [ ] **Step 5: 写 model.rs**

```rust
use std::path::PathBuf;

pub const DEFAULT_OCR_MODEL: &str = "PP-OCRv6-small";

/// 模型组目录：~/.octopus/models/ocr/<model_name>/
pub fn model_dir(model_name: &str) -> PathBuf {
    octopus_infra::paths::octopus_config_home()
        .join("models")
        .join("ocr")
        .join(model_name)
}

/// 检查模型组三件套是否就绪：det.mnn + rec.mnn + keys.txt
pub fn is_model_ready(model_name: &str) -> bool {
    let dir = model_dir(model_name);
    dir.join("det.mnn").exists()
        && dir.join("rec.mnn").exists()
        && dir.join("keys.txt").exists()
}
```

- [ ] **Step 6: workspace Cargo.toml 加 member**

在 `Cargo.toml` line 2 的 members 列表末尾加 `"crates/ocr"`：

```toml
members = ["crates/infra", "crates/asr-local", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download", "crates/clipboard", "crates/ocr"]
```

- [ ] **Step 7: 验证编译**

```bash
cargo build -p octopus-ocr 2>&1 | tail -5
```

Expected: 编译通过（可能有 unused warning）。如果 ocr-rs 编译失败需排查 cmake/cc 依赖。

- [ ] **Step 8: Commit**

```bash
git add crates/ocr/ Cargo.toml
git commit -m "feat(ocr): octopus-ocr crate 骨架（engine/model 占位）"
```

---

### Task 2: OcrEngine 实现

**Files:**
- Modify: `crates/ocr/src/engine.rs`

- [ ] **Step 1: 实现 OcrEngine（完整）**

```rust
use anyhow::{Context, Result};
use std::sync::{Arc, OnceLock};

use crate::model;

pub struct OcrEngine {
    inner: ocr_rs::OcrEngine,
}

static INSTANCE: OnceLock<Arc<OcrEngine>> = OnceLock::new();

impl OcrEngine {
    /// 全局单例，首次调用时懒加载。
    /// model_name 从 app_config.ocr_model 读取，默认 PP-OCRv6-small。
    pub fn instance() -> Result<Arc<OcrEngine>> {
        if let Some(e) = INSTANCE.get() {
            return Ok(e.clone());
        }

        let model_name = octopus_infra::db::load_config_key("ocr_model")
            .unwrap_or_else(|| model::DEFAULT_OCR_MODEL.to_string());

        if !model::is_model_ready(&model_name) {
            anyhow::bail!("OCR 模型未就绪: {}（请检查 ~/.octopus/models/ocr/{}/）", model_name, model_name);
        }

        let dir = model::model_dir(&model_name);
        let det_path = dir.join("det.mnn");
        let rec_path = dir.join("rec.mnn");
        let keys_path = dir.join("keys.txt");

        log::info!("Loading OCR model: {} from {}", model_name, dir.display());

        let inner = ocr_rs::OcrEngine::new(
            det_path.to_str().context("invalid det path")?,
            rec_path.to_str().context("invalid rec path")?,
            keys_path.to_str().context("invalid keys path")?,
            None,
        ).context("Failed to init ocr_rs::OcrEngine")?;

        let engine = Arc::new(OcrEngine { inner });

        // OnceLock::set 如果已设置则忽略（竞争安全，两个线程都加载只是浪费一次）
        let _ = INSTANCE.set(engine.clone());

        Ok(engine)
    }

    /// 识别图片字节（PNG），返回识别文本（多行用 \n 连接）。
    pub fn recognize(&self, png_bytes: &[u8]) -> Result<String> {
        let img = ::image::load_from_memory_with_format(png_bytes, ::image::ImageFormat::Png)
            .context("Failed to decode PNG")?;

        let results = self.inner.recognize(&img)
            .context("OCR recognize failed")?;

        let text: Vec<String> = results.into_iter()
            .map(|r| r.text)
            .collect();

        Ok(text.join("\n"))
    }
}
```

- [ ] **Step 2: 验证编译**

```bash
cargo build -p octopus-ocr 2>&1 | tail -10
```

Expected: 编译通过。如果 ocr-rs API 签名与预期不符（`OcrEngine::new` 参数或 `recognize` 返回类型），需查阅 `cargo doc -p ocr-rs --open` 调整。

- [ ] **Step 3: 写集成测试验证真实模型**

在 `crates/ocr/tests/ocr_integration.rs`：

```rust
use octopus_ocr::OcrEngine;

#[test]
fn test_recognize_real_model() {
    // 跳过条件：模型未就绪
    if !octopus_ocr::model::is_model_ready("PP-OCRv6-small") {
        eprintln!("Skipping: OCR model not ready");
        return;
    }

    let engine = OcrEngine::instance().expect("Failed to init engine");

    // 用一张含中文的测试图片（如果没有就跳过）
    let test_img = std::env::var("OCTOPUS_OCR_TEST_IMAGE").ok();
    let test_img = match test_img {
        Some(p) => p,
        None => {
            eprintln!("Skipping: set OCTOPUS_OCR_TEST_IMAGE=/path/to/test.png");
            return;
        }
    };

    let png_bytes = std::fs::read(&test_img).expect("read test image");
    let text = engine.recognize(&png_bytes).expect("recognize");
    assert!(!text.is_empty(), "OCR should return text");
    println!("OCR result: {}", text);
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p octopus-ocr --test ocr_integration -- --nocapture
```

Expected: 如果模型就绪且有测试图片，输出识别文本。否则打印 skip 信息。

- [ ] **Step 5: Commit**

```bash
git add crates/ocr/
git commit -m "feat(ocr): OcrEngine 实现（单例懒加载 + recognize）"
```

---

### Task 3: store.rs 新增 update_search_text

**Files:**
- Modify: `crates/clipboard/src/store.rs`（toggle_favorite 函数之后，约 line 249）

- [ ] **Step 1: 添加 update_search_text 函数**

在 `toggle_favorite` 函数之后添加：

```rust
/// 更新条目的 search_text（OCR 场景：识别后让图片可搜索）。
pub fn update_search_text(conn: &Connection, id: i64, search_text: &str) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_history SET search_text = ? WHERE id = ?",
        params![search_text, id],
    )?;
    Ok(())
}
```

- [ ] **Step 2: 验证编译**

```bash
cargo build -p octopus-clipboard 2>&1 | tail -3
```

Expected: PASS

- [ ] **Step 3: 运行测试**

```bash
cargo test -p octopus-clipboard 2>&1 | tail -5
```

Expected: 14 passed; 0 failed

- [ ] **Step 4: Commit**

```bash
git add crates/clipboard/src/store.rs
git commit -m "feat(clipboard): update_search_text（OCR 文本回写）"
```

---

### Task 4: DB seed（models + app_config）

**Files:**
- Modify: `crates/infra/src/db.sql`（OCR models seed + app_config seed）

- [ ] **Step 1: 在 db.sql 的 ASR models seed 之后添加 OCR seed**

找到最后一个 `INSERT OR IGNORE INTO models` 语句之后，添加：

```sql

-- ── OCR 模型（domain='ocr'）─────────────────────────────────────────
-- source = det 下载地址；secret_key = rec 下载地址（本地模型复用字段）
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, secret_key, language, description, is_local, is_enabled, is_streaming)
VALUES
    ('ocr','paddleocr','ocr','PP-OCRv6-small',
     'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_det.mnn',
     'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_rec.mnn',
     'auto','PP-OCRv6 small (det 4.7M + rec 10M + keys 73K)，中/英/繁体/日',
     1,1,0);
```

- [ ] **Step 2: 在 app_config seed 末尾添加 ocr_model**

找到最后一个 app_config seed INSERT 之后，添加：

```sql
INSERT OR IGNORE INTO app_config (key, value, category) VALUES
    ('ocr_model', 'PP-OCRv6-small', 'setting');
```

- [ ] **Step 3: 验证编译 + DB 迁移**

```bash
cargo build -p octopus-infra 2>&1 | tail -3
```

- [ ] **Step 4: 手动验证 seed 生效**

```bash
sqlite3 ~/.octopus/octopus.db "SELECT domain, category, model_name, is_enabled FROM models WHERE domain='ocr';"
sqlite3 ~/.octopus/octopus.db "SELECT key, value FROM app_config WHERE key='ocr_model';"
```

Expected: 一行 OCR 模型记录 + 一行 ocr_model 配置。如果未出现，需检查 db.sql 是否在 init_schema 中被 execute_batch。

- [ ] **Step 5: Commit**

```bash
git add crates/infra/src/db.sql
git commit -m "feat(infra): OCR models + app_config seed"
```

---

### Task 5: desktop 新增 ocr_image 命令

**Files:**
- Modify: `crates/desktop/Cargo.toml`（新增 octopus-ocr 依赖）
- Modify: `crates/desktop/src/clipboard_commands.rs`（新增 ocr_image 命令）
- Modify: `crates/desktop/src/main.rs`（注册命令）

- [ ] **Step 1: Cargo.toml 加依赖**

在 `crates/desktop/Cargo.toml` 的 `[dependencies]` 中找到 `# octopus-asr-local` 附近，添加：

```toml
# OCR
octopus-ocr = { path = "../ocr" }
```

- [ ] **Step 2: 实现 ocr_image 命令**

在 `crates/desktop/src/clipboard_commands.rs` 末尾（open_file_item 之后）添加：

```rust
/// 图片条目 OCR：识别文本 → 写 search_text + 写剪贴板 + 新建文档。
#[tauri::command]
pub async fn ocr_image(
    id: i64,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<String, String> {
    // 1. 从 DB 读条目
    let item = octopus_infra::db::with_db(|conn| {
        let items = octopus_clipboard::store::query_history(conn, &QueryFilter {
            filter: "all".into(),
            search: None,
            page: 1,
            size: 1000,
        })?;
        Ok::<_, anyhow::Error>(items.into_iter().find(|i| i.id == id))
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
        .ok_or("图片元数据缺失")?;

    // 2. 读原图 PNG
    let orig_path = octopus_clipboard::image::clipboard_images_dir()
        .join(format!("{}.png", blob_hash));
    let png_bytes = std::fs::read(&orig_path).map_err(|e| e.to_string())?;

    // 3. OCR 识别
    let engine = octopus_ocr::engine::OcrEngine::instance()
        .map_err(|e| e.to_string())?;
    let text = engine.recognize(&png_bytes).map_err(|e| e.to_string())?;

    if text.trim().is_empty() {
        return Err("未识别到文本".into());
    }

    // 4. 写 search_text（FTS5 可搜索）
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_search_text(conn, id, &text)
    }).map_err(|e| e.to_string())?;

    // 5. 写系统剪贴板
    handle.write_text(&text).map_err(|e| e.to_string())?;

    // 6. 系统文本编辑器新建无标题文档
    open_text_editor_with_content(&text);

    Ok(text)
}

/// 用系统文本编辑器新建无标题文档（不落盘临时文件）。
fn open_text_editor_with_content(text: &str) {
    #[cfg(target_os = "macos")]
    {
        // 转义双引号
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            r#"tell application "TextEdit"
    activate
    make new document with properties {{text:"{}"}}
end tell"#,
            escaped
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        // 剪贴板已有文本，启动 notepad，用户 Ctrl+V
        let _ = std::process::Command::new("notepad").spawn();
    }
    #[cfg(target_os = "linux")]
    {
        // 剪贴板已有文本，启动文本编辑器
        let _ = std::process::Command::new("xdg-open")
            .arg("text://")
            .spawn();
    }
}
```

- [ ] **Step 3: main.rs 注册命令**

在 `crates/desktop/src/main.rs` 的 `invoke_handler` 中，找到 `clipboard_commands::open_file_item,` 之后添加：

```rust
            clipboard_commands::ocr_image,
```

- [ ] **Step 4: 验证编译**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

Expected: PASS（可能有 dead_code warning）

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/Cargo.toml crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): ocr_image 命令（识别+search_text+剪贴板+新建文档）"
```

---

### Task 6: 前端 OCR 按钮（ClipboardItem）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`

- [ ] **Step 1: 添加 ScanText import + OCR 状态 + handler**

在 import 行添加 `ScanText`：

```typescript
import { Star, Mic, Type, Image as ImageIcon, FileText, Trash2, Download, FolderOpen, Copy, ScanText, Loader2, Check } from "lucide-react";
```

在组件函数体内（deletePending 状态附近）添加：

```typescript
  const [ocrLoading, setOcrLoading] = useState(false);
  const [ocrDone, setOcrDone] = useState(false);
```

添加 handler：

```typescript
  const handleOcr = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (ocrLoading) return;
    setOcrLoading(true);
    try {
      await invoke("ocr_image", { id: item.id });
      setOcrLoading(false);
      setOcrDone(true);
      setTimeout(() => setOcrDone(false), 1000);
    } catch (e) {
      setOcrLoading(false);
      const msg = String(e);
      if (msg.includes("未识别到文本")) {
        setOcrDone(true);
        setTimeout(() => setOcrDone(false), 1000);
      } else {
        console.error(e);
      }
    }
  };
```

- [ ] **Step 2: 在操作按钮区域添加 OCR 按钮**

找到图片保存按钮（`{item.item_type === "image" && (`）之后，文件打开按钮之前，添加 OCR 按钮：

```tsx
        {item.item_type === "image" && (
          <button
            className={cn(
              "p-0.5 transition-opacity",
              ocrLoading || ocrDone
                ? "opacity-100"
                : "opacity-0 group-hover:opacity-60 hover:!opacity-100",
            )}
            onClick={handleOcr}
            disabled={ocrLoading}
            title="OCR 识别"
          >
            {ocrLoading ? (
              <Loader2 className="w-3.5 h-3.5 text-stone-500 animate-spin" />
            ) : ocrDone ? (
              <Check className="w-3.5 h-3.5 text-emerald-600" />
            ) : (
              <ScanText className="w-3.5 h-3.5 text-stone-500 hover:text-stone-800" />
            )}
          </button>
        )}
```

- [ ] **Step 3: 构建前端**

```bash
cd crates/desktop/frontend && npm run build 2>&1 | tail -5
```

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/ crates/desktop/dist/
git commit -m "feat(clipboard): 剪贴板浮窗 OCR 按钮（ScanText + 三态）"
```

---

### Task 7: 前端 OCR 按钮（ClipboardPanel）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx`

- [ ] **Step 1: 在 ClipboardRow 子组件添加 OCR 能力**

在 import 添加 `ScanText, Loader2, Check`：

```typescript
import {
  Star, Mic, Type, Image as ImageIcon, FileText,
  LayoutGrid, Search, Trash2, Copy, Download, FolderOpen,
  ScanText, Loader2, Check,
} from "lucide-react";
```

在 ClipboardRow 函数体内（deletePending 状态附近）添加：

```typescript
  const [ocrLoading, setOcrLoading] = useState(false);
  const [ocrDone, setOcrDone] = useState(false);
```

添加 handler（复用 ClipboardItem 逻辑）：

```typescript
  const handleOcr = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (ocrLoading) return;
    setOcrLoading(true);
    try {
      await invoke("ocr_image", { id: item.id });
      setOcrLoading(false);
      setOcrDone(true);
      setTimeout(() => setOcrDone(false), 1000);
    } catch (e) {
      setOcrLoading(false);
      const msg = String(e);
      if (msg.includes("未识别到文本")) {
        setOcrDone(true);
        setTimeout(() => setOcrDone(false), 1000);
      } else {
        showToast("OCR 失败：" + e);
      }
    }
  };
```

- [ ] **Step 2: 在行操作区域添加 OCR 按钮**

找到图片保存按钮（`{item.item_type === "image" && (` 块）之后，文件打开按钮之前，添加：

```tsx
        {item.item_type === "image" && (
          <button
            className={cn(
              "p-1 rounded transition-opacity",
              ocrLoading || ocrDone
                ? "opacity-100"
                : "opacity-0 group-hover:opacity-50 hover:!opacity-100",
            )}
            onClick={handleOcr}
            disabled={ocrLoading}
            title="OCR 识别"
          >
            {ocrLoading ? (
              <Loader2 className="w-3.5 h-3.5 text-stone-500 animate-spin" />
            ) : ocrDone ? (
              <Check className="w-3.5 h-3.5 text-emerald-600" />
            ) : (
              <ScanText className="w-3.5 h-3.5 text-stone-500 hover:text-stone-800" />
            )}
          </button>
        )}
```

- [ ] **Step 3: 构建前端**

```bash
cd crates/desktop/frontend && npm run build 2>&1 | tail -5
```

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/ crates/desktop/dist/
git commit -m "feat(clipboard): 管理页 OCR 按钮（ClipboardRow + 三态）"
```

---

### Task 8: load_config_key 辅助函数（如不存在）

**Files:**
- Modify: `crates/infra/src/db.rs`

- [ ] **Step 1: 检查 load_config_key 是否已存在**

```bash
grep -n "pub fn load_config_key" crates/infra/src/db.rs
```

如果已存在，跳过此 Task。如果不存在：

- [ ] **Step 2: 添加 load_config_key 函数**

在 `with_db` 函数附近添加：

```rust
/// 读取 app_config 表中某个 key 的值。
pub fn load_config_key(key: &str) -> Option<String> {
    with_db(|conn| {
        conn.query_row(
            "SELECT value FROM app_config WHERE key = ?",
            params![key],
            |row| row.get::<_, String>(0),
        )
    })
    .ok()
}
```

- [ ] **Step 3: 验证编译**

```bash
cargo build -p octopus-infra 2>&1 | tail -3
```

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(infra): load_config_key 辅助函数"
```

---

### Task 9: 端到端验证

- [ ] **Step 1: 完整构建**

```bash
cd crates/desktop/frontend && npm run build
cd .. && cargo build --features embedded 2>&1 | tail -5
```

Expected: 全部 PASS

- [ ] **Step 2: 运行应用，复制一张含文字的截图**

```bash
./run-octopus.sh
```

然后：
1. Cmd+Shift+4 截一张含文字的截图
2. 打开剪贴板浮窗（Alt+V）
3. 找到图片条目，点击 OCR 按钮
4. 观察：按钮 spin → ✓ → TextEdit 新建文档弹出含识别文本

- [ ] **Step 3: 验证 search_text 可搜索**

回到剪贴板浮窗，在搜索框输入 OCR 文本中的关键词 → 图片条目应出现在搜索结果中。

- [ ] **Step 4: 验证管理页 OCR**

打开 Settings → 剪贴板 tab → 对图片条目点 OCR 按钮 → 同样行为。

- [ ] **Step 5: 最终 Commit（如有修复）**

```bash
git add -A
git commit -m "feat(ocr): 端到端验证通过"
```

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1 架构（crate 结构） | Task 1 |
| §1.3 engine.rs 接口 | Task 2 |
| §1.4 model.rs | Task 1 |
| §2 模型管理（models 表 + app_config） | Task 4 |
| §3 OCR 触发流程 | Task 5 |
| §3.2 结果处理（search_text + 剪贴板 + 新建文档） | Task 5 |
| §4 前端集成（按钮 + 状态机） | Task 6 + 7 |
| §5 数据流（update_search_text） | Task 3 |
| §5.2 models seed | Task 4 |
| §5.3 app_config seed | Task 4 |
| §6 错误处理（空文本/模型未就绪） | Task 5 + 6/7 |
| §7 依赖变更（ocr-rs） | Task 1 |
| load_config_key（engine 依赖） | Task 8 |

---

## 实施偏差与补充记录

### 偏差 1：图片存储迁移影响（Task 6 变更）

原计划 OCR 从文件系统读取 PNG，实施时图片存储已迁移到 DB BLOB：
- `ocr_image` 从 `image_data` 表读 WebP BLOB（`store::get_image_blob`），不再读 `clipboard_images/`
- `save_image_item` 从 DB 读 WebP BLOB → 按目标格式转码（WebP 直接写 / PNG+JPEG 解码再编码）
- `OcrEngine::recognize` 改用 `image::load_from_memory`（自动检测格式）

### 偏差 2：ocr-rs API（Task 2）

实际 API 签名：
- `OcrEngine::new(det_path: impl AsRef<Path>, rec_path, charset_path, config: Option<OcrEngineConfig>)` → `OcrResult<Self>`
- `recognize(&self, image: &DynamicImage)` → `OcrResult<Vec<OcrResult_>>`
- `OcrResult_` 有 `.text: String` 和 `.confidence: f32`
- MNN build script 从 GitHub 下载预编译库，release 构建需手动放置

### 偏差 3：osascript 静默（Task 5）

osascript `spawn()` 后 stdout/stderr 会打印「document 未命名」到控制台。修复：`.stdout(Stdio::null()).stderr(Stdio::null())`。

### 偏差 4：Task 8 已存在

`load_config_key` 在 `crates/infra/src/db.rs:406` 已存在，返回 `Result<Option<String>>`。无需新增，Task 8 跳过。

### 偏差 5：DB seed 手动插入

seed 写在 db.sql 中（幂等 `INSERT OR IGNORE`），但已运行的 DB 不会重新执行 db.sql。需手动 `sqlite3` 插入 OCR models + app_config seed。


---

## 来自原文件 `2026-06-28-polish-global-shortcut.md`

# 全局立即润色快捷键（polish_global_shortcut）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增全局快捷键 `polish_global_shortcut`（默认 `CmdOrCtrl+Shift+S`），任意应用聚焦时对当前识别结果立即润色（show 窗口不聚焦），复刻 `edit_global_shortcut` 模式。

**Architecture:** config 字段 + result_window handler（show+emit，不 set_focus）+ 前端 listen 复用 `polishNow`（从 polish-now 按钮抽出）+ settings 热重载 + 设置 UI 行。纯复刻 edit_global，零新机制。

**Tech Stack:** Rust（tauri-plugin-global-shortcut）/ TypeScript（React + Tauri event）/ SQLite app_config。

**关联 spec:** [2026-06-28-polish-global-shortcut-design.md](../specs/2026-06-28-polish-global-shortcut-design.md)

---

## File Structure

| 文件 | 责任 | 改动 |
|------|------|------|
| `crates/infra/src/config.rs` | AppConfig 字段定义 | +`polish_global_shortcut` 字段 + default fn + Default impl + 单测 |
| `crates/infra/src/db.sql` | app_config seed | +seed 行 |
| `crates/infra/src/db.rs` | DB load/save | load +分支 / save +字段（26→27）|
| `crates/desktop/src/result_window.rs` | 窗口管理 + 全局键 handler | +`trigger_global_polish` +`register_polish_global_shortcut` |
| `crates/desktop/src/main.rs` | setup 注册 | +注册调用 |
| `crates/desktop/src/settings_commands.rs` | set_config 热重载 + 校验 | apply_config_value +分支 / set_config +热重载块（old_polish_global）|
| `crates/desktop/frontend/src/pages/Result/index.tsx` | 结果窗前端 | 抽 `polishNow` + 按钮 onClick + listen useEffect |
| `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` | 设置页 | 快捷键卡片 +「立即润色」行 |
| `docs/architecture.md` | 架构文档 | 同步卡片清单 + handler 描述 |

---

## Task 1: 配置层 `polish_global_shortcut` 字段

**Files:**
- Modify: `crates/infra/src/config.rs`（字段 L153-154 区、default fn L227-229 区、Default impl L271 区、单测 L320 区）
- Modify: `crates/infra/src/db.sql`（seed L188 区）
- Modify: `crates/infra/src/db.rs`（load L324 区、save L369/393 区）

- [x] **Step 1.1: config.rs 加字段定义**

在 `edit_global_shortcut` 字段定义之后（L154 `pub edit_global_shortcut: String,` 之后）插入：

```rust
    /// 全局立即润色快捷键（跨应用，show 结果窗不聚焦 + 触发 polish_now）。
    /// 默认 CmdOrCtrl+Shift+S。
    #[serde(default = "default_polish_global_shortcut")]
    pub polish_global_shortcut: String,
```

- [x] **Step 1.2: config.rs 加 default 函数**

在 `default_edit_global_shortcut`（L227-229）之后插入：

```rust
fn default_polish_global_shortcut() -> String {
    "CmdOrCtrl+Shift+S".into()
}
```

- [x] **Step 1.3: config.rs Default impl 加初始化**

在 Default impl 的 `edit_global_shortcut: default_edit_global_shortcut(),`（L271）之后插入：

```rust
            polish_global_shortcut: default_polish_global_shortcut(),
```

- [x] **Step 1.4: config.rs 单测加断言**

在单测 `assert_eq!(cfg.edit_global_shortcut, "CmdOrCtrl+Shift+E");`（L320）之后插入：

```rust
        assert_eq!(cfg.polish_global_shortcut, "CmdOrCtrl+Shift+S");
```

- [x] **Step 1.5: db.sql seed 加行**

在 `edit_global_shortcut` seed 行（L188）之后插入（注意对齐 + category 吃列 DEFAULT='setting'）：

```sql
    ('polish_global_shortcut',   'CmdOrCtrl+Shift+S',                    '全局立即润色快捷键（跨应用 show 结果窗不聚焦 + 触发 polish_now）'),
```

- [x] **Step 1.6: db.rs load 加分支**

在 `load_app_config_at` 的 `"edit_global_shortcut" => cfg.edit_global_shortcut = value,`（L324）之后插入：

```rust
            "polish_global_shortcut" => cfg.polish_global_shortcut = value,
```

- [x] **Step 1.7: db.rs save 加字段**

`save_app_config_at`：
- 数组长度 `let fields: [(&str, String); 26]` → `27`
- 在 `("edit_global_shortcut", cfg.edit_global_shortcut.clone()),`（L393）之后插入：

```rust
        ("polish_global_shortcut", cfg.polish_global_shortcut.clone()),
```

- [x] **Step 1.8: 验证 config 编译 + 单测**

Run: `cargo test -p octopus-infra config::tests -- --nocapture`（或含默认值断言的测试名）
Expected: PASS，含 `polish_global_shortcut == "CmdOrCtrl+Shift+S"` 断言通过。

- [x] **Step 1.9: Commit**

```bash
git add crates/infra/src/config.rs crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(infra): polish_global_shortcut 配置字段 + db load/save（27 字段）"
```

---

## Task 2: 后端 handler + 注册

**Files:**
- Modify: `crates/desktop/src/result_window.rs`（L180 `register_edit_global_shortcut` 之后）
- Modify: `crates/desktop/src/main.rs`（L389 注册块之后）

- [x] **Step 2.1: result_window.rs 加 trigger_global_polish + register**

在 `register_edit_global_shortcut` 函数（L162-180）之后插入。**关键区别：trigger 只 `show` 不 `set_focus`**（润色不需窗口接收键盘）：

```rust
/// 全局立即润色快捷键被按下：show 结果窗（不 set_focus，润色不需窗口聚焦接收键盘）
/// 并通知前端触发 polish_now。前端 polishNow 内部判空（无结果静默）+ polishLoading
/// 门控（幂等）。与 trigger_global_edit 的区别仅在此处不 set_focus。
pub fn trigger_global_polish(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.emit("global-polish-trigger", ());
    }
}

/// 注册全局立即润色快捷键。与 register_edit_global_shortcut 的区别：handler 调
/// trigger_global_polish。set_config 热重载时复用此函数。
pub fn register_polish_global_shortcut(
    app: &tauri::AppHandle,
    shortcut_str: &str,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
    let shortcut: Shortcut = shortcut_str
        .parse()
        .map_err(|e| format!("Failed to parse shortcut '{}': {}", shortcut_str, e))?;
    let app_handle = app.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_ah, _scut, event| {
            if event.state() == ShortcutState::Pressed {
                trigger_global_polish(&app_handle);
            }
        })
        .map_err(|e| format!("Failed to register shortcut '{}': {}", shortcut_str, e))?;
    debug!("Registered global polish shortcut: {}", shortcut_str);
    Ok(())
}
```

- [x] **Step 2.2: main.rs setup 加注册**

在 `register_edit_global_shortcut` 注册块（L386-389）之后插入：

```rust
            // 6.2 Register global polish shortcut（跨应用 show 结果窗 + 立即润色）
            if let Err(e) = result_window::register_polish_global_shortcut(app.handle(), &config.polish_global_shortcut) {
                log::error!("Failed to register global polish shortcut: {}", e);
            }
```

- [x] **Step 2.3: 验证 desktop 编译**

Run: `cargo check -p octopus-desktop`
Expected: 0 error（可能有 pre-existing dead_code warning，无关）。

- [x] **Step 2.4: Commit**

```bash
git add crates/desktop/src/result_window.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): trigger_global_polish + register（show 不聚焦）+ main 注册"
```

---

## Task 3: 热重载 + 校验

**Files:**
- Modify: `crates/desktop/src/settings_commands.rs`（L87-89 old 拆分、L107-118 热重载块、L245-247 apply 分支）

- [x] **Step 3.1: set_config old 拆分加 old_polish_global**

L87-89：
```rust
    let (old_asr_sc, old_clipboard_sc, old_edit_global, mut cfg) = {
        let g = rc.read().unwrap();
        (g.asr_shortcut.clone(), g.clipboard_shortcut.clone(), g.edit_global_shortcut.clone(), g.clone())
    };
```
改为（加 `old_polish_global`）：
```rust
    let (old_asr_sc, old_clipboard_sc, old_edit_global, old_polish_global, mut cfg) = {
        let g = rc.read().unwrap();
        (g.asr_shortcut.clone(), g.clipboard_shortcut.clone(), g.edit_global_shortcut.clone(), g.polish_global_shortcut.clone(), g.clone())
    };
```

- [x] **Step 3.2: set_config 加 polish_global 热重载块**

在 `edit_global_shortcut` 热重载块（L107-118）之后、`clipboard_shortcut` 块（L120）之前插入：

```rust
    // polish_global_shortcut 热重载：注册成功后才持久化（同 asr/edit_global 审查 Issue 3）。
    if key == "polish_global_shortcut" && cfg.polish_global_shortcut != old_polish_global {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        if let Ok(old) = old_polish_global.parse::<tauri_plugin_global_shortcut::Shortcut>() {
            let _ = app_handle.global_shortcut().unregister(old);
        }
        if let Err(e) = crate::result_window::register_polish_global_shortcut(&app_handle, &cfg.polish_global_shortcut) {
            let _ = crate::result_window::register_polish_global_shortcut(&app_handle, &old_polish_global);
            return Err(format!("快捷键注册失败，配置未更改: {}", e));
        }
    }
```

- [x] **Step 3.3: apply_config_value 加 polish_global 分支**

在 `"edit_global_shortcut" =>` 分支（L245-247）之后插入：

```rust
        "polish_global_shortcut" => {
            cfg.polish_global_shortcut = value.as_str().ok_or("polish_global_shortcut 需要字符串")?.to_string();
        }
```

- [x] **Step 3.4: 验证 desktop 编译 + 单测**

Run: `cargo test -p octopus-desktop settings_commands`
Expected: PASS（既有 apply_config_value 单测不受影响；新分支字符串校验同 edit_global）。

- [x] **Step 3.5: Commit**

```bash
git add crates/desktop/src/settings_commands.rs
git commit -m "feat(desktop): polish_global_shortcut 热重载 + apply_config_value 分支"
```

---

## Task 4: 前端

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`（polish-now 按钮 onClick L348-352、新增 polishNow useCallback + listen useEffect）
- Modify: `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx`（快捷键卡片 L154-156 语音编辑行后）

- [x] **Step 4.1: Result/index.tsx 抽 polishNow + 按钮 onClick 复用**

把 polish-now 按钮（L348-352）内联的 onClick 逻辑抽成 `polishNow` useCallback（加 polishLoading 门控 + trim 判空）。在 `toggleEdit` 声明区附近（global-edit-toggle useEffect 之前）加：

```ts
  // 立即润色：工具栏按钮 + 全局 polish_global_shortcut 共用。
  // polishLoading 门控（幂等，与按钮 disabled 一致）+ 空文本判空（无结果静默）。
  const polishNow = useCallback(async () => {
    if (polishLoading) return;
    if (!displayedRef.current.trim()) return;
    setPolishLoading(true);
    try { await invoke("polish_now"); showToast("润色中…"); }
    catch (e) { setPolishLoading(false); showToast("润色失败：" + e); }
  }, [polishLoading, showToast]);
```

polish-now 按钮 onClick 改为 `onClick: polishNow`（去掉内联 async）。

- [x] **Step 4.2: Result/index.tsx 加 global-polish-trigger listen**

在 `global-edit-toggle` useEffect（L254-262）之后加独立 useEffect（规避 TDZ，同 global-edit-toggle）：

```ts
  // 全局立即润色快捷键（polish_global_shortcut）：后端 show 结果窗（不聚焦）后 emit 此事件，
  // 复用 polishNow——空文本静默、进行中幂等，与工具栏「立即润色」按钮同语义。
  // 独立 useEffect（同 global-edit-toggle）：polishNow 在此声明，前置使用触发 TS2448。
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen("global-polish-trigger", () => polishNow()).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, [polishNow]);
```

- [x] **Step 4.3: GeneralPanel.tsx 加「立即润色」行**

在「语音编辑」行（L154-156）之后插入（快捷键卡片内，`</Card>` 之前）：

```tsx
        <Row label="立即润色" effect="立即" hint="对当前识别结果立即润色">
          <ShortcutButton shortcut={cfg.polish_global_shortcut as string} capturing={capturingKey === "polish_global_shortcut"} onClick={() => startShortcutCapture("polish_global_shortcut")} />
        </Row>
```

- [x] **Step 4.4: 验证前端 build**

Run: `npm --prefix crates/desktop/frontend run build`
Expected: tsc + vite 通过，新 bundle 生成（含 polishNow + listen + 立即润色行）。

- [x] **Step 4.5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Result/index.tsx crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx crates/desktop/dist
git commit -m "feat(desktop): 前端 polishNow 抽函数 + global-polish-trigger listen + 设置页立即润色行"
```

---

## Task 5: 文档同步 + 全量验证

**Files:**
- Modify: `docs/architecture.md`（L152 result_window 描述、L298 设置卡片清单 + handler 描述）
- Modify: 本 plan（checkbox 全勾）

- [x] **Step 5.1: architecture.md result_window 描述加全局润色入口**

L152 `result_window` 行的工具栏/编辑入口描述里，在全局 `edit_global_shortcut` 之后补全局润色入口：

> + 全局 `polish_global_shortcut` 默认 CmdOrCtrl+Shift+S（任意应用聚焦时 show 结果窗**不聚焦** + 触发 `polish_now` 立即润色，复用前端 `polishNow`：空文本静默、polishLoading 幂等）

- [x] **Step 5.2: architecture.md 设置卡片清单 + handler 描述**

L298：
- 快捷键卡片清单「语音识别/语音编辑/剪贴板浮窗」→「语音识别/语音编辑/立即润色/剪贴板浮窗」
- `set_config` 热重载快捷键列表 `asr_shortcut / clipboard_shortcut / edit_global_shortcut` → 加 `/ polish_global_shortcut`；handler 描述补 `register_polish_global_shortcut`（handler 调 `trigger_global_polish`：show 结果窗不聚焦 + emit `global-polish-trigger` → 前端 `polishNow`）；save 字段 `26 字段` → `27 字段`。

- [x] **Step 5.3: 全量编译 + 测试**

Run: `cargo check -p octopus-desktop -p octopus-infra && cargo test -p octopus-infra -p octopus-desktop`
Expected: 0 error，单测全绿。

- [x] **Step 5.4: 前端最终 build**

Run: `npm --prefix crates/desktop/frontend run build`
Expected: 通过。

- [x] **Step 5.5: 本 plan checkbox 全勾 + Commit 文档**

```bash
git add docs/architecture.md docs/superpowers/plans/2026-06-28-polish-global-shortcut.md
git commit -m "docs: polish_global_shortcut 同步 architecture + plan checkbox"
```

---

## 验证清单（e2e，待用户桌面环境确认）

1. 按默认 `CmdOrCtrl+Shift+S`：结果窗 show（不抢焦点）+ 当前识别结果立即润色（toast「润色中…」→ 润色文本）。
2. 无识别结果时按：结果窗 show（透明）但不润色（前端判空）。
3. 润色进行中再按：幂等忽略（polishLoading 门控）。
4. 结果窗当前隐藏时按：show 后润色，`update-result` 显示润色文本。
5. 设置 → 快捷键 → 立即润色：键盘捕获改键，热重载即时生效 + 设置页显示新值（DB 持久化）；冲突键报错恢复。
6. 重启应用：配置持久化，全局润色键仍生效（验证 DB 存取）。
7. 不抢焦点验证：在别的工作应用输入时按润色键，当前应用键盘焦点不丢失。

## 不改动

- 工具栏「立即润色」按钮功能（仅 onClick 改 polishNow，行为零差异）。
- `polish_now` 后端命令、`Command::PolishNow`、coordinator 润色逻辑。
- `polish_mode`（自动润色）独立不受影响。


---

## 来自原文件 `2026-06-28-screenshot.md`

# 屏幕截图功能实施计划（一期）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 实现主屏截图：快捷键/托盘触发 → 全屏遮罩 → 鼠标框选 + 8 手柄调整 → Enter 确认 → 进剪贴板历史

**Architecture:** 独立 crate `octopus-capx`（封装 xcap 截图引擎）+ Tauri 全屏透明窗口 + React Canvas 选区 UI。截图结果手动写入剪贴板历史（绕过 watcher）。

**Tech Stack:** Rust + xcap 0.9.6（本地路径引用）+ image 0.25 + Tauri + React Canvas

**Spec:** `docs/superpowers/specs/2026-06-28-screenshot-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/capx/Cargo.toml` | Create | crate 清单 |
| `crates/capx/src/lib.rs` | Create | 模块入口 |
| `crates/capx/src/capture.rs` | Create | 截全屏 + 裁剪选区 |
| `Cargo.toml` | Modify | workspace members 加 capx |
| `crates/clipboard/src/handle.rs` | Modify | 新增 write_image 方法 |
| `crates/infra/src/config.rs` | Modify | AppConfig 新增 screenshot_shortcut |
| `crates/infra/src/db.rs` | Modify | save/load_app_config 补 screenshot_shortcut |
| `crates/infra/src/db.sql` | Modify | app_config seed screenshot_shortcut |
| `crates/desktop/Cargo.toml` | Modify | 新增 octopus-capx 依赖 |
| `crates/desktop/src/screenshot_commands.rs` | Create | start/confirm/cancel 命令 |
| `crates/desktop/src/main.rs` | Modify | 注册命令 + 快捷键 + 托盘菜单 |
| `crates/desktop/src/settings_commands.rs` | Modify | apply_config_value + 热重载 |
| `crates/desktop/src/tray.rs` | Modify | 托盘菜单加「截图」 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | Create | 选区 Canvas UI |
| `crates/desktop/frontend/src/main.tsx` | Modify | 路由加 screenshot 页面 |

---

### Task 1: octopus-capx crate

**Files:**
- Create: `crates/capx/Cargo.toml`
- Create: `crates/capx/src/lib.rs`
- Create: `crates/capx/src/capture.rs`
- Modify: `Cargo.toml`（workspace root）

- [ ] **Step 1: 创建 crate**

```bash
mkdir -p crates/capx/src
```

`crates/capx/Cargo.toml`:
```toml
[package]
name = "octopus-capx"
version = "0.1.0"
edition = "2021"

[dependencies]
xcap = { path = "../../xcap" }
image = "0.25"
anyhow = "1"
log = "0.4"
```

`crates/capx/src/lib.rs`:
```rust
pub mod capture;
```

- [ ] **Step 2: 实现 capture.rs**

```rust
use anyhow::{Context, Result};
use xcap::Monitor;

pub struct ScreenCapture {
    pub rgba_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 截取主显示器全屏（返回 RGBA 像素 + 尺寸）。
/// 主显示器 = 包含鼠标当前位置的显示器。
pub fn capture_full_screen() -> Result<ScreenCapture> {
    let monitors = Monitor::all().context("Failed to list monitors")?;
    let monitor = monitors
        .into_iter()
        .next()
        .context("No monitor found")?;

    let img = monitor
        .capture_image()
        .context("Failed to capture screen")?;

    let width = img.width();
    let height = img.height();
    let rgba_bytes = img.into_raw();

    log::info!(
        "Screen captured: {}x{} ({}KB RGBA)",
        width,
        height,
        rgba_bytes.len() / 1024
    );

    Ok(ScreenCapture {
        rgba_bytes,
        width,
        height,
    })
}

/// 从全屏 RGBA 中裁剪矩形区域，返回 PNG bytes。
/// 坐标为物理像素。
pub fn crop_region(
    full: &ScreenCapture,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<Vec<u8>> {
    let img = ::image::RgbaImage::from_raw(full.width, full.height, full.rgba_bytes.clone())
        .context("Failed to create RgbaImage from full screen")?;

    // clamp
    let x = x.min(full.width.saturating_sub(1));
    let y = y.min(full.height.saturating_sub(1));
    let w = w.min(full.width - x);
    let h = h.min(full.height - y);

    let cropped = ::image::imageops::crop_imm(&img, x, y, w, h).to_image();

    let mut png_bytes = Vec::new();
    cropped
        .write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
        .context("Failed to encode cropped PNG")?;

    Ok(png_bytes)
}
```

- [ ] **Step 3: workspace Cargo.toml 加 member**

在 members 列表末尾加 `"crates/capx"`。

- [ ] **Step 4: 验证编译**

```bash
cargo build -p octopus-capx 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add crates/capx/ Cargo.toml
git commit -m "feat(capx): octopus-capx crate（xcap 截全屏 + 裁剪选区）"
```

---

### Task 2: ClipboardHandle 新增 write_image

**Files:**
- Modify: `crates/clipboard/src/handle.rs`

- [ ] **Step 1: 添加 write_image 方法**

在 `write_text` 方法之后添加：

```rust
/// 写入 PNG 图片到剪贴板（设置 suppress flag）。
pub fn write_image(&self, png_bytes: &[u8]) -> Result<()> {
    self.suppress_flag.store(true, Ordering::SeqCst);
    let ctx = self.ctx.lock().unwrap();
    let img = clipboard_rs::common::RustImageData::from_bytes(png_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to create RustImageData: {}", e))?;
    ctx.set_image(img)
        .map_err(|e| anyhow::anyhow!("Clipboard write image failed: {}", e))?;
    Ok(())
}
```

- [ ] **Step 2: 验证编译 + 测试**

```bash
cargo build -p octopus-clipboard 2>&1 | tail -3
cargo test -p octopus-clipboard 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
git add crates/clipboard/src/handle.rs
git commit -m "feat(clipboard): ClipboardHandle::write_image"
```

---

### Task 3: AppConfig + DB seed（screenshot_shortcut）

**Files:**
- Modify: `crates/infra/src/config.rs`
- Modify: `crates/infra/src/db.rs`
- Modify: `crates/infra/src/db.sql`

- [ ] **Step 1: config.rs 新增字段**

在 `clipboard_max_age_days` 之后添加：

```rust
    /// 截图全局快捷键（Tauri Accelerator 格式）。默认 "Alt+S"。
    #[serde(default = "default_screenshot_shortcut")]
    pub screenshot_shortcut: String,
```

新增默认值函数：
```rust
fn default_screenshot_shortcut() -> String {
    "Alt+S".into()
}
```

Default impl 末尾加：
```rust
            screenshot_shortcut: default_screenshot_shortcut(),
```

- [ ] **Step 2: db.rs save_app_config_at + load_app_config_at 补字段**

save fields 数组 `[(&str, String); 25]` → `[(&str, String); 26]`，末尾加：
```rust
        ("screenshot_shortcut", cfg.screenshot_shortcut.clone()),
```

load match 分支加：
```rust
            "screenshot_shortcut" => cfg.screenshot_shortcut = value,
```

- [ ] **Step 3: db.sql app_config seed 加**

```sql
    ('screenshot_shortcut',       'Alt+S',                                '截图快捷键'),
```

- [ ] **Step 4: settings_commands apply_config_value 加字段**

```rust
        "screenshot_shortcut" => {
            cfg.screenshot_shortcut = value.as_str().ok_or("screenshot_shortcut 需要字符串")?.to_string();
        }
```

- [ ] **Step 5: set_config 热重载**

在 `clipboard_shortcut` 热重载块之后添加 `screenshot_shortcut` 热重载（同模式：unregister 旧 + register 新 + on_shortcut 回调调 start_screenshot）。

- [ ] **Step 6: 验证编译**

```bash
cargo build -p octopus-infra -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 7: 手动 seed DB**

```bash
sqlite3 ~/.octopus/octopus.db "INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES ('screenshot_shortcut', 'Alt+S', '截图快捷键');"
```

- [ ] **Step 8: Commit**

```bash
git add crates/infra/ crates/desktop/src/settings_commands.rs
git commit -m "feat(infra): screenshot_shortcut 配置 + 热重载"
```

---

### Task 4: screenshot_commands.rs（start/confirm/cancel）

**Files:**
- Create: `crates/desktop/src/screenshot_commands.rs`
- Modify: `crates/desktop/Cargo.toml`（加 octopus-capx + octopus-clipboard 依赖）
- Modify: `crates/desktop/src/main.rs`（注册命令 + 快捷键 + 托盘菜单）

- [ ] **Step 1: Cargo.toml 加依赖**

```toml
octopus-capx = { path = "../capx" }
```

- [ ] **Step 2: 实现 screenshot_commands.rs**

```rust
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use octopus_clipboard::ClipboardHandle;
use octopus_clipboard::image;

static SCREENSHOT_DATA: Mutex<Option<octopus_capx::capture::ScreenCapture>> = Mutex::new(None);
const WINDOW_LABEL: &str = "screenshot_window";

#[tauri::command]
pub async fn start_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    // 1. 截全屏
    let capture = octopus_capx::capture::capture_full_screen()
        .map_err(|e| format!("截图失败: {}", e))?;

    // 2. RGBA → PNG base64（前端渲染用）
    let img = ::image::RgbaImage::from_raw(capture.width, capture.height, capture.rgba_bytes.clone())
        .map_err(|e| format!("图像处理失败: {}", e))?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败: {}", e))?;

    let width = capture.width;
    let height = capture.height;

    // 3. 暂存
    *SCREENSHOT_DATA.lock().unwrap() = Some(capture);

    // 4. 创建/重建截图窗口
    if let Some(old) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = old.destroy();
    }

    use tauri::WebviewWindowBuilder;
    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        tauri::WebviewUrl::App("index.html#/screenshot".into()),
    )
    .title("")
    .fullscreen(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .transparent(true)
    .build();

    // 5. 等前端 ready 后 emit 图片数据
    use base64::{Engine, engine::general_purpose};
    let b64 = general_purpose::STANDARD.encode(&png_bytes);
    let ah = app_handle.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = ah.emit("screenshot://ready", serde_json::json!({
            "image": b64,
            "width": width,
            "height": height,
        }));
    });

    Ok(())
}

#[tauri::command]
pub async fn confirm_screenshot(
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    app_handle: tauri::AppHandle,
    handle: State<'_, std::sync::Arc<ClipboardHandle>>,
) -> Result<(), String> {
    // 1. 取全屏数据
    let full = SCREENSHOT_DATA.lock().unwrap().take()
        .ok_or("无截图数据")?;

    // 2. 裁剪选区
    let png_bytes = octopus_capx::capture::crop_region(&full, x, y, w, h)
        .map_err(|e| format!("裁剪失败: {}", e))?;

    // 3. 编码去重 → WebP → DB
    let (png_for_hash, hash) = image::encode_and_hash(
        // crop_region 返回的已经是 PNG，需要先解码为 RGBA 再走 encode_and_hash
        // 实际上 crop_region 返回 PNG，可以直接算 hash
        &{
            // 重新编码 RGBA → PNG for hash consistency
            let img = ::image::load_from_memory(&png_bytes)
                .map_err(|e| format!("decode failed: {}", e))?;
            let rgba = img.to_rgba8();
            let w = rgba.width();
            let h = rgba.height();
            rgba.into_raw().as_slice().to_vec()
            // encode_and_hash 接受 RGBA，这里需要适配
            // 实际上 encode_and_hash 签名是 (rgba: &[u8], width, height)
        },
        w, h,
    ).map_err(|e| format!("编码失败: {}", e))?;
    // 注：crop_region 返回 PNG bytes，不是 RGBA。
    // 需要在 capx 中额外暴露 crop_region_rgba 或在这里解码 PNG → RGBA
    // 简化方案：直接用 png_bytes 算 hash（SHA-256 of PNG bytes）

    // 4. encode_to_webp + insert_image_data + insert_clipboard_item
    // 5. write_image to clipboard (suppress flag)
    // 6. 关窗口

    Ok(())
}

#[tauri::command]
pub async fn cancel_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    *SCREENSHOT_DATA.lock().unwrap() = None;
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.destroy();
    }
    Ok(())
}
```

**注意**：confirm_screenshot 中的去重/WebP/DB 逻辑需要适配——crop_region 返回 PNG bytes，而 `encode_and_hash` 接受 RGBA。需要在 capx 中改为返回 RGBA，或新增 `crop_region_png` + 直接对 PNG bytes 算 SHA-256。

简化方案：在 capx 中新增 `crop_region_png` 返回 `Vec<u8>` PNG，confirm 中直接对 PNG bytes 算 SHA-256（绕过 encode_and_hash），然后解码 PNG → encode_to_webp。

- [ ] **Step 3: main.rs 注册命令 + 快捷键**

mod 声明加 `mod screenshot_commands;`

invoke_handler 加：
```rust
            screenshot_commands::start_screenshot,
            screenshot_commands::confirm_screenshot,
            screenshot_commands::cancel_screenshot,
```

setup 中加截图快捷键注册（从 config 读 `screenshot_shortcut`）。

托盘菜单加「截图」项（tray.rs）。

- [ ] **Step 4: 验证编译**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/
git commit -m "feat(desktop): screenshot 命令 + 快捷键 + 托盘菜单"
```

---

### Task 5: 前端选区 Canvas UI

**Files:**
- Create: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`
- Modify: `crates/desktop/frontend/src/main.tsx`（路由加 screenshot）

- [ ] **Step 1: main.tsx 路由**

```typescript
// 路由判断：URL hash = #/screenshot 时渲染 Screenshot 组件
```

- [ ] **Step 2: 实现 Screenshot/index.tsx**

核心功能：
- 监听 `screenshot://ready` 事件 → 拿到全屏 PNG base64
- Canvas 渲染全屏图 + 暗遮罩
- 鼠标拖拽框选（mousedown/mousemove/mouseup）
- 8 手柄 resize + 选区 move
- devicePixelRatio 换算
- Enter 确认 → invoke confirm_screenshot
- ESC/右键 → invoke cancel_screenshot
- 选区右下角尺寸标注

组件结构（伪代码）：
```tsx
export default function Screenshot() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [bgImage, setBgImage] = useState<HTMLImageElement | null>(null);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [mode, setMode] = useState<"idle" | "selecting" | "move" | "resize">("idle");
  const [resizeHandle, setResizeHandle] = useState<string | null>(null);
  const dpr = window.devicePixelRatio || 1;

  // 1. listen screenshot://ready → setBgImage
  // 2. Canvas 绘制：bgImage + 遮罩（clearRect 选区）+ 选区边框 + 8 手柄 + 尺寸标注
  // 3. mousedown：判断命中手柄/选区/外部 → 设 mode
  // 4. mousemove：按 mode 更新 selection（归一化 + clamp）
  // 5. mouseup：回 idle/selected
  // 6. keydown：Enter → confirm（dpr 换算），ESC → cancel

  // 全屏 Canvas：position fixed, w/h = window.innerWidth/Height
  // bgImage 按 CSS 像素渲染，confirm 时坐标 × dpr → 物理坐标
}
```

- [ ] **Step 3: 构建前端**

```bash
cd crates/desktop/frontend && npm run build 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/
git commit -m "feat(screenshot): 前端选区 Canvas UI（框选 + 手柄调整 + 尺寸标注）"
```

---

### Task 6: 端到端验证

- [ ] **Step 1: 完整构建**

```bash
cd crates/desktop/frontend && npm run build
cd .. && cargo build --features embedded 2>&1 | tail -5
```

- [ ] **Step 2: 运行应用测试截图**

```bash
./run-octopus.sh
```

验证：
1. 按 Alt+S → 全屏变暗 + 十字准星
2. 鼠标拖拽框选 → 选区高亮 + 尺寸标注
3. 拖拽手柄 → 选区调整
4. 拖拽选区内部 → 平移
5. Enter → 截图进剪贴板浮窗（图片条目）
6. ESC → 取消
7. 托盘菜单「截图」→ 同样触发
8. 设置页「截图」快捷键可改 + 热重载

- [ ] **Step 3: 最终 Commit（如有修复）**

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1 架构（crate 结构） | Task 1 |
| §1.3 capture.rs 接口 | Task 1 |
| §2 截图触发流程 | Task 4 |
| §2.1 选区交互状态机 | Task 5 |
| §2.2 选区调整手柄 | Task 5 |
| §3 前端选区 Canvas | Task 5 |
| §4 数据流（手动写入剪贴板历史） | Task 4（confirm_screenshot）|
| §4.3 截图配置 | Task 3 |
| §5 Tauri 命令 + 窗口 | Task 4 |
| §6 错误处理 | Task 4 + Task 5 |

---

## 实施偏差与补充记录

### 偏差 1：前端拉取模式（替代 emit 延迟）

原设计用 emit + 300ms 延迟发送截图数据给前端，实际 emit 在前端未 ready 时丢失。改为 `get_screenshot_image` 命令——前端 mount 后主动调用，暂存到 `PENDING_IMAGE` 静态变量（与 settings_window 的 `PENDING_PAGE` 同模式）。

### 偏差 2：Monitor::from_point 定位

`Monitor::all().next()` 可能取到错误的显示器。改为 `Monitor::from_point(鼠标位置)` 定位用户当前所在显示器。macOS 用 `core-graphics::CGEvent` 获取鼠标位置。

### 偏差 3：去掉 transparent: true

透明窗口在加载期间闪烁黑色。改为不透明窗口，前端自行渲染全屏 Canvas（黑色 loading 态 → 截图数据就绪后渲染）。

### 偏差 4：base64 从 optional 改为非 optional

screenshot_commands 需要 base64 编码 PNG，将 base64 从 cloud feature 的 optional 依赖改为非 optional。

### 偏差 5：xcap 软链接 + workspace exclude

xcap 声明了 `[workspace]`，导致 octopus workspace 冲突。解决：`exclude = ["xcap"]` + `.gitignore` 排除软链接。

### 偏差 6：macOS 权限

通过 `cargo run` 运行时，屏幕录制权限绑定终端应用（非二进制）。首次截图黑屏 → 授权终端后重启生效。打包 .app 后绑定 octopus 本身。

### 偏差 7：1.1 期多显示器——每屏独立窗口（非拼接）

原设计为「截取所有屏幕拼接为一张全图」，用户澄清为「指定截哪个屏幕」。改为每屏独立窗口：
- `capture_all_monitors()` 截所有显示器
- 每个显示器创建独立 Tauri 窗口（`screenshot_window` / `screenshot_window_N`）
- 窗口坐标用 Tauri `available_monitors()` 逻辑坐标（物理除以 `scale_factor`）
- confirm/cancel 关闭所有 `screenshot_*` 窗口

### 偏差 8：窗口闪烁——延迟显示

窗口创建后立即可见导致白屏闪烁。改为 `visible(false)` + 前端 Canvas 渲染完后调 `show_screenshot_window` 命令显示。
- main.tsx 按 window label 提前设 body 背景为 `rgba(0,0,0,0.5)`
- Loading 态也用 `rgba(0,0,0,0.5)` 和最终遮罩一致
- **TODO**：后续可加窗口过渡动画进一步消除抖动（达到 Xnip 级体验）

### 偏差 9：二期标注工具栏——Canvas clip + 临时合成

二期实现矩形/箭头/文字三种标注工具 + 撤销：
- 标注在选区内绘制，用 `ctx.clip()` 限制到选区矩形
- 矩形/箭头：鼠标拖拽绘制（红色 `#ef4444`），过滤太小的
- 文字：点击弹 textarea，失焦确认
- 撤销：Cmd+Z / 工具栏按钮，删除最后一个标注
- 确认时临时 Canvas 合成标注到截图（坐标 × dpr 转物理像素）→ 替换底图 → 裁剪
- 工具栏是 DOM 元素浮在 Canvas 上（选区下方，空间不够时放上方），白色圆角 + 阴影

### 偏差 10：标注重影 + 消失 + 不能移动 + 分辨率/比例问题（多轮修复）

**问题 1 — 文字重影**：文字标注同时在 Canvas（textDraft）和 DOM（textarea）渲染，两层叠加。修复：Canvas 不画 textDraft，只靠 DOM textarea 显示。

**问题 2 — 文字标注消失**：文字工具激活时点击其他地方，Canvas mousedown 创建新空 textDraft 覆盖旧值。修复：mousedown 开头检查 textDraftRef，有未保存文字先写入 annotations。用 ref（非 state）存最新值避免闭包陷阱。

**问题 3 — 标注框不能移动**：标注选中+移动只在 tool=none 时生效。修复：任何工具状态下优先 hitTestAnnotation，命中后进入拖动模式。

**问题 4 — 截图分辨率低**：合成 Canvas 用 cssW*dpr 而非原图 naturalWidth。修复：临时 Canvas 用原图 naturalWidth/Height（全分辨率），`drawImage(bg, 0, 0)` 1:1 无缩放。

**问题 5 — 标注变小**：合成到原图分辨率时标注的 lineWidth(3px) 和 font(16px) 没缩放。修复：新增 `drawAnnotationScaled` 函数，坐标/线宽/字号/箭头头部全部 × scale（`scale = natW / cssW`）。

### 偏差 11：confirm_screenshot_with_data（前端合成替代后端裁剪）

原设计：前端传坐标 → 后端从 SCREENSHOT_DATA 裁剪。但标注在前端 Canvas 上，后端无法感知。
改为：前端完整合成（原图 + 标注 → 裁剪选区 → base64 PNG）→ 新增 `confirm_screenshot_with_data` 命令接收最终 PNG → 后端跳过裁剪直接入库。

### 偏差 12：工具栏扩展——直线/画笔/序号 + 属性浮窗 + 自定义图标

- 新增 line（直线）、pen（自由曲线，追加点序列）、number（序号，点击递增圆圈数字）三种标注
- 序号：实心彩色圆圈 + 白色加粗数字，圆圈大小可调（16-60），数字字号 = 圆圈 × 0.6
- 标注独立记忆 color + lineWidth/fontSize/circleSize（用 useRef 避免 onBlur 闭包陷阱）
- 工具属性浮窗（ToolPropsPopover）三行→两行：第一行滑轨+当前色圆形指示器，第二行预设色+彩虹调色板
- 三种模式：粗细(1-10) / 字号(10-48) / 圆圈(16-60)
- 工具栏全部换为自定义 SVG 图标（square/straight-line/arrow-line/sketching/text/sequence-note/restore/save/copy/close）
- 保存改为系统保存对话框（`save_screenshot_dialog`，tauri_plugin_dialog）
- 撤销按钮使用 `restore.svg` 图标

### 偏差 13：多显示器窗口崩溃修复

同时创建多个全屏 WebView 导致 macOS WKWebView 进程崩溃（segfault）。改为串行创建（150ms 间隔）+ 错误处理（创建失败跳过该显示器）。

### 偏差 14：e2e 审计修复（7 bug + UX + 性能）

**Bug**：双相同分辨率画面重合（坐标匹配）、自由曲线 points 未平移、文字输入竞态丢失、选区越界、工具激活时标注命中阻碍绘图、丢弃标注 Canvas 残留、文字多行不支持。

**UX**：保存对话框先关窗口、工具栏 clamp、barrier 同步显示 + 3s 超时、Delete 删除标注、序号撤销回退、选择工具按钮、右键行为、双击编辑（ESC 恢复）、选区外点击忽略。

**性能**：Canvas 尺寸仅初始化一次、PNG→JPEG 85%、encode_to_webp_from_image 避免重复解码。

### 偏差 15：modeRef 同步（右键状态机卡死修复）

React 异步闭包读旧 mode 导致右键模拟左键后状态卡死。引入 modeRef（useRef）+ setModeSafe 同步更新 ref。

### 偏差 16：双击编辑文字——ESC 丢文字 + 篡改全局颜色

不立即删除原标注（标记 text:"" 隐藏），ESC 从 editTextOrigRef 恢复。editTextColorRef/editTextFontSizeRef 独立存编辑颜色/字号，不修改全局。

### 偏差 17：空心标注精确命中 + 手柄优先 + 右键简化 + session ID + 聚焦修复

**空心标注精确命中**：新增 `hitTestAnnotationPrecise` 替代 bounding box——矩形检查到四条边距离 ≤8px，椭圆检查到轮廓距离 ≤8px，直线/箭头/画笔用点到线段距离 ≤8px。空心形状内部空白不命中。

**手柄优先检测**：选区手柄 hitTest 提到 mousedown 最前（任何工具状态下可调整选区大小），标注 hitTest 在其之后。

**右键简化**：彻底移除右键模拟左键逻辑，`onContextMenu` 仅 `preventDefault`——取消截图仅通过 ESC 或工具栏取消按钮。

**session ID 窗口 label**：用 `screenshot_{timestamp}_{i}` 替代固定 label，移除 50ms sleep 等待 destroy——新旧窗口 label 不同即使旧窗口未完全销毁也不冲突。

**主显示器聚焦修复**：`show_all_screenshot_windows` 中聚焦的 label 从硬编码 `"screenshot_window"` 改为从窗口列表查找以 `_0` 结尾的 label（匹配 session ID 格式）。


---

## 来自原文件 `2026-06-28-settings-model-selection.md`

# 设置页「模型选择」Card 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在系统设置页「交互」Card 下方新增独立「模型选择」Card，集中 asr_engine / polish_llm / ocr_model 三类模型选择；补 `ocr_model` 进 AppConfig 持久化链路；新增 `list_ocr_models` 数据源。

**Architecture:** 前端 Card 重组（搬运 asr/polish 行 + 新增 ocr 行）+ 后端补漏（ocr_model 纳入 AppConfig load/save）+ 新增 OCR 选项查询（db list_ocr_models → runtime_config build_ocr_options → ConfigResponse）。OCR 因 OnceLock 单例，改后重启生效（不热重载）。

**Tech Stack:** Rust（infra db/config + desktop runtime_config/settings_commands）/ TypeScript（React）/ SQLite models 表 domain='ocr'。

**关联 spec:** [2026-06-28-settings-model-selection-design.md](../specs/2026-06-28-settings-model-selection-design.md)

---

## File Structure

| 文件 | 责任 | 改动 |
|------|------|------|
| `crates/infra/src/config.rs` | AppConfig schema | +`ocr_model` 字段 + default fn + Default impl + 单测 |
| `crates/infra/src/db.rs` | DB load/save + 模型查询 | load +分支 / save +字段（27→28）/ +`OcrModelInfo` +`list_ocr_models` +单测 |
| `crates/desktop/src/runtime_config.rs` | 选项 DTO + 构造 | +`OcrOption` +`build_ocr_options_public` |
| `crates/desktop/src/settings_commands.rs` | get_config / set_config | ConfigResponse +`ocr_models` / get_config 组装 / apply_config_value +`ocr_model` 分支 |
| `crates/desktop/frontend/src/pages/Settings/index.tsx` | ConfigResponse 接口 | +`ocr_models` 字段 |
| `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx` | 设置页 | +「模型选择」Card / 删识别引擎行 / 删润色模型行 / import Layers |
| `docs/architecture.md` | 架构文档 | 同步 Card 清单 + ocr_model 字段 + list_ocr_models |

---

## Task 1: `config.rs` 补 `ocr_model` 字段

**Files:**
- Modify: `crates/infra/src/config.rs`（字段 L176-177 区、default fn L248-250 区、Default impl L285 区、单测 L331 区）

- [x] **Step 1.1: 加字段定义**

在 `clipboard_max_age_days` 字段（L176-177）之后、结构体闭合 `}`（L178）之前插入：

```rust

    /// OCR 模型（当前激活），对应 ~/.octopus/models/ocr/<name>/ 目录名。
    /// 默认 "PP-OCRv6-small"。OCR 引擎 OnceLock 单例缓存，改后重启生效。
    #[serde(default = "default_ocr_model")]
    pub ocr_model: String,
```

- [x] **Step 1.2: 加 default 函数**

在 `default_clipboard_max_age_days`（L248-250）之后插入：

```rust
fn default_ocr_model() -> String {
    "PP-OCRv6-small".into()
}
```

- [x] **Step 1.3: Default impl 加初始化**

在 Default impl 的 `clipboard_max_age_days: default_clipboard_max_age_days(),`（L285）之后、闭合 `}`（L286）之前插入：

```rust
            ocr_model: default_ocr_model(),
```

- [x] **Step 1.4: 单测加断言**

在 `app_config_default_values` 测试的 `assert_eq!(cfg.polish_global_shortcut, "CmdOrCtrl+Shift+S");`（L331）之后插入：

```rust
        assert_eq!(cfg.ocr_model, "PP-OCRv6-small");
```

- [x] **Step 1.5: 验证 config 编译 + 单测**

Run: `cargo test -p octopus-infra config::tests -- --nocapture`
Expected: PASS，含 `ocr_model == "PP-OCRv6-small"` 断言通过。

---

## Task 2: `db.rs` load/save + `list_ocr_models`

**Files:**
- Modify: `crates/infra/src/db.rs`（load L321 区、save L356/370/386 区、LlmModelInfo 区 L650-684、测试区 L1340 区）

- [x] **Step 2.1: load_app_config_at 加 ocr_model 分支**

在 `"polish_llm" => cfg.polish_llm = value,`（L321）之后插入（字符串区分支）：

```rust
            "ocr_model" => cfg.ocr_model = value,
```

- [x] **Step 2.2: save_app_config_at 注释改 28 字段**

L356 注释 `/// 全量写入应用配置（27 字段 ON CONFLICT DO UPDATE）。` → `28 字段`：

```rust
/// 全量写入应用配置（28 字段 ON CONFLICT DO UPDATE）。set_config / yaml 迁移用。
```

- [x] **Step 2.3: save_app_config_at 数组长度 27 → 28**

L370 `let fields: [(&str, String); 27] = [` → `28`：

```rust
    let fields: [(&str, String); 28] = [
```

- [x] **Step 2.4: save_app_config_at 加 ocr_model 字段**

在 `("polish_llm", cfg.polish_llm.clone()),`（L386）之后插入：

```rust
        ("ocr_model", cfg.ocr_model.clone()),
```

- [x] **Step 2.5: 加 OcrModelInfo + list_ocr_models**

在 `list_llm_models`（L682-684）之后插入：

```rust

/// OCR 模型列表项（菜单用，仅含显示字段）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrModelInfo {
    pub model_name: String,
    pub description: String,
}

/// 列出所有启用的 OCR 模型（domain='ocr' AND is_enabled=1）。
fn list_ocr_models_at(conn: &Connection) -> Result<Vec<OcrModelInfo>> {
    let mut stmt = conn.prepare(
        "SELECT model_name, description FROM models
         WHERE domain='ocr' AND is_enabled = 1",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(OcrModelInfo {
            model_name: row.get::<_, String>(0)?,
            description: row.get::<_, String>(1)?,
        })
    })?;
    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

/// 从 DB 列出启用的 OCR 模型（经 with_db，供 Tauri 命令调用）。
pub fn list_ocr_models() -> Result<Vec<OcrModelInfo>> {
    with_db(|conn| list_ocr_models_at(conn))
}
```

- [x] **Step 2.6: 加 list_ocr_models 单测**

在 `list_llm_models_at_empty_when_all_disabled` 测试（L1334-1340）之后插入：

```rust

    #[test]
    fn list_ocr_models_returns_enabled() {
        let conn = open_init();
        let list = list_ocr_models_at(&conn).unwrap();
        // seed 默认 1 条 OCR（PP-OCRv6-small, is_enabled=1）
        assert_eq!(list.len(), 1, "seed 1 条启用 OCR");
        assert_eq!(list[0].model_name, "PP-OCRv6-small");
        assert!(!list[0].description.is_empty(), "description 非空");
    }

    #[test]
    fn list_ocr_models_filters_disabled() {
        let conn = open_init();
        conn.execute("UPDATE models SET is_enabled = 0 WHERE domain='ocr'", []).unwrap();
        let list = list_ocr_models_at(&conn).unwrap();
        assert!(list.is_empty(), "全禁用时返回空");
    }
```

- [x] **Step 2.7: 验证 infra 编译 + 单测**

Run: `cargo test -p octopus-infra -- --nocapture`
Expected: PASS（含 config ocr_model 默认值 + list_ocr_models 两测）。

- [x] **Step 2.8: Commit**

```bash
git add crates/infra/src/config.rs crates/infra/src/db.rs
git commit -m "feat(infra): ocr_model 纳入 AppConfig（28 字段）+ list_ocr_models 查询"
```

---

## Task 3: `runtime_config.rs` 加 `OcrOption` + 构造函数

**Files:**
- Modify: `crates/desktop/src/runtime_config.rs`（LlmOption L152-158 区、build_llm_options_public L198-203 区）

- [x] **Step 3.1: 加 OcrOption 结构**

在 `LlmOption` 结构（L152-158）之后插入：

```rust

/// OCR 模型菜单项（与 LlmOption 同构，current 标记当前选中的 ocr_model）。
/// 与 LLM 的区别：不做「不选择模型」首项——OCR 必须有一个模型。
#[derive(Serialize)]
pub struct OcrOption {
    pub name: String,
    pub label: String,
    pub current: bool,
}
```

- [x] **Step 3.2: 加 build_ocr_options + 公开包装**

在 `build_llm_options_public`（L198-203）之后插入：

```rust

/// 构造 OCR 选项列表（纯逻辑）：DB 启用的 OCR 模型，current 按裸 model_name 标记。
/// 不做「不选择」首项（OCR 必须有一个）。label 优先 description，空则 model_name。
fn build_ocr_options(current: &str, ocrs: Vec<octopus_infra::db::OcrModelInfo>) -> Vec<OcrOption> {
    ocrs.into_iter()
        .map(|m| OcrOption {
            current: m.model_name == current,
            label: if m.description.is_empty() {
                m.model_name.clone()
            } else {
                m.description
            },
            name: m.model_name,
        })
        .collect()
}

/// 公开包装（供 settings_commands 调用）。
pub fn build_ocr_options_public(
    current: &str,
    ocrs: Vec<octopus_infra::db::OcrModelInfo>,
) -> Vec<OcrOption> {
    build_ocr_options(current, ocrs)
}
```

- [x] **Step 3.3: 验证 desktop 编译**

Run: `cargo check -p octopus-desktop`
Expected: 0 error（OcrOption/build_ocr_options_public 暂未使用，可能有 dead_code warning，Task 4 消除）。

---

## Task 4: `settings_commands.rs` ConfigResponse + get_config + apply 分支

**Files:**
- Modify: `crates/desktop/src/settings_commands.rs`（ConfigResponse L19 区、get_config L34-35/54-59 区、apply_config_value L260-262 区）

- [x] **Step 4.1: ConfigResponse 加 ocr_models 字段**

在 `pub llm_models: Vec<crate::runtime_config::LlmOption>,`（L19）之后插入：

```rust
    pub ocr_models: Vec<crate::runtime_config::OcrOption>,
```

- [x] **Step 4.2: get_config 组装 ocr_models**

在 `let llm_models = crate::runtime_config::build_llm_options_public(&g.polish_llm, llms);`（L35）之后插入：

```rust

    let ocrs = octopus_infra::db::list_ocr_models().map_err(|e| e.to_string())?;
    let ocr_models = crate::runtime_config::build_ocr_options_public(&g.ocr_model, ocrs);
```

- [x] **Step 4.3: get_config 返回填 ocr_models**

L52-59 `Ok(ConfigResponse { ... })` 加 `ocr_models,`（在 `llm_models,` 之后）：

```rust
    Ok(ConfigResponse {
        config: config_json,
        asr_engines,
        llm_models,
        ocr_models,
        microphones,
        prompts,
        active_prompt_id,
    })
```

- [x] **Step 4.4: apply_config_value 加 ocr_model 分支**

在 `"polish_global_shortcut" => { ... }`（L260-262）之后插入（裸 model_name，简单字符串校验，照 asr_shortcut 模板）：

```rust
        "ocr_model" => {
            cfg.ocr_model = value.as_str().ok_or("ocr_model 需要字符串")?.to_string();
        }
```

- [x] **Step 4.5: 验证 desktop 编译 + 单测**

Run: `cargo test -p octopus-desktop settings_commands`
Expected: PASS（既有 apply_config_value 单测不受影响）。

- [x] **Step 4.6: Commit**

```bash
git add crates/desktop/src/runtime_config.rs crates/desktop/src/settings_commands.rs
git commit -m "feat(desktop): OcrOption + build_ocr_options + get_config 组装 + apply ocr_model 分支"
```

---

## Task 5: 前端 ConfigResponse 接口 + GeneralPanel 模型选择 Card

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/index.tsx`（ConfigResponse L15 区）
- Modify: `crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx`（import L4、解构 L87、交互 Card 后 L145、删识别引擎行 L162-167、删润色模型行 L189-196）

- [x] **Step 5.1: index.tsx ConfigResponse 加 ocr_models**

在 `llm_models: { name: string; label: string; current: boolean }[];`（L15）之后插入：

```ts
  ocr_models: { name: string; label: string; current: boolean }[];
```

- [x] **Step 5.2: GeneralPanel import 加 Layers**

L4 改：

```tsx
import { Mic, Volume2, Sparkles, Keyboard, ClipboardList, Layers } from "lucide-react";
```

- [x] **Step 5.3: GeneralPanel 解构加 ocr_models**

L87 改：

```tsx
  const { config: cfg, asr_engines, llm_models, ocr_models, prompts, active_prompt_id, microphones } = configResp;
```

- [x] **Step 5.4: 新增「模型选择」Card（交互 Card 之后）**

在「交互」Card 闭合 `</Card>`（L145）之后、「快捷键」Card（L147）之前插入：

```tsx

      <Card icon={Layers} title="模型选择">
        <Row label="语音识别模型" effect="下次录音">
          <select className={selectClass} value={cfg.asr_engine as string} onChange={(e) => setVal("asr_engine", e.target.value)}>
            {asr_engines.map((e) => <option key={e.name} value={e.name}>{e.label}</option>)}
          </select>
        </Row>
        <Row label="润色模型" effect="立即">
          <select className={selectClass}
            value={llm_models.find((m) => m.current)?.name ?? ""}
            onChange={(e) => setVal("polish_llm", e.target.value)}>
            {llm_models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
          </select>
        </Row>
        <Row label="OCR 模型" effect="下次启动" hint="截图识别用，改后重启生效">
          <select className={selectClass} value={cfg.ocr_model as string} onChange={(e) => setVal("ocr_model", e.target.value)}>
            {ocr_models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
          </select>
        </Row>
      </Card>
```

- [x] **Step 5.5: 删除「语音识别」Card 的识别引擎行**

删除「语音识别」Card 内的识别引擎 Row（L162-167）：

```tsx
        <Row label="识别引擎" effect="下次录音">
          <select className={selectClass} value={cfg.asr_engine as string} onChange={(e) => setVal("asr_engine", e.target.value)}>
            {asr_engines.map((e) => <option key={e.name} value={e.name}>{e.label}</option>)}
          </select>
        </Row>
```

- [x] **Step 5.6: 删除「语音识别润色」Card 的润色模型行**

删除「语音识别润色」Card 内的润色模型 Row（L189-196）：

```tsx
        <Row label="润色模型" effect="立即">
          <select className={selectClass}
            value={llm_models.find((m) => m.current)?.name ?? ""}
            onChange={(e) => setVal("polish_llm", e.target.value)}>
            {llm_models.map((m) => <option key={m.name} value={m.name}>{m.label}</option>)}
          </select>
        </Row>
```

- [x] **Step 5.7: 验证前端 build**

Run: `npm --prefix crates/desktop/frontend run build`
Expected: tsc + vite 通过，新 bundle 生成（含模型选择 Card + ocr_models 接口）。

- [x] **Step 5.8: Commit**

```bash
git add crates/desktop/frontend/src/pages/Settings/index.tsx crates/desktop/frontend/src/pages/Settings/GeneralPanel.tsx crates/desktop/dist
git commit -m "feat(desktop): 设置页模型选择 Card（asr/polish/ocr 集中）+ 删除原两行"
```

---

## Task 6: 全量验证

- [x] **Step 6.1: 后端全量编译 + 测试**

Run: `cargo test -p octopus-infra -p octopus-desktop`
Expected: 0 error，单测全绿（含 config ocr_model + list_ocr_models 两测）。

- [x] **Step 6.2: 前端最终 build**

Run: `npm --prefix crates/desktop/frontend run build`
Expected: 通过。

---

## Task 7: 文档同步 + plan checkbox

**Files:**
- Modify: `docs/architecture.md`（设置页 Card 清单 + ocr_model 字段 + list_ocr_models）
- Modify: 本 plan（checkbox 全勾）

- [x] **Step 7.1: architecture.md 设置页 Card 清单**

设置页 Card 清单由「交互/快捷键/语音识别/语音识别润色/剪贴板」改为「交互/模型选择/快捷键/语音识别/语音识别润色/剪贴板」；标注「模型选择」含 asr_engine（下次录音）/ polish_llm（立即）/ ocr_model（下次启动，OnceLock 重启生效）；「语音识别」Card 去识别引擎行、「语音识别润色」Card 去润色模型行。

- [x] **Step 7.2: architecture.md ocr_model 字段 + list_ocr_models**

AppConfig 字段表加 `ocr_model`（默认 PP-OCRv6-small，OCR 引擎 OnceLock 单例，改后重启生效）；models 查询列表加 `list_ocr_models`（domain='ocr' AND is_enabled=1）；save_app_config 字段数 27→28。

- [x] **Step 7.3: 本 plan checkbox 全勾 + Commit 文档**

```bash
git add docs/architecture.md docs/superpowers/plans/2026-06-28-settings-model-selection.md
git commit -m "docs: 模型选择 Card 同步 architecture + plan checkbox"
```

---

## 验证清单（e2e，待用户桌面环境确认）

1. 设置页「交互」正下方出现「模型选择」Card，含三行：语音识别模型 / 润色模型 / OCR 模型。
2. 语音识别模型下拉 = 原 asr_engines 选项，切换后下次录音生效（与原行为一致）。
3. 润色模型下拉 = 原 llm_models 选项（含「不选择模型」首项），切换后立即生效。
4. OCR 模型下拉显示 PP-OCRv6-small（description 标签），切换后写 DB；重启应用后 OCR 用新模型（OnceLock）。
5. 原「语音识别」Card 不再有「识别引擎」行；原「语音识别润色」Card 不再有「润色模型」行。
6. 重启应用：ocr_model 配置持久化（设置页显示上次选择）；DB app_config 表 ocr_model 行更新。

## 不改动

- `ocr/engine.rs::OcrEngine::instance()` 读取入口（仍 `load_config_key("ocr_model")`）、recognize、单例缓存。
- 侧栏「模型管理」Tab（ModelsPanel，下载/校验 ASR 模型）。
- `db.sql`（OCR models seed + app_config seed 均已存在）。
- ASR/polish 后端切换逻辑（仅前端换 Card 归属）。

