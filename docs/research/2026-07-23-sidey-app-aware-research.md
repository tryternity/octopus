# Sidey 调研报告：对 octopus action_bar 的借鉴意义

> **日期**：2026-07-23
> **调研对象**：[Sidey](https://github.com/chentao1006/Sidey)（macOS menu bar AI assistant，Swift/SwiftUI）
> **官网**：https://sidey.ct106.com/
> **目标**：调研 Sidey 的功能特性，找出对 octopus action_bar 的借鉴价值，决定哪些值得实现。
> **结论**：采纳 P0「App-aware 菜单绑定」+ P1「选区位置感知」，明确不借鉴纯 AI 对话范式 / iCloud / Provider switch 等。已基于本调研实现 app-aware 功能（spec `2026-07-23-actionbar-app-aware.md`）。

---

## 0. TL;DR

### 定位差异（不是同类竞品）

> **Sidey** = menu bar 浮窗 AI 对话助手，范式是「**唤起 → 输入问题 → AI 回答 → 复制**」，强调 **per-app assistant 绑定 + 对话连续性**。
> **octopus action_bar** = 选中驱动的命令面板 + 搜索引擎，范式是「**选中 → 唤起 → 挑 action → 结果进编辑器**」，强调 **macOS 原生集成（AX/CGEvent）+ 多 action 类型**。

### 采纳的借鉴点

| 优先级 | 借鉴点 | 状态 |
|---|---|---|
| **P0** | App-aware Assistant（per-app 命令/行为绑定） | ✅ 已实现（spec `2026-07-23-actionbar-app-aware.md`） |
| ~~P1~~ | ~~选区位置感知（浮标定位）~~ | ❌ 放弃（覆盖率 < 20%，详见 §3.1） |
| P1 | 选区捕获的 AX 通知驱动 + 递归查找 | ⏳ backlog |
| P2 | 附件式上下文注入（clipboard/selection 勾选附加） | ⏳ backlog |

### 明确不借鉴的

| Sidey 做法 | 不借鉴原因 |
|---|---|
| Provider 用 switch 字符串（非 protocol） | octopus 的 LLM crate 已有更好抽象，Sidey 这是技术债 |
| iCloud 同步（NSUbiquitousKeyValueStore） | octopus 已有 git 同步体系，且是跨平台 Rust |
| 无 auto-paste / run-and-paste | 两边都没做。octopus 曾有 auto_paste 后删除——有意的产品决策 |
| menu bar NSStatusItem 常驻图标 | octopus 是多窗口工具集（已有托盘菜单），不需要这个入口 |
| 纯 AI 对话范式 | octopus action_bar 的命令+搜索体系更丰富，不该退化成纯对话 |

---

## 1. Sidey 架构概览

macOS menu bar AI assistant，Swift/SwiftUI，约 20 个 Swift 文件，~3700 行核心代码。`LSUIElement = true`（纯 menu bar agent app，Dock 不显示）。

### 核心机制

| 机制 | 实现 | 关键文件 |
|---|---|---|
| **窗口管理** | `NSStatusItem` + **双形态可切换**（NSPopover / 独立 NSWindow）+ Window Companion（吸附窗口边缘跟随） | `ContextAIApp.swift` |
| **App-aware** | `NSWorkspace` 通知 + 0.5s 轮询双保险；`Prompt.apps: [bundleID]` 反向绑定 | `ContextDetector.swift` |
| **选区捕获** | 多级 fallback：AX 直查 → AX 递归（深度 20）→ AppleScript（Finder）→ Cmd+C 模拟（带冷却 + 剪贴板恢复） | `SelectionManager.swift` |
| **Assistant 模型** | 极简：`Prompt { id, name, system, apps: [String], contextMessageCount }` + JSON 文件存储 | `PromptStore.swift` |
| **AI Provider** | `apiFormat` 字符串 + 巨型 switch（openai/anthropic/gemini/ollama），**非 protocol** | `LLMClient.swift` |
| **快捷键** | Carbon `RegisterEventHotKey`（最底层最可靠），默认 Cmd+Option+Space | `HotKeyManager.swift` |
| **数据持久化** | UserDefaults（标量）+ JSON 文件（prompts + history）+ iCloud（NSUbiquitousKeyValueStore + 文件双轨） | `PromptStore.swift` / `HistoryStore.swift` / `SyncManager.swift` |
| **结果后置** | **无 auto-paste**——仅复制到剪贴板 + `app.activate()` 切回前台，用户手动 Cmd+V | `AssistantWindow.swift` |

---

## 2. 能力对比矩阵（Sidey vs octopus action_bar）

| 维度 | octopus action_bar | Sidey | 借鉴价值 |
|---|---|---|---|
| **唤起** | global-shortcut 插件 + Quick Execute 双热键 ✅ | Carbon RegisterEventHotKey + menu bar + 吸附 icon + 选区浮标 + URL scheme | 🟡 Sidey 唤起入口更多元 |
| **窗口形态** | 单例浮窗，跟鼠标/主屏居中 | NSPopover / 独立 Window **双形态可切** + Window Companion（吸附窗口边缘跟随） | 🔴 Window Companion 是 octopus 没有的差异化形态 |
| **输入处理** | 搜索 + 两级菜单混合，6 种 action_type ✅ | 纯 AI 对话 + prompt 标签切换 | 🟢 octopus 更强（命令体系丰富） |
| **上下文捕获** | Cmd+C 模拟 + AX + 三级 dispatch + 剪贴板恢复 ✅强 | AX 直查→递归→Cmd+C fallback + AppleScript 插件 | 🟢 各有千秋，Sidey 的 AX 递归 + kAXSelectedTextChanged 通知值得学 |
| **App-aware** | 识别 app（AppKind 分类）但**无 per-app 行为差异** ⚠️ | **prompt 自带 apps:[bundleID] 反向绑定，per-app 自动切换 assistant** | 🔴 octopus 明显的缺口 → **已实现** |
| **结果后置** | 进 CompactEditor，**无 auto-paste**（已删） | 复制到剪贴板 + 切回前台（也无 auto-paste） | ⚪ 两边都没做 run-and-paste |
| **对话连续性** | **无**（每次 action 独立） | per-app × per-prompt 内存 session + 多轮上下文窗口 | 🟡 取决于产品方向 |
| **AI provider** | LLM crate（已有润色/翻译能力） | switch 字符串（openai/anthropic/gemini/ollama） | ⚪ Sidey 是反面教材 |
| **同步** | vault/hotword git 同步（已有） | iCloud（NSUbiquitousKeyValueStore + 文件双轨） | ⚪ 各自生态 |

---

## 3. 值得借鉴的设计点详解

### 🔴 P0：App-aware Assistant（per-app 绑定）— ✅ 已实现

**Sidey 怎么做**：`Prompt` 自带 `apps: [String]` 字段（bundleID 或 `"*"` 通配），`getPrompts(for: bundleID)` 过滤排序（specific > global）。用户在 Xcode 里自动看到「Code Review」，在 Safari 里自动看到「Summarize」。

**octopus 原状**：action_bar 识别了 `AppKind`（Editor/Terminal/Browser/Chat），但只用于采集策略和按键 dispatch，菜单内容对所有 app 一致。

**已实现的方案**（spec `2026-07-23-actionbar-app-aware.md`）：
- `action_bar_items` 加 `app_bundle_ids` 列（JSON 数组，存 bundle_id）
- 应用索引（`launcher_index`）加 `bundle_id` 列（扫描时读 `CFBundleIdentifier`）
- 唤起时前端 `isItemVisibleForApp` 按前台 app 过滤：全局项（空）永远显示 + 专属项追加
- 设置页 `AppPicker` 多选器（chips + 搜索浮层，调 `list_all_apps` 命令）
- 与现有 `accepts`（text/file/any）维度独立 AND
- **不做类别维度**（新终端在多选器搜选中即可，避免重构 classify_app 硬编码表）

---

### 🔴 P0→P2：Window Companion（窗口边缘吸附跟随）— ⏳ backlog

**Sidey 怎么做**：borderless icon 窗口，贴着当前活动窗口的边缘，随目标窗口移动/缩放实时跟随。用 AX `kAXFocusedWindowAttribute` + `kAXPosition/kAXSize` 拿窗口 frame，注册 `AXObserver` 监听 `kAXMoved/kAXResized/kAXFocusedWindowChanged` 事件驱动（非纯轮询）。

**octopus 现状**：action_bar 是「唤起即来、用完即走」的瞬态浮窗，靠鼠标位置或主屏居中定位。

**借鉴评估**：差异化形态，但需评估是否符合 octopus「工具集」产品定位。建议作为 P2 探索项。技术上 octopus 已有 AX 基础设施（`app_context/macos_ax.rs`），实现 `AXObserver` 监听窗口移动可行。

---

### 🟡 P1：选区捕获的 AX 通知驱动 + 递归查找 — ⏳ backlog

**Sidey 怎么做**（比 octopus 更优的两点）：
1. **`kAXSelectedTextChangedNotification` 事件驱动**：注册 AX Observer，选区变化时回调，比 octopus「唤起时才模拟 Cmd+C」更精准
2. **AX 递归查找选区**：`findSelectedTextRecursive` 深度优先遍历 AX 树（最大深度 20），兼容 webview 等「focused element 不直接暴露选区」的场景

**octopus 现状**：唤起时才检测，靠模拟 Cmd+C + changeCount 判断（有三级 dispatch，完善），但对 webview 类 app（微信内嵌浏览器、Electron app）的选区捕获仍是弱项。

**借鉴建议**：
- **递归 AX 查找**值得加：Cmd+C 之前先 AX 直查（`kAXFocusedUIElementAttribute` → `kAXSelectedTextAttribute` → 递归 children），命中就免掉 Cmd+C 侵入性
- **AX 通知驱动**对 action_bar 瞬态场景意义不大，但对「选区浮标按钮」类常驻功能有价值

---

### ❌ P1：选区位置感知（浮标定位）— 已放弃（2026-07-23 评估）

**Sidey 怎么做**：`getSelectedTextBounds` 用 `kAXSelectedTextRangeAttribute` + `kBoundsForRangeParameterizedAttribute` 拿到选区的屏幕坐标 `CGRect`，浮动按钮贴着选区上方弹出。

**放弃理由**（brainstorming 评估结论）：
1. **覆盖率 < 20%**：只有原生 NSTextView app（TextEdit/Notes/Mail）能通过 AXBoundsForRange 拿到选区坐标。浏览器走 AppleScript JS、VSCode/Cursor/微信/Slack 是 Electron 自绘、终端选区自绘、Sublime 走插件、WPS AX 禁用——这些**全部 fallback 到跟鼠标**。
2. **现有方案已够用**：用户选中文字后鼠标本来就在选区附近，`get_mouse_position` + 碰撞检测对 80%+ 场景已提供不错定位。
3. **代码铁证**：`build_surrounding` 的整套 fallback 链（sublime_plugin → pages_applescript → lsof → 磁盘读文件）正是因为 AX 在主流编辑器上**连文本都拿不到**——文本都拿不到，bounds 更别想。
4. **投入产出比低**：需新写 FFI（参数化 AX API + CGRect 类型）+ 定位算法 + 降级逻辑，工程量不小但收益微弱。

---

### 🟢 P2：附件式上下文注入 — ⏳ backlog

**Sidey 怎么做**：唤起时把选中文本、剪贴板内容作为 `Attachment`（带 `isSelected` 勾选框），用户可勾选是否随消息发送，以 `\n---\n` 拼接进 prompt。

**octopus 现状**：选中内容是「隐式注入」——自动塞进 LLM 的 enriched text，用户无感知、不可控。

**借鉴评估**：如 action_bar 未来增加「自由对话」模式，附件式上下文让用户可控选择「带不带选中/剪贴板」会更透明。对当前「菜单 action」范式价值有限。

---

## 4. Sidey 的技术细节参考（供后续实现借鉴）

### 选区捕获多级 fallback（SelectionManager.swift）

1. **Finder 特例**：AppleScript `tell application "Finder" to get selection`
2. **AX API**（`PermissionManager.canUseAccessibility()` 守卫）：
   - `AXUIElementCreateApplication(pid)` → `kAXFocusedUIElementAttribute`（最快）→ `kAXFocusedWindowAttribute`（catch web views）→ `findSelectedTextRecursive` 深度优先递归（最大深度 20）
   - 同时尝试 `kAXSelectedTextAttribute` 和字符串 `"AXSelectedText"` 两种 attr 名
3. **Cmd+C fallback**（`forceCaptureSelection`）：CGEvent 构造 4 个事件（cmdDown/cDown/cUp/cmdUp），`postToPid(pid)` 精准投递，轮询 4×50ms 等剪贴板变化，**抓完恢复原剪贴板**。带 2s 冷却防 VSCode 菜单闪烁，`com.apple.*` 系统应用跳过。

### App-aware 反向绑定（PromptStore.swift）

```swift
struct Prompt: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var system: String          // system prompt 模板
    var apps: [String]          // bundleIDs 或 "*"
    var contextMessageCount: Int? = nil  // 上下文轮数
}
```

`getPrompts(for: bundleID)` 过滤 `apps.contains("*") || apps.contains(bundleID)`，排序 specific 在前 global 在后。

### Carbon 全局热键（HotKeyManager.swift）

```swift
RegisterEventHotKey(UInt32(keyCode), UInt32(modifiers),
                    hotKeyID, GetApplicationEventTarget(), 0, &eventHotKeyRef)
```

用 Carbon `RegisterEventHotKey`（非第三方库、非 CGEvent tap），是 macOS 全局热键最可靠方案。octopus 用的是 Tauri global-shortcut 插件（封装了底层）。

### 权限处理（PermissionManager）

- 启动时立即检查 `AXIsProcessTrusted()` + 2s 轮询探测（用户去系统设置授权后自动检测到）
- `@Published isAccessibilityGranted` + Combine 订阅，授权瞬间自动启动 monitor
- `#if !APPSTORE` 编译期隔离敏感 API（App Store 沙盒下禁用 AX/CGEvent）

---

## 5. Sidey 的反面教材（明确不学）

| 做法 | 问题 |
|---|---|
| **Provider switch 字符串**（LLMClient.swift） | 4 种格式逻辑全挤在一个类，扩展性差；加新 provider 要改 3 个 switch case。octopus 的 LLM crate 有更好抽象 |
| **NSUbiquitousKeyValueStore 同步** | 受 100KB/key 限制；octopus 是跨平台 Rust，iCloud 不适用；已有 git 同步体系 |
| **无 auto-paste** | Sidey 把「回填」留给用户（复制 + 切回前台）。octopus 曾有 auto_paste 后删除——验证了这是有意的产品决策，不是遗漏 |
| **AppKind 硬编码 match 表** | Sidey 不做类别维度（纯 bundle_id），但它识别 app 用的也是硬编码。octopus 的 classify_app 同样问题——本期 app-aware 也选了纯 bundle_id 不碰类别 |

---

## 参考

- [Sidey 源码](https://github.com/chentao1006/Sidey)（本地 `/Users/wudarui/workspace/agent/Sidey`）
- [Sidey 官网](https://sidey.ct106.com/)
- 实现 spec：`docs/superpowers/specs/2026-07-23-actionbar-app-aware.md`
- 实现 plan：`docs/superpowers/plans/2026-07-23-actionbar-app-aware.md`
