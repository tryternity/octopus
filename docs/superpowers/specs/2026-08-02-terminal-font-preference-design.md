# 终端字号/字体偏好

> **日期**：2026-08-02
> **关联**：[内嵌终端设计](2026-07-31-embedded-terminal-design.md)

## 目标

终端字体从硬编码（13px SF Mono）改为可配置——用户可在设置页调整字号 + 字体族，终端窗口内快速调字号。

## 背景

现状 `useTerminalSession.ts:38-40` 硬编码：
```typescript
const TERMINAL_FONT_FAMILY = '"SF Mono", Menlo, Monaco, "Cascadia Code", "Roboto Mono", monospace';
const TERMINAL_FONT_SIZE = 13;
```

用户无法调整——固定 13px 对高分屏太小或对老用户太大。

## 范围

### 包含
- AppConfig 加 `terminal_font_size`（f64）+ `terminal_font_family`（String），存 DB `n` 表
- 设置页字号输入 + 字体族下拉（常用等宽字体）+ 自定义输入框
- 终端窗口工具栏字号 +/- 按钮 + `Cmd+=` / `Cmd+-` 快捷键
- 运行时实时生效（`term.options.fontSize` + `fitAddon.fit()` + `term.refresh()`）

### 不包含
- 字重配置（Terax 有，但 octopus 用户无需求）
- 字体预览（设置页只显示字体名）
- 每窗口/tab 独立字体（全局一份）

## 架构

### 配置存储

DB `n` 表（key-value），和 `engine_mode`/`asr_shortcut` 等一致。`config.rs` AppConfig 加两个字段：

```rust
#[serde(default = "default_terminal_font_size")]
pub terminal_font_size: f64,  // 默认 13.0

#[serde(default = "default_terminal_font_family")]
pub terminal_font_family: String,  // 默认 '"SF Mono", Menlo, Monaco, "Cascadia Code", monospace'
```

### 数据流

**加载**（终端窗口打开时）：
```
DB n 表 → load_config() → AppConfig
  → 前端 invoke("get_config") → terminal_font_size / terminal_font_family
  → useTerminalSession 读 config 值替代硬编码常量
```

**设置页改字号/字体**：
```
用户在设置页改字号/字体 → invoke("save_config", { config: {...} })
  → DB n 表更新 → emit("config://changed") 广播
  → 终端窗口 listen 收到 → term.options.fontSize = newSize + fitAddon.fit() + term.refresh()
```

**终端窗口 +/- 快捷键**：
```
用户按 Cmd+= / Cmd+-
  → 本地 state fontSize ±1（clamp 8-32）
  → term.options.fontSize = newSize + fitAddon.fit() + term.refresh()（即时生效）
  → invoke("save_config") 写 DB（持久化，不阻塞 UI）
```

### 运行时实时生效

xterm 6 支持运行时改 `fontSize`/`fontFamily`（不需重建实例）：
```typescript
term.options.fontSize = newSize;
term.options.fontFamily = newFamily;
fitAddon.fit();  // 字号变 → cols/rows 变 → 重适配 + pty.resize
term.refresh(0, term.rows - 1);  // 刷新显示
```

### 字号范围

8-32px，默认 13。超出 clamp 到边界。

### 字体族选项

下拉提供常用等宽字体 + 自定义输入框：
- SF Mono（默认）
- Menlo
- Monaco
- Cascadia Code
- JetBrains Mono
- Fira Code
- Roboto Mono
- 自定义（用户填任意字体名）

**fallback**：用户填的字体系统没装时，xterm 通过 CSS font-family 自动降级到 `monospace`。无需额外处理。

### 终端窗口快捷键

| 快捷键 | 动作 | 范围 |
|---|---|---|
| `Cmd+=` / `Ctrl+=` | 字号 +1 | clamp 32 |
| `Cmd+-` / `Ctrl+-` | 字号 -1 | clamp 8 |

对齐 macOS 标准放大/缩小快捷键（Safari/Terminal 都用这个）。在 keymap.ts 注册。

## 不变量

1. **全局一份配置**——所有终端窗口/tab 共享同一字号/字体
2. **改字号即时生效**——`term.options.fontSize` + `fitAddon.fit()` 不需重建
3. **字号范围 8-32px**——clamp 到边界
4. **字体 fallback**——未安装的字体自动降级 monospace
5. **配置存 DB n 表**——和现有配置一致，不走 config.yaml

## 测试策略

| 单元 | 覆盖 | 方式 |
|---|---|---|
| AppConfig 默认值 | terminal_font_size=13.0, terminal_font_family 默认串 | rust 测试 |
| 字号 clamp | 8-32 范围边界 | 前端纯函数（clampFontSize） |
| 快捷键 | Cmd+= / Cmd+- 触发字号 +/- | keymap.ts 测试 |
| 运行时生效 | term.options.fontSize 更新 + fit | e2e 手动 |

## 风险

1. **config://changed 事件**：需确认现有是否有 config 变化广播机制。如果没有，终端 +/- 时用本地 state + DB 双写（不依赖全局事件）；设置页改字体时需手动刷新终端（或终端窗口每次聚焦时重读 config）。
2. **fitAddon.fit() 的 pty.resize**：字号变 → cols/rows 变 → fitAddon 触发 onResize → pty.resize。这个链路已存在（窗口 resize 时走同样的路径），应该可靠。
3. **自定义字体名安全**：用户可填任意字符串作 fontFamily——xterm 直接设到 CSS，无注入风险（CSS font-family 不执行代码）。
