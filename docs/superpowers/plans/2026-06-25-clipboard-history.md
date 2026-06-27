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
- 启用/禁用监听（`clipboard_enabled`）→ shadcn Switch
- 快捷键（`clipboard_shortcut`）→ Input + 录制
- 最大条数（`clipboard_max_items`）→ Select（500/1000/2000/5000）
- 清理天数（`clipboard_max_age_days`）→ Select（7/30/90）
- 点击行为（`clipboard_auto_paste`）→ Select（single/double）

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
- 提取 `ClipboardRow` 子组件：行操作与浮窗一致（复制/收藏/保存图片/打开文件/单条二次确认删除）
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
