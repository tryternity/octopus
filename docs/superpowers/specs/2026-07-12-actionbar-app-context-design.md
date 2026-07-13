# Action Bar 应用上下文获取设计

> **状态**：设计中
> **日期**：2026-07-12
> **scope**：在 action bar 触发时，根据操作系统获取选中文本所在应用的上下文信息（来源应用、前后文、窗口标题），让 LLM 动作具备情境感知能力
> **前置文档**：[`2026-07-12-action-bar-command-shortcut-design.md`](./2026-07-12-action-bar-command-shortcut-design.md)（action bar DB 化与菜单架构）

---

## 1. 背景与动机

### 1.1 现状

action bar 触发流程（`trigger_action_bar`，仅 macOS）：

1. 热键 → 记录剪贴板快照
2. osascript 模拟 `Cmd+C`
3. 读剪贴板拿到选中文本 `text`
4. `ActionBarContext { text }` 暂存到全局 `PENDING_CONTEXT`
5. 显示浮窗

前端 `ActionBar/index.tsx` mount 时调 `action_bar_get_context` 拿 `context.text`，直接喂给 translate / polish / summarize / explain 等 AI 动作。

**问题**：`ActionBarContext` 只有裸选中文本，LLM 不知道这段文字来自 Word 还是终端还是浏览器，无法利用前后文做更智能的操作。

### 1.2 期望

用户例子：

- **在 Word 里选中一段** → action bar 应能拿到选中前后的段落，LLM 据此做更贴合语境的润色/翻译
- **在终端里选中一行** → action bar 应能拿到终端之前的输出历史，LLM 据此解释命令结果或诊断错误

### 1.3 设计约束（已确认）

| 决策点 | 选择 |
|--------|------|
| 技术方案 | **平台无障碍 API（方案 B）**：macOS Accessibility / Windows UIA / Linux AT-SPI |
| MVP 平台范围 | **仅 macOS**（action bar 本身当前仅 macOS 支持；Windows/Linux 留 trait stub） |
| 失败行为 | **降级到现状**：上下文获取失败绝不阻塞浮窗显示，`source`/`surrounding` 为 None 时行为等同改造前 |
| Terminal scrollback | 默认最近 **~50 行 / 2000 字** 截断 |
| 前后文裁剪 | before/after 各默认 **2000 字** 截断 |
| 权限 | 无新增——action bar 模拟 Cmd+C 已需「辅助功能」权限 |

---

## 2. 为什么选无障碍 API（方案对比）

| 方案 | 机制 | 优点 | 致命短板 |
|------|------|------|---------|
| **A. 每 App 写脚本** | Word 走 AppleScript dictionary、Terminal 走 osascript 取 scrollback、浏览器各自一套 | 每个 App 数据最丰富 | N² 维护成本；脚本随版本崩；Windows/Linux 要全部重写——与「按 OS 分发」诉求背道而驰 |
| **B. 平台无障碍 API** ✅ | 读焦点元素的 `AXSelectedText` / `AXValue` / `AXSelectedTextRange`，按 range 切前后 | 一套代码覆盖**所有原生 App**；三平台各有官方标准 API，天然映射「按 OS 分发」 | 需无障碍权限（项目已需）；Electron/自绘 App 的 AX 树可能稀疏（降级即可） |
| **C. B + A 增强层** | 默认走 AX；对已知 App（iTerm 全量 scrollback、浏览器 DOM）叠加专属取数器 | 覆盖率最高 | MVP 过重，留作 v2 |

**选 B 的理由**：

1. 「按 OS 分发」在工程上的标准答案就是三个 OS 各有无障碍 API，语义对齐（焦点元素 / 选区 / 文本值 / 窗口标题）。
2. 用户举的两个场景 AX 都能直接拿到：
   - **Word/TextEdit**：`AXValue` 即全文，按 `AXSelectedTextRange` 切前后。
   - **Terminal.app / iTerm2**：文本区的 `AXValue` 就是 scrollback 全文。
3. 项目已用 osascript 模拟按键、本就需无障碍权限，**无新增权限负担**。
4. 失败时降级到「仅 text」，零回归。

**方案 C（v2 预留）**：对 AX 覆盖不足的 App（如部分 Electron 应用）叠加专属取数器。

---

## 3. 数据模型

### 3.1 ActionBarContext 升级

```rust
/// action bar 触发时的完整上下文。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    /// 选中文本（保持兼容，必填）。
    pub text: String,
    /// 来源应用信息（新增，可空——取不到时 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<AppSource>,
    /// 选中文本的周围上下文（新增，可空）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surrounding: Option<SurroundingText>,
}

/// 选中文本所在的应用。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSource {
    /// macOS bundle id（如 `com.microsoft.Word`），其他平台可空
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// 应用显示名（如 "Word"、"Terminal"）
    pub name: String,
    /// 应用类别——前端/LLM 据此决定如何利用上下文
    pub kind: AppKind,
}

/// 应用语义类别。Terminal 的 `before` 是命令输出历史，
/// Editor 的 `before` 是上文段落，含义不同，LLM 提示词需区分。
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    /// 编辑器：Word、TextEdit、Sublime、VSCode 文本区等
    Editor,
    /// 终端：Terminal.app、iTerm2、Windows Terminal 等
    Terminal,
    /// 浏览器：Safari、Chrome、Firefox 的网页文本区
    Browser,
    /// 聊天/笔记类 App
    Chat,
    /// 无法归类
    Unknown,
}

/// 选中文本的周围文本。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurroundingText {
    /// 选区前的文字（按配置截断，默认 2000 字）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// 选区后的文字（按配置截断，默认 2000 字）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// 窗口标题（如 "report.docx - Word"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
}
```

### 3.2 向后兼容

`source` / `surrounding` 均为 `Option` 且 `skip_serializing_if`。前端旧逻辑只读 `context.text` 不受影响。新增字段是纯增量。

---

## 4. 平台抽象（trait + cfg 分发）

### 4.1 ContextProvider trait

```rust
/// 平台无关的应用上下文获取接口。
/// 各 OS 各有一个实现模块，通过 cfg 分发。
pub trait ContextProvider {
    /// 在 action bar 拿到选中文本后调用，采集来源应用 + 环境上下文。
    ///
    /// 返回语义：
    /// - `Ok(ctx)` —— 至少拿到 `source`（前台 app 信息靠 NSWorkspace 等系统 API，几乎不会失败），
    ///   `ctx.surrounding` 可能 None（AX 取数失败 / 焦点元素无文本）。
    /// - `Err(e)` —— 连 source 都拿不到的极端情况。调用方应降级到「仅 text」。
    fn gather(&self) -> anyhow::Result<ExtraContext>;
}

/// gather 采集到的额外上下文（拼进 ActionBarContext 前的中间结构）。
pub struct ExtraContext {
    /// 前台应用信息（几乎总能拿到）。
    pub source: AppSource,
    /// 选区周围文本（AX 取数失败时 None）。
    pub surrounding: Option<SurroundingText>,
}
```

### 4.2 模块布局

```
crates/desktop/src/
└── app_context/
    ├── mod.rs          # trait 定义 + provider() 工厂函数
    ├── macos_ax.rs     # macOS Accessibility 实现（MVP）
    ├── windows_uia.rs  # Windows UIA stub（返回 None）
    └── linux_atspi.rs  # Linux AT-SPI2 stub（返回 None）
```

`provider()` 工厂：

```rust
pub fn provider() -> Box<dyn ContextProvider> {
    #[cfg(target_os = "macos")]
    { Box::new(macos_ax::AxProvider) }
    #[cfg(not(target_os = "macos"))]
    { Box::new(NullProvider) } // 永远返回 None
}
```

### 4.3 trigger_action_bar 集成点

在 `trigger_action_bar` 拿到 `text` 后（现有第 5 步暂存上下文处），插入一次 `gather()` 调用：

```rust
// 5. 暂存上下文
let mut ctx = ActionBarContext { text, source: None, surrounding: None };
match crate::app_context::provider().gather() {
    Ok(extra) => { ctx.source = Some(extra.source); ctx.surrounding = extra.surrounding; }
    Err(e) => log::warn!("[action-bar] context gather failed (degraded): {}", e),
}
*PENDING_CONTEXT.lock().unwrap() = Some(ctx);
```

**硬约束**：`gather()` 失败只记日志并降级，绝不 `return` 或阻塞浮窗。

---

## 5. macOS AX 取数路径（核心实现）

### 5.1 取数步骤

1. **前台应用**：`NSWorkspace.shared.frontmostApplication` → pid + bundleIdentifier + localizedName
2. **AppKind 映射**：bundle id 查内置映射表（Terminal/iTerm → Terminal；Word/TextEdit/Sublime/VSCode → Editor；Safari/Chrome/Firefox → Browser；微信/Slack/Discord → Chat；未命中 → Unknown）
3. **AX 应用元素**：`AXUIElementCreateApplication(pid)`
4. **焦点元素**：`AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute, ...)`
5. **窗口标题**：焦点元素或应用元素的 `kAXTitleAttribute`
6. **选区与全文**：
   - `AXSelectedText`（与剪贴板 text 交叉验证，可选）
   - `AXSelectedTextRange`（CFRange）
   - 元素 `AXValue`（全文）
7. **切前后文**：按 range 在全文里切 before / after，各裁剪到配置上限（默认 2000 字）
8. **Terminal 特例**：Terminal.app / iTerm2 的文本区 `AXValue` 即 scrollback 全文，`before` 从选区起点向前取，以 **50 行或 2000 字先达到者**为准截断（而非按等长 range 切）

### 5.2 bundle id → AppKind 映射表

```rust
fn classify_app(bundle_id: &str) -> AppKind {
    match bundle_id {
        "com.apple.Terminal" | "com.googlecode.iterm2" => AppKind::Terminal,
        "com.microsoft.Word" | "com.apple.TextEdit"
          | "com.sublimetext.4" | "com.microsoft.VSCode" => AppKind::Editor,
        "com.apple.Safari" | "com.google.Chrome"
          | "org.mozilla.firefox" => AppKind::Browser,
        // ... 更多见实现
        _ => AppKind::Unknown,
    }
}
```

映射表是纯函数，易于单元测试。未命中返回 Unknown，前端据此决定是否弱化上下文展示。

### 5.3 超时与性能

- AX 调用整体设 **500ms 上限**。超时 → 该字段 None，其余字段照常。
- 实现：在独立线程跑 `gather()`，主线程 `recv_timeout(500ms)`。超时返回部分结果（已采集的字段）。
- 全文 `AXValue` 可能很大（如长文档）——读后立即裁剪，不持有全量。

### 5.4 依赖

使用 `objc2` 生态调用 AppKit / ApplicationServices：

```toml
# crates/desktop/Cargo.toml 新增（macOS only）
[target.'cfg(target_os = "macos")'.dependencies]
objc2-app-kit = { version = "0.2", features = ["NSWorkspace", "NSRunningApplication"] }
objc2-accessibility = "0.2"
# 或直接用 core-foundation 绑定 ApplicationServices（与项目已有 core-foundation 0.10 一致）
```

具体绑定方式（objc2 vs raw FFI）在实现阶段定，优先与项目现有 `core-graphics` / `core-foundation` 风格一致。

---

## 6. 失败与降级（不变量）

| 故障 | 行为 |
|------|------|
| 非 macOS 平台 | `NullProvider` 返回 None，行为等同改造前 |
| 无辅助功能权限 | `AXUIElement` 调用返回错误码 → gather 返回 None → 降级 |
| AX 调用超时（>500ms） | 返回已采集的部分字段，缺失字段 None |
| 焦点元素无 `AXValue` | `surrounding` 为 None，`source` 照常返回 |
| AppKind 未命中 | `Unknown`，前端弱化上下文展示 |
| gather 整体 Err | 记日志，ctx 仅含 `text`，**浮窗照常显示** |

**最高优先级不变量**：上下文获取的任何失败都不得影响 action bar 核心流程（取选中、显示浮窗、执行动作）。

---

## 7. 前端消费

### 7.1 ActionBarContext 类型更新

```ts
type AppKind = 'editor' | 'terminal' | 'browser' | 'chat' | 'unknown';

interface AppSource {
  bundleId?: string;
  name: string;
  kind: AppKind;
}

interface SurroundingText {
  before?: string;
  after?: string;
  windowTitle?: string;
}

interface ActionBarContext {
  text: string;
  source?: AppSource;
  surrounding?: SurroundingText;
}
```

### 7.2 LLM 提示词拼接

translate / polish / summarize / explain 的请求构造里，当 `context.source` / `context.surrounding` 存在时，追加情境块到 prompt：

```
【选中文本】{text}
【来源】{source.name}（{source.kind}）{surrounding.windowTitle ? "- " + windowTitle : ""}
【上文】{surrounding.before}
【下文】{surrounding.after}
```

前端按 `appKind` 决定带哪段：

- **Terminal**：`before` 是命令输出历史，explain/summarize 高价值；translate 可能不需要
- **Editor**：before/after 是上下文段落，polish/translate 高价值
- **Browser**：before/after 是网页上下文，价值中等

具体「哪些 action 带哪些字段」的矩阵在实现阶段细化，MVP 可先「全带」，后续按效果调优。

### 7.3 可选：UI 展示来源

浮窗可显示一个小标签（如 `📍 Word`），让用户感知上下文已获取。这是 v1.1 增强，非 MVP 必需。

---

## 8. 权限

### macOS 辅助功能权限

- action bar 模拟 Cmd+C **已需**「辅助功能」权限
- AX API 读取同样走此权限——**无新增**
- 首次缺失时用 `AXIsProcessTrustedWithOptions` 触发系统授权弹窗（与现有流程一致）

### Windows / Linux

- Windows UIA 默认无需特殊权限（Low IL 进程即可读）
- Linux AT-SPI2 通常无需权限
- MVP 不实现，故暂不涉及

---

## 9. 测试策略

### 9.1 单元测试（纯函数）

- `classify_app(bundle_id)` → AppKind 映射（覆盖命中/未命中）
- range 切片 + 裁剪逻辑（`extract_surrounding(full_text, range, limit)`）：正常、range 越界、全文短于 limit、空全文
- Terminal scrollback 行数截断逻辑

### 9.2 集成测试

- mock `ContextProvider` trait 注入到 `trigger_action_bar`，验证 ActionBarContext 拼接正确
- mock gather 返回 Err → 验证降级路径（ctx 仅含 text，浮窗照常）

### 9.3 手动验证清单

在以下 App 中各选中一段文字，触发 action bar，检查 `PENDING_CONTEXT` 日志输出的 source/surrounding：

- [ ] TextEdit（Editor 基线）
- [ ] Terminal.app（Terminal scrollback）
- [ ] iTerm2（Terminal，第三方）
- [ ] Safari 文本框（Browser）
- [ ] Word（如有安装——Editor 重点场景）
- [ ] VSCode 编辑器（Editor，Electron 验证 AX 覆盖）
- [ ] 无辅助功能权限场景（验证降级）

---

## 10. 未来扩展（不在 MVP）

1. **方案 C 增强层**：对 AX 覆盖不足的 App 叠加专属取数器（iTerm 全量 scrollback、浏览器 DOM via 剪贴板 hack）
2. **Windows 实现**：UIAutomation COM 绑定
3. **Linux 实现**：AT-SPI2 via `atspi` crate
4. **UI 展示**：浮窗显示来源标签、可展开查看获取到的上下文
5. **按 action 精细化上下文矩阵**：不同 AI 动作携带不同字段的策略表
6. **上下文缓存**：同一应用短时间内重复触发时复用 source 信息（省 AX 调用）
