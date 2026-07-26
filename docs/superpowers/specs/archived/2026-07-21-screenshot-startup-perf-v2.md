# 2026-07-21 截图启动性能优化 v2（bundle 拆分 + i18n 解阻塞 + 并行截图）

> 上一轮：[2026-07-20-screenshot-startup-perf.md](./2026-07-20-screenshot-startup-perf.md)（RGBA bytes 直传 + 窗口创建去 sleep + force show 兜底）
> 本轮在上一轮基础上继续压缩 ready 时间，并并行化后端截图。

## 背景

上一轮优化后仍有两个遗留问题（SESSION_HANDOFF.md P2 + P3）：

1. **前端 ready 慢**：3s force show 兜底仍偶尔触发。根因是 JS bundle 1.27MB 单 chunk + `initI18n` 的 `get_config` IPC 阻塞渲染。
2. **双屏截图 ~800ms**：`capture_all_monitors` 同步串行 for 循环，两屏顺序截图。

## 测量数据

### 前端 bundle（优化前）

`crates/desktop/dist/assets/index-CmP3Annf.js`：**1,295,103 B (1.27 MB)**，9 个窗口共用。

bundle 构成（截图窗口实际只需要 react-dom + tauri api + 几个本地组件，但被打进了）：
- CodeMirror + Lezer：3.1M + 1.7M（node_modules 体积，仅 Result/CompactEditor 用）
- markdown-it：1.8M（仅编辑器用）
- lucide-react：39M（仅 Settings/Clipboard/Vault 用）

### 后端截图（优化前）

`capture_all_monitors` 同步串行：双屏 4K ~800ms（两屏顺序 ~400ms each）。

**关键纠偏**：原以为是 ScreenCaptureKit 固有开销，实际 xcap 0.9.6 底层是 `CGWindowListCreateImage`（macOS 旧版 CoreGraphics API），**无线程亲和性约束**（不像 SCStream 的 runloop 绑定），完全可以并行。

## 方案

### 决策矩阵（与用户讨论）

| 子任务 | 候选方案 | 选定 | 理由 |
|---|---|---|---|
| bundle 拆分 | (a) 路由级 React.lazy (b) 独立 screenshot entry | **(b)** | 产物边界 = 依赖边界 = 职责边界。entry 让截图域与编辑器域物理隔离，不依赖 manualChunks 持续维护 |
| i18n 解阻塞 | (a) 先渲染再后台初始化 (b) localStorage 缓存 + 后台校正 | **(b)** | 与 `restoreCachedTheme` 同范式，跨启动零延迟，首屏直接正确 locale |
| 并行化 | (a) thread::scope + spawn_blocking (b) tokio::spawn_blocking per monitor | **(a)** | 零新依赖，scope 内线程 join 开销小于 Tokio blocking pool 调度 |

### Task 1: 独立 screenshot.html entry

**文件**：
- `crates/desktop/frontend/screenshot.html`（新建）：复制 index.html 的 inline script（主题恢复 + 透明窗口处理），改 module 指向 `/src/screenshot-main.tsx`
- `crates/desktop/frontend/src/screenshot-main.tsx`（新建）：极简入口，只 import Screenshot + theme/i18n restore
- `crates/desktop/frontend/vite.config.ts`：`rollupOptions.input = { main: "index.html", screenshot: "screenshot.html" }`
- `crates/desktop/src/screenshot_commands.rs:175`：`WebviewUrl::App("screenshot.html")`（原 `index.html?screenshot=1`）

**Rolldown 注意**：Vite 8 用 rolldown，`manualChunks` 必须是函数形式（对象形式会报 `manualChunks is not a function`）。但 multi-entry 下 rolldown 自动按 import 闭包 dedupe 共享依赖，不需要手动配 manualChunks。

**产物**（实测）：
```
dist/screenshot.html                     1.41 kB
dist/index.html                          2.06 kB
dist/assets/Screenshot-C9q4c18o.css     79.14 kB  (共享)
dist/assets/screenshot-Bu6POavM.js       0.18 kB  (screenshot-main 入口胶水)
dist/assets/Screenshot-kIK7xi7n.js     291.72 kB  (截图依赖闭包：React + tauri api + annotation + Screenshot)
dist/assets/main-Dzdrtfki.js         1,003.60 kB  (主入口：含 CodeMirror/markdown-it/lucide-react)
```

验证 Screenshot chunk 不含重依赖：`grep -c "codemirector|markdown_it|lucide" Screenshot-*.js` = 0。

### Task 2: i18n localStorage 缓存

**文件**：
- `crates/desktop/frontend/src/lib/i18n.ts`：
  - 模块加载时同步 `localStorage.getItem("octopus-locale")` 恢复 `currentLocale`（零 IPC）
  - `setLocale` 时同步 `localStorage.setItem` 持久化
  - 新增 `restoreCachedLocale()`（语义对齐 `restoreCachedTheme`）
  - `initI18n` 注释改为"后台校正"语义
- `crates/desktop/frontend/src/main.tsx`：
  ```ts
  restoreCachedTheme()
  restoreCachedLocale()
  const root = createRoot(...)
  root.render(<App />)
  initI18n().catch(() => {})  // 后台 IPC 校正
  ```
- `crates/desktop/frontend/src/screenshot-main.tsx`：同范式

**多窗口同步**：Settings 改 locale → `emit("locale-changed")` → 各窗口 `listen` → `setLocale`（写 localStorage + 触发 listeners）→ `useT` 订阅的组件重渲染。

### Task 3: capture_all_monitors 并行化

**文件**：
- `crates/capx/src/capture.rs::capture_all_monitors`：
  - `Monitor::all()` 仍在调用方线程（xcap 内部 `MainThreadMarker`）
  - 预提取 monitor 元数据到 `Vec<(Monitor, name, x, y, w, h)>`
  - `std::thread::scope(|s| { monitor_infos.iter().map(|m| s.spawn(move || m.capture_image())) })`
  - join 时分类处理：`Ok(Ok(capture))` / `Ok(Err(e))` 跳过 / `Err(panic)` 跳过
- `crates/desktop/src/screenshot_commands.rs:93`：`tokio::task::spawn_blocking(capture_all_monitors).await`（隔离 Tokio worker）

**线程安全依据**：
- `Monitor` 内部 `ImplMonitor { cg_direct_display_id: CGDirectDisplayID }`，`CGDirectDisplayID = u32`，天然 `Send + Sync`
- `CGWindowListCreateImage` 线程安全（Apple 官方文档明确），不同于 SCStream 的 runloop 绑定

## 验证

### 编译
- `npm run build`：0 error 0 warning
- `cargo build --release -p octopus-capx -p octopus-desktop --features embedded,custom-protocol`：0 error 0 warning

### 测试
- `npx vitest run src/lib/i18n.test.ts`：6 passed
- `cargo test -p octopus-capx --lib`：49 passed

### 产物验证
- `dist/assets/Screenshot-*.js` 291KB（vs 原 1.27MB，**降 77%**）
- `grep -c "codemirror|markdown_it|lucide" dist/assets/Screenshot-*.js` = 0（无重依赖污染）

### 端到端（待用户在双屏环境实测）
- 触发截图 → 截图窗口 <1s ready（不再 force show）
- 日志 `Monitor X captured: ... elapsed: ?ms` 两屏并行（elapsed 接近，总耗时 ≈ max 而非 sum）

## 不做（超出范围）

- React.lazy（与 entry 方案混用增加复杂度，主入口 bundle 暂不优化）
- bundle 分割 lucide-react（entry 拆分后自然不进 screenshot chunk）
- SCStream / ScreenCaptureKit 迁移（xcap 0.9.6 用的 CG API 已够用，并行化即可）
- 前端 bundle analyze 工具引入（npm run build 输出足够）

## 后续可继续优化

- **screenshot chunk 进一步压**：annotation.ts 423 行 + SVG 资源，可评估是否拆 mosaics/highlight 模块
- **index.html 主入口优化**：如果 settings 等窗口也想加速，可单独拆 entry 或用 React.lazy
- **capx 截图流式化**：当前是"截完所有屏→建窗"，未来可"截完一屏→立即建窗该屏"流水线（用户感知首屏更早）
