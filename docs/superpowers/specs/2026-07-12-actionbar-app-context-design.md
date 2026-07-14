# Action Bar 应用上下文获取设计

> **状态**：已实现
> **日期**：2026-07-12
> **scope**：在 action bar 触发时，根据操作系统获取选中文本所在应用的上下文信息（来源应用、前后文、窗口标题），让 LLM 动作具备情境感知能力
> **前置文档**：[`2026-07-12-action-bar-command-shortcut-design.md`](./2026-07-12-action-bar-command-shortcut-design.md)（action bar DB 化与菜单架构）

---

## 1. 背景与动机

### 1.1 现状（改造前）

action bar 触发流程（`trigger_action_bar`，仅 macOS）：

1. 热键 → 记录剪贴板快照
2. osascript 模拟 `Cmd+C`
3. 读剪贴板拿到选中文本 `text`
4. `ActionBarContext { text }` 暂存到全局 `PENDING_CONTEXT`
5. 显示浮窗

**问题**：`ActionBarContext` 只有裸选中文本，LLM 不知道这段文字来自 Word 还是终端还是浏览器，无法利用前后文做更智能的操作。

### 1.2 设计约束（已确认）

| 决策点 | 选择 |
|--------|------|
| 技术方案 | **平台无障碍 API + Browser AppleScript JS（macOS）/ UIAutomation（Windows）** |
| 平台范围 | **macOS + Windows**；Linux 暂不支持（回退 NullProvider，AT-SPI2 需事件流，v2 待 atspi crate） |
| 失败行为 | **降级到现状**：上下文获取失败绝不阻塞浮窗显示，`source`/`surrounding` 为 None 时行为等同改造前 |
| Terminal scrollback | 最近 **30 行 / 1000 字** 截断 |
| Editor 前后文裁剪 | before/after 各 **1000 字** 截断 |
| 权限 | 无新增——复用辅助功能权限 + 浏览器「允许 Apple 事件执行 JavaScript」 |

---

## 2. 方案选择与演进

### 2.1 初始选型：AX（方案 B）

| 方案 | 机制 | 优点 | 短板 |
|------|------|------|------|
| A. 每 App 写脚本 | 每 App 专属 AppleScript | 数据最丰富 | N² 维护 |
| **B. AX** ✅ | `AXFocusedUIElement` → `AXValue` + `AXSelectedTextRange` | 一套代码覆盖所有原生 App | 自绘编辑器/复杂 DOM 覆盖不足 |
| C. B + A 增强层 | AX 默认 + 专属取数器 | 覆盖率最高 | 维护重 |

### 2.2 实际演进：AX + Browser AppleScript JS

实际测试发现 AX 对两类 App 覆盖不足：

- **自绘编辑器**（Sublime Text、WPS）：AX 树只有 `AXStaticText`（如 "UNREGISTERED" 水印），`AXValue` 不含真实编辑器内容 → 内容校验降级返回 None
- **浏览器**（Chrome）：AX 树是 DOM 的简化投影，复杂页面定位不准

Browser 改为通过 **AppleScript execute javascript** 直接读 DOM：

- Chrome/Edge: `execute (active tab of window 1) javascript jsCode`
- Safari: `do JavaScript jsCode in document 1`
- Firefox: 无此接口，fallback 到 AX
- JS 源码写入临时文件（`/tmp/octopus_browser_context.js`），AppleScript `read POSIX file` 读入——彻底避免引号转义

---

## 3. 数据模型

### 3.1 ActionBarContext 升级

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionBarContext {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::app_context::AppSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surrounding: Option<crate::app_context::SurroundingText>,
}
```

### 3.2 app_context 类型（mod.rs）

```rust
pub enum AppKind { Editor, Terminal, Browser, Chat, Unknown }

pub struct AppSource {
    pub bundle_id: Option<String>,
    pub name: String,
    pub kind: AppKind,
}

pub struct SurroundingText {
    pub before: Option<String>,
    pub after: Option<String>,
    pub window_title: Option<String>,
}

pub struct ExtraContext {
    pub source: AppSource,
    pub surrounding: Option<SurroundingText>,
    pub diagnostics: Option<String>,  // AX 诊断信息（不序列化，仅写日志）
}
```

`source`/`surrounding` 均为 `Option` + `skip_serializing_if`，纯增量向后兼容。

---

## 4. 平台抽象（trait + cfg 分发）

### 4.1 ContextProvider trait

```rust
pub trait ContextProvider {
    fn gather(&self, selected_text: &str) -> anyhow::Result<ExtraContext>;
}
```

`selected_text` 用于：(1) 内容校验（AX full_text 不含选中文本 → 降级）；(2) Browser DOM 搜索定位；(3) Terminal before/after 切分。

便捷封装：`gather_context(selected_text)` = `provider().gather(selected_text)`。

### 4.2 模块布局

```
crates/desktop/src/app_context/
├── mod.rs          # 类型 + trait + classify_app（三平台进程名，内部统一 to_ascii_lowercase）+ provider() 工厂
├── ffi.rs          # AXUIElement C FFI 声明 + AX 属性名 CFString（macOS only）
├── macos_ax.rs     # macOS 实现：NSWorkspace + AX + Browser AppleScript JS（macOS only）
├── windows_uia.rs  # Windows 实现：IUIAutomation + TextPattern/ValuePattern + TreeWalker（Windows only）
└── linux_atspi.rs  # 设计参考（暂不编译）——AT-SPI2 需事件流，v2 待 atspi crate
```

**各平台 API 对应**：

| 能力 | macOS | Windows | Linux |
|------|-------|---------|-------|
| 前台应用 | NSWorkspace.frontmostApplication | GetForegroundWindow + QueryFullProcessImageNameW | ❌ 暂不支持 |
| 焦点元素 | AXFocusedUIElement | IUIAutomation.GetFocusedElement | ❌ 暂不支持 |
| 文本内容 | AXValue | TextPattern.DocumentRange.GetText 或 ValuePattern | ❌ 暂不支持 |
| 选区定位 | AXSelectedTextRange（坐标系不一致→改用 find） | 全文 find（GetSelection + ExpandToEnclosingUnit 留作 v2） | ❌ 暂不支持 |
| 浏览器 | AppleScript execute javascript（精准 DOM 定位） | UIA TextPattern 全文 find（无 AppleScript 等价物，复杂页面精度降级） | ❌ 暂不支持 |

### 4.3 trigger_action_bar 集成（异步架构）

浮窗显示和上下文采集**异步分离**——浮窗先弹（仅 text），后台线程采集上下文完成后回填：

```rust
// 1. 暂存基础 ctx（仅 text），浮窗 mount 时能立即拿到
*PENDING_CONTEXT.lock().unwrap() = Some(ActionBarContext {
    text: text.clone(), source: None, surrounding: None,
});

// 2. 浮窗先弹（主线程）
app.run_on_main_thread(move || show_action_bar_window(&app, win_x, win_y));

// 3. 后台线程采集上下文，完成后回填 PENDING_CONTEXT
std::thread::spawn(move || {
    match gather_context(&text) {
        Ok(extra) => {
            // 回填前校验归属——防止跨触发污染
            if ctx.text == text_for_gather {
                ctx.source = Some(extra.source);
                ctx.surrounding = extra.surrounding;
            }
        }
        Err(e) => log::warn!(...),
    }
});
```

**关键设计点**：
- gather 不阻塞浮窗显示（osascript 卡住不影响用户体验）
- 回填时校验 `ctx.text == text_for_gather`，防止用户 dismiss 后重触发时旧 gather 线程污染新 ctx
- 浮窗 mount 时拿到基础 ctx（仅 text），execute_action_bar 时拿到完整 ctx（含 source/surrounding）

---

## 5. macOS 取数路径（双路径）

### 5.1 路径选择

```
Browser (Chrome/Edge/Safari)
  ├─ 首选：AppleScript execute javascript → 读真实 DOM
  └─ 降级：AX 遍历（AppleScript 失败时）

非 Browser
  └─ AX: NSWorkspace + AXUIElement
```

### 5.2 AX 取数（macos_ax.rs）

1. **NSWorkspace.frontmostApplication** → pid + bundleId + name
2. **classify_app(bundle_id)** → AppKind
3. **AXUIElementCreateApplication(pid)** → app element
4. **AXFocusedUIElement** → focused element（`-25212` 时 200ms 重试一次，受 deadline 约束）
5. **焦点元素角色** (`AXRole`) 判断是否文本元素（AXTextArea/AXTextField/AXStaticText）
6. 非文本元素 → **find_text_element** 遍历 AX 子树（深度 8 层、广度 100），优先找 `AXValue` 包含 selected_text 的文本元素。**每层递归入口检查 deadline**，超时即返回已采集部分
7. **AXValue** + **AXSelectedTextRange**（经 `is_cf_value` 类型守卫）→ 切 before/after
8. **内容校验**（仅 `kind == Editor`）：`full_text` 为空（WPS AX 返回 -25212 禁用）**或**不包含选中文本 → 触发自绘编辑器 fallback 链（见 §5.5，按 Sublime 插件 → WPS lsof → 磁盘 顺序）。Terminal 排除：full_text 是真实 scrollback，选中文本可能在不可见区域（光标行以下），find-fail fallback 应取 scrollback 末尾作 before

**deadline 透传**：`Instant` 从 `gather()` 创建，透传到 `gather_surrounding` → `build_surrounding` → `find_text_element_depth`，确保庞大的 AX 树（Electron App）不会卡死。

### 5.3 Browser AppleScript JS（macos_ax.rs::gather_browser_via_applescript）

JS 源码写入唯一临时文件（`pid + 纳秒时间戳`），AppleScript `read POSIX file` 读入后执行。RAII guard 确保所有路径（含 spawn 失败/超时）Drop 时删除文件，防止选中文本明文残留。

osascript 通过 `spawn()` + `Stdio::piped()` + `try_wait` 轮询超时执行（与 `spawn_script` 的 `wait_with_timeout_secs` 范式一致）。超时 kill 后回收僵尸进程，stdin/stdout/stderr 管道由独立线程并发读取防 pipe 满阻塞。

- Chrome/Edge: `execute (active tab of window 1) javascript jsCode`
- Safari: `do JavaScript jsCode in document 1`
- Firefox: 不支持，return None → fallback AX

**JS 逻辑**：

1. TreeWalker 遍历 `document.body` 所有文本节点，找到包含 selected_text 的节点
2. 从该节点向上溯源 parentNode，直到 textContent ≥ `2000 + sel.length` 字或到 body
3. 在 scope 的 textContent 内用 `findIn()` 模糊匹配定位选中文本：
   - 完整精确 → 完整忽略大小写 → 中间 60% 子串 → 首尾 30 字符联合 → 末尾 30 字符
4. 按 idx 切 before/after 各 1000 字

**前提**：用户需在浏览器中开启「允许 Apple 事件执行 JavaScript」。

### 5.4 Terminal 特例（AX 路径）

iTerm2 的 `AXSelectedTextRange` 基于整个终端缓冲区，与 `AXValue`（可见 scrollback）不在同一坐标系。改为用 selected_text 在 full_text 中 `find()` 定位，按位置切 before/after。Terminal 排除内容校验（选中文本可能在不可见区域，find-fail fallback 取 scrollback 末尾）。

### 5.5 自绘编辑器取数（macOS）

自绘编辑器（Sublime Text、WPS、Pages）的 AX 树不含真实编辑器内容。当 §5.2 第 8 点内容校验触发（`full_text` 空 / 不含选中文本）时，按 bundle 匹配优先级走 fallback 链（`build_surrounding`，仅 `kind == Editor`，`bundle_id` 大小写不敏感匹配）：

1. `bundle.contains("sublimetext")` → **Sublime 插件取数器**（`sublime_plugin.rs`）
2. `bundle.contains("iwork")` → **Pages AppleScript**（`try_pages_applescript`）
3. 所有 Editor → **lsof 文件路径取数**（`try_lsof_context`，泛化，不再限 WPS）
4. 有 `window_title` → **窗口标题磁盘 fallback**（所有 Editor）

**1. Sublime 插件取数器**（`sublime_plugin.rs`，仅 `com.sublimetext.*`）

1. 自动安装插件到 `Packages/Octopus/octopus_context.py`（首次运行，内容比对确保最新）
2. 触发 `subl --command octopus_export_context`
3. 插件从 Sublime Python API 读 view 全文 + 选区位置 → 写 `/tmp/octopus_sublime_ctx.json`
4. 读取 JSON → 用选区位置精确切 before/after 各 1000 字
5. **对未保存文件（untitled）同样有效**——插件直接读 view 内容

**2. Pages AppleScript**（`try_pages_applescript`，仅 `com.apple.iWork.*`）

Pages 是 Apple 原生 App，AppleScript 支持 `body text of document 1`。
AX 失败时（-25205 CannotComplete）直接 AppleScript 读全文 → `slice_around_text` 切上下文。

**3. lsof 文件路径取数**（`try_lsof_context`，所有 Editor）

窗口标题不可靠时（WPS 空 / Pages 显示 App 名），用 `lsof -c <proc_name> -F n` 列出进程打开的文件：
- `bundle_id_to_procname` 映射：`kingsoft` → `wpsoffice`、`iwork.pages` → `Pages` 等
- `filter_lsof_doc_files` 纯函数筛选 `.docx/.xlsx/.pptx/.pdf/.doc/.xls/.ppt/.pages`，排除 `/dev/`、`.~`
- 逐个 `read_file_as_text` → `slice_around_text` 用选中文本匹配

**4. 窗口标题磁盘 fallback**（所有 Editor）

1. `extract_filename_from_title("test.txt — Sublime Text")` → `"test.txt"`
2. `find_file_path(filename, bundle_id)`：Sublime session（`find_file_in_session_json` 纯函数，多窗口遍历）→ `mdfind -name` Spotlight
3. `read_file_as_text(path)` → `slice_around_text` 切 before/after 各 1000 字

**`read_file_as_text` 支持的格式**：

| 格式 | 优先工具 | Fallback |
|------|---------|----------|
| `.docx`/`.xlsx`/`.pptx` | **officecli**（`officecli view file text`，处理修订/批注/公式/图表） | 内置 zip+XML 解析 |
| `.pdf` | **pdftotext** + `merge_cjk_line_breaks`（CJK 排版换行合并） | None |
| `.pages` | zip → `Index/Document.iwa` → Protobuf 明文碎片提取 | None |
| `.txt`/`.md`/`.rs` 等 | 直接读取（UTF-8 lossy） | — |

**`slice_around_text` 三轮匹配**：

1. 精确匹配（`find`）
2. 忽略大小写匹配
3. **NFKC 归一化匹配**（`unicode-normalization` crate）——WPS Cmd+C 产生康熙部首（U+2Exx）和全角标点（U+FFxx），pdftotext 提取的是正常 Unicode。归一化后只保留 CJK + ASCII 字母数字比较。一次性构建 `norm_to_byte[]` 映射表，O(n) 定位回原文 byte offset。

**各编辑器覆盖情况**：

| 编辑器 | fallback 路径 | 备注 |
|--------|-------------|------|
| Sublime Text | 插件取数器（最可靠，含未保存） | session 磁盘 fallback 备用 |
| Pages | AppleScript `body text`（最可靠） | lsof .pages 备用 |
| WPS Office | lsof + officecli/pdftotext | 窗口标题通常空 |
| 其他 Editor | lsof → 窗口标题磁盘 fallback | 取决于 App |

**降级诊断**：区分无窗口标题 / 标题无法提取文件名 / 文件未找到 / 读取失败 / 内容不含选中文本。

### 5.6 AX 安全约束（踩坑总结）

| 风险 | 防护 |
|------|------|
| AX 返回非 CFString 被当 CFString 解转 → NSException 崩溃 | `is_cf_string()` CFTypeID 检查 |
| AXChildren 返回 CFBoolean 被当 CFArray 解转 → 崩溃 | `is_cf_array()` CFTypeID 检查 |
| AXSelectedTextRange 返回非 AXValue 被当 AXValueGetValue → UB | `is_cf_value()` AXValueGetTypeID 检查 |
| CFArray Drop 后子元素被释放 → use-after-free | `CFRetain` 返回的子元素 |
| AXValue 检查后不释放 → 内存泄漏 | `Ok(v) => { CFRelease(v); }` |
| 终端 scrollback 含 `\0` 控制字符 | `strip_control_chars()` 过滤 C0（保留 \n \t \r） |
| AXSelectedTextRange 反向选区 → before/after 重叠 | `extract_surrounding` 归一化 `end = end.max(start)` |
| 后台 gather 线程跨触发污染新 ctx | 回填前校验 `ctx.text == text_for_gather` |
| osascript 挂起 → 后台线程泄漏 | `spawn` + `try_wait` 轮询 + `deadline` 超时 `kill` |
| JS 临时文件明文残留 | RAII guard（Drop 删文件）+ 唯一文件名（纳秒时间戳）|

---

## 6. bundle id → AppKind 映射

```rust
fn classify_app(bundle_id: &str) -> AppKind {
    match bundle_id {
        "com.apple.Terminal" | "com.googlecode.iterm2" => Terminal,
        "com.microsoft.Word" | "com.apple.TextEdit"
          | "com.sublimetext.4" | "com.sublimetext.3"
          | "com.microsoft.VSCode" | "com.todesktop.230313mzl4w4u92"
          | "com.github.atom" | "com.kingsoft.wpsoffice.mac" => Editor,
        "com.apple.Safari" | "com.google.Chrome"
          | "org.mozilla.firefox" | "com.microsoft.edgemac" => Browser,
        "com.tencent.xinWeChat" | "com.tinyspeck.slackmacgap"
          | "com.hnc.Discord" => Chat,
        _ => Unknown,
    }
}
```

---

## 7. 上下文日志

采集到的上下文写入 `~/.octopus/logs/action-bar.log`（append 模式），每条含：

- 时间戳、应用名/BundleID/类别、窗口标题
- 选中文本（截断 1000 字）、上文 before（截断 500 字）、下文 after（截断 500 字）
- AX 诊断信息（focused_role/child_role/ax_value_len/selected_range/full_text_preview/降级原因）

代码：`log_app_context()` → `format_context_entry()`（纯函数）+ `write_context_log()`（IO）。

---

## 8. LLM prompt 上下文注入

`build_enriched_text()` 将 source/surrounding 拼成情境块追加到选中文本前：

```
【来源】TextEdit（编辑器）
【窗口】report.txt
【上文】...
【下文】...

【选中文本】{text}
```

- LLM 翻译 + 润色/摘要/解释：注入 enriched_text
- 本地翻译引擎（opus-mt/m2m100）：**不注入**（NMT 不需上下文）
- `action_bar_show_result` 的 `original_text` 仍传原始 `text`

---

## 9. 失败与降级

| 故障 | 行为 |
|------|------|
| Linux 平台 | `NullProvider` 返回 Err，行为等同改造前 |
| 无辅助功能权限 | AX 调用返回错误码 → gather 返回 Err → 降级 |
| Chrome `-25212` | 200ms 后重试一次；仍失败 → fallback AX 或 Browser JS |
| 自绘编辑器（Sublime/WPS） | full_text 空/不含选中文本 → 触发 fallback 链（Sublime 插件 → WPS lsof → 磁盘） |
| AX 调用超时（>500ms） | 返回已采集的部分字段 |
| Browser AppleScript 失败 | fallback 到 AX |
| gather 整体 Err | 记日志，ctx 仅含 `text`，**浮窗照常显示** |

**最高优先级不变量**：上下文获取的任何失败都不得影响 action bar 核心流程。

---

## 10. 已知限制与未来扩展

| 限制 | 现状 | v2 方向 |
|------|------|---------|
| Sublime Text | ✅ 插件取数器（subl --command + Python API）+ 磁盘 fallback | — |
| WPS Office | AX 禁用 + 无 AppleScript + 无插件 API + 窗口标题通常为空；**lsof 取进程打开文件路径**（绕过空标题）+ officecli/OOXML 提取 .docx/.xlsx/.pptx + pdftotext 提取 .pdf | WPS 插件 API（如有） |
| Firefox (macOS) | 无 AppleScript JS 接口，走 AX（Mozilla AX 比 Chrome 完整） | 浏览器扩展 + WebSocket |
| Windows 浏览器 | UIA TextPattern 全文 find，无 AppleScript JS 精度 | GetSelection + ExpandToEnclosingUnit 段落级精准扩展 |
| Windows 全文 find | selected_text 多次出现时命中第一次，可能错位 | GetSelection 精确选区 + range 扩展 |
| Linux | 暂不支持（AT-SPI2 需事件流） | atspi crate + object:state-change:focused 事件 |
| 前端来源标签 | 已移除（挤压菜单布局） | 浮窗 tooltip 或更紧凑样式 |
| 上下文 UI 可见性 | 仅写日志文件 | 浮窗展开查看获取到的上下文 |

---

## 11. 测试覆盖

| 层 | 测试 | 数量 |
|----|------|------|
| 纯函数（mod.rs） | classify_app / extract_surrounding | 5 |
| AX 类型安全（macos_ax.rs） | is_cf_string / is_cf_array / is_cf_value | 11 |
| AX 错误码翻译 | ax_error_desc | 2 |
| 控制字符过滤 | strip_control_chars | 5 |
| Terminal 截断 | truncate_text_tail / truncate_text_head | 6 |
| 日志格式化/写入（action_bar_commands.rs） | format_context_entry / write_context_log | 12 |
| 磁盘 fallback 标题提取 | extract_filename_from_title | 9 |
| 磁盘 fallback 切片 | slice_around_text（char-level，含 CJK/大小写/limit/多行） | 11 |
| 磁盘 fallback 端到端 | 真实文件/大文件截断/多行选中 | 3 |
| session JSON 解析 | find_file_in_session_json / path_matches_filename（多窗口/后缀防护/精确匹配） | 10 |
| App 名判定 | name_result_is_app_name | 2 |
| **合计** | | **76** |
