# 终端字号 / 字体族偏好

> 内嵌终端增强。用户可调整终端字号（8-32）与字体族（系统已安装等宽字体），通过设置页或终端快捷键即时生效，持久化到 DB。

**日期**：2026-08-02
**状态**：✅ 已实现
**关联**：[内嵌终端设计](2026-07-31-embedded-terminal-design.md)

## 目标

终端默认字号 13px / 字体 SF Mono，但用户偏好各异（视力需要大字号、个人审美偏好特定等宽字体）。提供设置页 + 快捷键两入口，运行时即时生效（不需重启终端），跨窗口同步（设置页改了，已打开的终端窗口也跟上）。

## 范围

- ✅ 配置存储：`AppConfig.terminal_font_size`（f64，默认 13）/ `terminal_font_family`（String，默认 "SF Mono"），存 DB `n` 表（与所有 AppConfig 字段一致）
- ✅ 设置页入口（GeneralPanel terminal tab）：字号 slider 8-32 + 字体族下拉 + 预览 + 「恢复默认」按钮
- ✅ 终端快捷键入口：`Cmd/Ctrl + =`/`+` 增大、`Cmd/Ctrl + -` 减小（clamp 8-32）
- ✅ 运行时即时生效：`term.options.fontSize`/`fontFamily` + `fitAddon.fit()` + `refresh()`
- ✅ 字体族变化时 WebGL renderer 重建（dispose + 重新 attach）
- ✅ 跨窗口同步：`set_config` emit `config-changed`，终端窗口 listen 重读
- ✅ 字体族下拉动态加载系统已安装等宽字体（`fc-list`），过滤 `.` 前缀系统隐藏字体
- ❌ 字体粗细 / 斜体 / 行高（YAGNI，xterm 支持但当前无需求）
- ❌ 每终端独立字体（全局一份，简化）
- ❌ 自定义字体输入框（早期版本有，后去掉——下拉已覆盖系统所有等宽字体，自定义多此一举）

## 架构

### 配置层（infra）

`crates/infra/src/config.rs`：

```rust
pub struct AppConfig {
    // ...
    pub terminal_font_size: f64,        // 默认 13.0
    pub terminal_font_family: String,   // 默认 "SF Mono"
}

fn default_terminal_font_family() -> String {
    "SF Mono".to_string()
}
```

**为什么是单个族名而非 CSS 降级链**：早期实现用 `'"SF Mono", Menlo, monospace'` CSS 链作为默认值，但 xterm `fontFamily` 接受 CSS font-family 字符串后内部会自己处理 fallback，存单个族名更清晰、下拉匹配也更直接（用户选 "Menlo" 存的就是 "Menlo"，不是某个 CSS 片段）。浏览器对未知字体自动 fallback 到 monospace。

### 后端命令

**`list_monospace_fonts`**（`crates/desktop/src/commands/settings_commands.rs`）：

```rust
#[tauri::command]
pub fn list_monospace_fonts() -> Result<Vec<String>, String> {
    // fc-list :spacing=mono family → 拉系统等宽字体族名
    // 过滤 "." 前缀（.Apple Color Emoji UI / .LastResort / .SF NS Mono / .Times LT MM
    //   等系统隐藏/特殊字体，非真实等宽，xterm 选中后字符 atlas 错乱字变小）
    // fallback：fc-list 不可用时返回 macOS 常见白名单
}
```

**为什么过滤 `.` 前缀**：macOS fontconfig 会列出 `.SF NS Mono`、`.Apple Color Emoji UI`、`.LastResort`、`.Times LT MM` 等系统内部字体。它们不是用户可选的真实等宽字体——xterm WebGL renderer 选中后字符 atlas 渲染错乱（字变小 + 间距大）。这些是系统为自身 UI 保留的特殊字体变体，不应出现在用户字体选择列表里。

**`set_config`**（已有命令）：写入 DB 后 emit `"config-changed"` 全局事件，已打开的终端窗口 listen 后重读配置即时更新。

### 前端

**`pages/Terminal/useTerminalSession.ts`**：

```typescript
setFontSize: (size: number) => {
  term.options.fontSize = size;
  fitAddonRef.current?.fit();       // 字号变 → cols/rows 可能变
  term.refresh(0, term.rows - 1);
},
setFontFamily: (family: string) => {
  term.options.fontFamily = family;
  // ⚠️ WebGL renderer 缓存了旧字体的字符 atlas——fontFamily 变化必须 dispose + 重新 attach，
  // 否则 atlas 不重建，渲染的字宽错乱（字变小 + 间距大）。
  if (webglRef.current) {
    try { webglRef.current.dispose(); } catch { /* already disposed */ }
    webglRef.current = null;
  }
  webglRef.current = attachWebgl(term, webglRef);
  fitAddonRef.current?.fit();
  term.refresh(0, term.rows - 1);
},
```

**关键约束——字体族变化必须重建 WebGL renderer**：字号变化只需 `setFontSize`（atlas 按比例缩放），但**字体族变化必须 dispose + 重新 attachWebgl**。WebGL renderer 内部维护一个字符 atlas（glyph texture cache），切换字体族后旧 atlas 字形仍是旧字体的，不重建会渲染错乱。曾因遗漏此步导致「字体选了以后，终端显示就异常了，显示的字变的很小，且字和字间距很大」。

**`pages/Terminal/index.tsx`**：

- mount 时 `invoke("get_config")` 读字号/字体 → setState
- `listen("config-changed")` 跨窗口同步（设置页改了 → emit → 终端窗口重读）
- `handleFontResize(delta)`：`Cmd/Ctrl + =`/`-` 快捷键 → clamp 8-32 + setState + `set_config` 持久化（fire-and-forget）
- `fontSizeRef` 持有最新值（避免连按 `+` 累积滞后）

**`pages/Settings/GeneralPanel.tsx`**（terminal tab）：

- 字号 slider（8-32，整数 step）
- 字体族 dropdown：`invoke("list_monospace_fonts")` mount 时拉系统字体列表
- 预览行：用当前字号/字体渲染样例文字 `"The quick brown fox 123"`
- 「恢复默认」按钮：仅在字号≠13 或字体≠"SF Mono" 时显示（避免无意义点击），点击 `setVal("terminal_font_size", 13)` + `setVal("terminal_font_family", "SF Mono")`

**默认值单一真相源**：后端 `default_terminal_font_family()` 返回 "SF Mono"，前端常量 `TERMINAL_FONT_FAMILY_DEFAULT = "SF Mono"` / `TERMINAL_FONT_SIZE_DEFAULT = 13` 与之对齐。改默认值时两处都要改。

## 不变量

1. **字号范围 8-32**：slider 边界 + 快捷键 clamp 都用同一对常量（前端 `MIN_FONT_SIZE`/`MAX_FONT_SIZE`）
2. **字体族存单个族名**：不存 CSS 降级链，浏览器自动 fallback monospace
3. **字体族变化必须重建 WebGL renderer**：dispose + 重新 attachWebgl（字号变化不需要）
4. **跨窗口同步靠 `config-changed` 事件**：设置页 `set_config` → emit → 终端窗口 listen → 重读 `get_config`

## 降级路径

- `fc-list` 不可用（非 macOS 或未装 fontconfig）→ 返回 macOS 常见白名单（Andale Mono / Courier New / Menlo / Monaco / PT Mono / SF Mono）
- 用户存了旧格式 CSS 降级链（如 `'"SF Mono", Menlo, ...'`）→ dropdown `value` 匹配不到任何系统字体 → 回退到首项（首个系统等宽字体）
- `get_config` 失败（终端窗口未注册命令）→ catch 静默降级用默认字体

## 演进历史

1. **v1（初始）**：硬编码 CSS 降级链 `'"SF Mono", Menlo, Monaco, "Cascadia Code", "Roboto Mono", monospace'`
2. **v2**：AppConfig 加配置字段，设置页固定 7 预设 + 自定义输入框
3. **v3（当前）**：去掉固定预设 + 自定义，下拉动态加载 `fc-list` 系统字体（过滤 `.` 前缀）+ 「恢复默认」按钮。理由：固定预设不覆盖用户已装的编程字体（JetBrains Mono / Fira Code 等），自定义输入框又多一个 UI 路径——直接列系统所有等宽字体最简洁
