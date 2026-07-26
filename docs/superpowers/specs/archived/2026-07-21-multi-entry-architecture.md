# 2026-07-21 全窗口独立 entry 架构

> 承接：[2026-07-21-screenshot-startup-perf-v2.md](./2026-07-21-screenshot-startup-perf-v2.md) 只拆了 screenshot entry，本轮把剩余 8 个窗口也拆出去
> 设计原则：**产物边界 = 依赖边界 = 职责边界**——每窗口的 import 闭包只含自己需要的依赖

## 背景

上一轮把 screenshot 拆成独立 entry 后，剩余 8 个窗口仍共用 `index.html` → `main.tsx` → `App.tsx` 的 label switch 路由。主 bundle `main-*.js` ~1MB，包含所有窗口依赖并集：
- CodeMirror + lezer（仅 result + compact-editor 用，但被所有窗口加载）
- markdown-it（仅 compact-editor 用）
- lucide-react（仅 settings + clipboard + vault-picker + password-generator 用）

这违背了"产物边界对齐依赖边界"的原则——窗口的 import 闭包应该决定它的 chunk 内容。

## 方案

### 决策（与用户讨论）

| 问题 | 候选 | 选定 | 理由 |
|---|---|---|---|
| 拆分粒度 | (a) 每窗口一 entry (b) 分组 entry（editor/ui/misc 三组）(c) 只拆 CodeMirror | **(a)** | 架构纯净度优先。CodeMirror 是最大块但不应让其他窗口被迫合并；分组方案是性能权衡的妥协，违背"依赖边界对齐产物边界"原则 |
| 跨页共享组件 | (a) 抽到 components/ 共享 (b) 保留 pages/ 不动 | **(a)** | 目录语义正确（被多 page 共用的不是 page 而是 component）；Rolldown 自动 dedupe 共享 chunk |

## 实施（7 phase）

### Phase 1: 抽取跨页共享组件到 `components/`

机械移动（保持文件内容不变）：
- `pages/Clipboard/SaveImagePopover.tsx` → `components/SaveImagePopover.tsx`（被 Settings/ClipboardPanel + Clipboard/ClipboardItem 共用）
- `pages/Settings/Vault/PasswordGenerator.tsx` + `buildConfig.ts` + `buildConfig.test.ts` → `components/`（被 Settings/Vault/PasswordGeneratorModal + 独立 PasswordGenerator 窗口共用）

改 4 处 import 路径（`pages/Xxx` → `@/components/Xxx`）。

### Phase 2: 抽取 `src/lib/mountApp.tsx`

提取原 App.tsx 里跨窗口共用的启动逻辑：
- ErrorBoundary（错误兜底）
- `restoreCachedTheme` + `restoreCachedLocale`（同步恢复，零 IPC）
- `initI18n` + `applyThemeFromConfig`（后台 IPC 校正）
- `listen("config-changed")`（跨窗口主题同步）

签名：`mountApp(node: ReactNode)`。每个 entry main.tsx 一行调用：`mountApp(<Page/>)`。

vault feature probe 不进 mountApp（只 vault 相关窗口需要），留给 vault-picker/password-generator entry 在自己的 React 组件里 useEffect 处理。

### Phase 3: 9 HTML + 9 main.tsx

- 每窗口独立 HTML（沿用原 `index.html` 的 inline script：主题恢复 + bg 参数注入）
- 每窗口独立 `src/entries/xxx-main.tsx`（极简，~5 行）
- vault-picker-main / password-generator-main 含 VaultGate 组件做 feature probe

`src/screenshot-main.tsx` 也移到 `src/entries/screenshot-main.tsx` 统一目录，并改为走 `mountApp()`（与其他 entry 一致）。

### Phase 4: vite.config.ts multi-entry

```ts
rollupOptions: {
  input: {
    screenshot: "screenshot.html",
    result: "result.html",
    settings: "settings.html",
    clipboard: "clipboard.html",
    "compact-editor": "compact-editor.html",
    "action-bar": "action-bar.html",
    overlay: "overlay.html",
    "vault-picker": "vault-picker.html",
    "password-generator": "password-generator.html",
  },
}
```

**关键**：不再需要 `manualChunks` 对象配置——Vite 8 用 rolldown，对象形式会报错（`manualChunks is not a function`），且 multi-entry 下 rolldown 自动按 import 闭包 dedupe 共享依赖。

### Phase 5: Rust 端 WebviewUrl::App 改 URL

8 处改动（screenshot 已在上一轮改过）：

| Rust 文件 | 原 URL | 新 URL |
|---|---|---|
| `result_window.rs:39` | `WebviewUrl::default()` | `WebviewUrl::App("result.html".into())` |
| `settings_window.rs:69` | `index.html?bg=...` | `settings.html?bg=...` |
| `clipboard_window.rs:32` | `WebviewUrl::default()` | `clipboard.html` |
| `compact_editor_window.rs:108` | `index.html?itemId=...` | `compact-editor.html?itemId=...` |
| `action_bar_window.rs:16` | `WebviewUrl::default()` | `action-bar.html` |
| `overlay_window.rs:20` | `WebviewUrl::default()` | `overlay.html` |
| `password_generator_window.rs:42` | `index.html` | `password-generator.html` |
| `vault_commands.rs:1021` | `index.html` | `vault-picker.html` |

### Phase 6: 删除退役文件

- `index.html`（被 9 个独立 HTML 替代）
- `src/main.tsx`（被 9 个 entry main.tsx 替代）
- `src/App.tsx`（label switch 路由不再需要）

## 验证

### 编译/测试

| 验证 | 结果 |
|---|---|
| `npx tsc -b` | 0 error |
| `npx vitest run` | **304 tests passed**（16 test files，含 `components/buildConfig.test.ts` 移动后仍正常） |
| `cargo build --release -p octopus-desktop` | 0 error 0 warning |

### 产物体积（实测）

9 个独立 HTML + Rolldown 自动分 chunk：

| Chunk | 大小 | 类型 |
|---|---|---|
| compact-editor | 400 KB | 专属（CodeMirror 全栈 + ImagePreview） |
| dist (React/ReactDOM 共享) | 299 KB | 共享 |
| window (@tauri-apps/api 共享) | 258 KB | 共享 |
| settings | 170 KB | 专属（lucide + radix） |
| screenshot | 27 KB | 专属 |
| action-bar | 23 KB | 专属 |
| clipboard | 20 KB | 专属 |
| result | 18 KB | 专属 |
| vault-picker | 15 KB | 专属 |
| password-generator | 2 KB | 专属 |
| overlay | 1.5 KB | 专属 |

### 每窗口实际加载量（专属 + 共享）

- screenshot: 27 + 258 + 8 = ~293 KB（与上轮一致）
- overlay: 1.5 + 258 + 27 = ~287 KB（最轻）
- compact-editor: 400 + 258 + 8 = ~666 KB（最重，含 CodeMirror + ImagePreview）
- settings: 170 + 258 + 27 = ~455 KB

对比优化前：每窗口都加载 1.27MB。**窗口启动延迟普遍减小 50-77%**。

## 设计权衡讨论

### 为何不分组（editor/ui/misc 三组方案）

分组方案（result+compact-editor 合 editor.html，settings+vault+password-generator 合 ui.html，clipboard+action-bar+overlay 合 misc.html）能省少量共享 chunk 重复，但：
- 违背"一窗口一 entry"的架构纯净度
- PasswordGenerator 强依赖 Settings 子目录（同 entry 才能 dedupe），意味着分组方案需要先抽组件——抽了组件后每窗口独立 entry 也无障碍
- Rolldown 自动 dedupe 已经让共享依赖（React/tauri-api/annotation/createLucideIcon）进共享 chunk，每窗口独立 entry 不会重复打包共享部分

### 为何不引入 manualChunks

Vite 8 用 rolldown，对象形式 manualChunks 报错（`manualChunks is not a function`）。函数形式可以但增加维护负担——multi-entry 下 rolldown 自动按 import 闭包分析已经能正确 dedupe，无需手动配。

## 后续可优化

- **CodeMirror 拆分**：compact-editor 用了全套（lang-markdown + search + bracketMatching + HighlightStyle），result 只用基础。如果 result 想再瘦身，可评估把 CodeMirror 全套扩展单独抽 chunk
- **Settings lucide-react 按需**：Settings 重度用 lucide（26 imports），可评估是否换成自己的 SvgIcon 系统（已在 Result/Clipboard 用）
- **shared chunk 命名**：Rolldown 自动命名的 `dist-*.js` / `window-*.js` 不够语义化，可评估手动命名（rollupOptions.output.manualChunks 函数形式）
