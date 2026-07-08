# AI 命令面板实施记录

> plan 是实施记录，非一次性待办。以下为实际执行过程中的全部提交和偏差回写。

**Goal:** 新建 `action_bar_window` 迷你浮窗，用户选中文本→热键→模拟 Cmd+C→弹出动作栏→AI/搜索/翻译/网页→CompactEditor 展示结果。

**Architecture:** 新建独立 Tauri 窗口 + Rust 命令层 + React 前端（两级菜单 + 键盘导航）。复用现有 focus_tracker / clipboard / llm / theme 基础设施。

## Global Constraints

- **物理/逻辑坐标**（⚠️ 关键反复踩坑）：`CGEvent::location()` 返回**物理像素**，Tauri `LogicalPosition` 是**逻辑像素**。必须除以 `scale_factor()`（Retina=2.0）。详见 architecture.md 和 AGENTS.md。
- **capabilities 白名单**：每个新窗口必须加入 `capabilities/default.json` 的 `windows` 数组，否则 Tauri 2 拒绝 `listen`/`invoke`。
- **trigger 后台线程**：模拟 Cmd+C 后有 200ms sleep——不能在主线程（阻塞事件循环 → 窗口无焦点 → Esc/按钮无响应）。必须 `std::thread::spawn`。
- **mousedown capture 陷阱**：`addEventListener("mousedown", fn, true)` 的 capture 模式在 `onClick` 之前触发 → 按钮点击被拦截。外部点击检测用 `click` 事件冒泡阶段（`false`）。
- **常规窗口隐藏**：全局热键触发浮窗前必须隐藏 settings/compact_editor（Regular 激活策略下 app 被激活会把所有可见窗口带到前台）。

## 核心功能 Task（7 个）

| Task | 状态 | commit | 说明 |
|------|------|--------|------|
| 1. URL 宽松检测 + TDD | ✅ | `035a8c5` | 域名/IP/localhost 三路检测，14 测试 |
| 2. simulate_copy | ✅ | `38f2831` | macOS osascript Cmd+C |
| 3. 配置项注册 | ✅ | `6200dc6` | action_bar_shortcut（serde 3 处） |
| 4. 窗口创建 + 热键 | ✅ | `4fb84f0` | action_bar_window 透明浮窗 + register |
| 5. 后端命令层 | ✅ | `4fb84f0` | trigger/Cmd+C/AI/paste/openURL |
| 6. 前端浮窗 | ✅ | `42c49b5` | 两级菜单 + 键盘导航 + loading |
| 7. 设置页 UI | ✅ | `52db39c` | 快捷键配置 |

## 调试 + 优化 Task（27 个）

| # | commit | 说明 |
|------|--------|------|
| 8 | `02e0239` | 浮窗位置+按钮响应+点击外部消失 |
| 9 | `06a6364` | **capabilities 白名单**——action_bar_window 加入 windows 数组 |
| 10 | `4c912f4` | Esc+按钮修复（data-action-bar + ref 读最新状态） |
| 11 | `8b570de` | **trigger 移入后台线程**——200ms sleep 阻塞主线程 |
| 12 | `196c047` | 全局热键前隐藏常规窗口 |
| 13 | `8f19303` | **mousedown capture 拦截**——改 click 事件冒泡 |
| 14 | `3c964cc` | **llm_config_ignore_mode**——polish_mode=Disabled 时不返回 None |
| 15 | `2d0e5b3` | Cmd+C 不入剪贴板历史 |
| 16 | `bbdbb9e` | Cmd+A 全选后剪贴板未变时仍使用 |
| 17 | `5d258b3` | AI 结果改为 CompactEditor 展示（不做 Run And Paste） |
| 18 | `fee70f9` | **CompactEditor isTemp 临时 tab**——AI 结果不写 DB |
| 19 | `258485e` | 临时 tab 每次新开 tab 页（emit 推送） |
| 20 | `76fee08` | loading 状态保持浮窗可见 |
| 21 | `317d5cf` | 翻译 5s + 润色/摘要/解释 10s 超时 |
| 22 | `376076c` | 窗口高度 50→200（子菜单被截断） |
| 23 | `5163ad8` | 子菜单放上方 + ↑↓ 切行 ←→ 移动 |
| 24 | `e777a13` | 搜索改为子菜单（Google/百度/Bing） |
| 25 | `6eded3c` | 高亮 voice 色 ring，hover muted |
| 26 | `928e117` | 子菜单固定 h-[38px] + shrink-0 |
| 27 | `923a231` | 主菜单在上+子菜单在下，h-0 隐藏 |
| 28 | `928ede1` | 子菜单 Enter 用 submenuTypeRef |
| 29 | `00b3e86` | action bar 位置改为鼠标正上方 X 居中 |
| 30 | `c08ab26` | **CGEvent 物理坐标÷scale**——关键修复 |

## 与原 plan 的偏差

1. **AI 结果不做 Run And Paste**——浏览器安全策略阻止模拟粘贴 + 焦点时序问题。改为 CompactEditor 展示。
2. **搜索改为子菜单**——去掉设置页搜索引擎配置，改为面板内选择 Google/百度/Bing。
3. **trigger 后台线程**——200ms sleep 不能在主线程。
4. **capabilities 白名单**——新窗口必须加入。
5. **CompactEditor isTemp**——AI 结果不写 DB，保存按钮灰掉。
6. **热键前隐藏常规窗口**——防 app 激活时窗口抢焦点。
7. **物理/逻辑坐标**——CGEvent 返回物理像素，必须除 scale。
