# 终端字号/字体偏好 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 终端字体从硬编码改为可配置——设置页调字号/字体族 + 终端窗口 Cmd+=/- 快速调字号。

**Architecture:** AppConfig 加两字段存 DB → 前端 get_config 读 + set_config 写 → Terminal 运行时改 options.fontSize/fontFamily + fitAddon.fit()。

**Tech Stack:** Rust（config.rs）+ TypeScript（React + xterm.js），零新依赖。

**Spec:** `docs/superpowers/specs/2026-08-02-terminal-font-preference-design.md`

## Global Constraints

- **配置存 DB n 表**（和 engine_mode/asr_shortcut 一致），通过 `get_config`/`set_config` 读写
- **字号范围 8-32px**，默认 13，clamp 到边界
- **字体族默认** `'"SF Mono", Menlo, Monaco, "Cascadia Code", "Roboto Mono", monospace'`
- **运行时实时生效**：`term.options.fontSize` + `fitAddon.fit()` + `term.refresh()`，不重建实例
- **全局一份配置**——所有终端窗口/tab 共享
- **终端快捷键不区分平台**：Cmd 或 Ctrl += / - 都支持（对齐 Cmd+T 的 keymap 模式）
- **set_config 的 key 是 snake_case**（`terminal_font_size`/`terminal_font_family`），Tauri 自动 camelCase 映射

## File Structure

| 文件 | 责任 | 动作 |
|---|---|---|
| `crates/infra/src/config.rs` | AppConfig 加 terminal_font_size + terminal_font_family + default 函数 | 修改 |
| `crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts` | 读 config 替代硬编码 + 暴露 setFont 运行时改字体方法 | 修改 |
| `crates/desktop/frontend/src/pages/Terminal/index.tsx` | Cmd+=/- 快捷键 + get_config 读字体 + 工具栏字号显示 | 修改 |
| `crates/desktop/frontend/src/pages/Terminal/keymap.ts` | isFontShortcut 纯函数（Cmd/Ctrl +=/-） | 修改 |
| `crates/desktop/frontend/src/pages/Settings/` | 设置页字号 + 字体族 UI | 修改 |

---

### Task 1: AppConfig 加 terminal_font_size + terminal_font_family

**Files:**
- Modify: `crates/infra/src/config.rs`

**Interfaces:**
- Produces: AppConfig 加两字段，DB 自动持久化（n 表 serde 序列化）+ get_config 返回。

- [ ] **Step 1: 加字段 + default 函数**

在 AppConfig struct（`config.rs:56` 附近）末尾加：

```rust
    /// 终端字号（px）。默认 13。
    #[serde(default = "default_terminal_font_size")]
    pub terminal_font_size: f64,

    /// 终端字体族（CSS font-family 字符串）。默认 SF Mono 系列。
    #[serde(default = "default_terminal_font_family")]
    pub terminal_font_family: String,
```

在 default 函数区加：

```rust
fn default_terminal_font_size() -> f64 { 13.0 }
fn default_terminal_font_family() -> String {
    r#""SF Mono", Menlo, Monaco, "Cascadia Code", "Roboto Mono", monospace"#.to_string()
}
```

- [ ] **Step 2: 确认 apply_config_value 是否需加分支**

Run: `rg -n "apply_config_value" crates/desktop/src/`

`set_config` 命令对已知 AppConfig 字段调 `apply_config_value`（处理副作用如快捷键重注册）。terminal_font_size/family 无副作用（不需重注册什么），但需确认 `apply_config_value` 不会因「未知字段」报错。若它有 match 分支 + 兜底，加一个 `_ => Ok(())` 或显式 `terminal_font_size | terminal_font_family => Ok(())` 分支。

- [ ] **Step 3: 编译 + 测试**

```bash
cargo build --release -p octopus-desktop 2>&1 | tail -3
cargo test -p octopus-infra --lib 2>&1 | tail -3
```
Expected: build 0 error；infra test 全过。

- [ ] **Step 4: Commit**

```bash
git add crates/infra/src/config.rs
git commit -m "feat(config): AppConfig 加 terminal_font_size + terminal_font_family"
```

---

### Task 2: useTerminalSession 读 config + 运行时改字体

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts`

**Interfaces:**
- Produces: session 暴露 `setFontSize(size)` / `setFontFamily(family)` 方法，供 index.tsx 快捷键和 config 变化时调。

- [ ] **Step 1: 删硬编码常量，改为参数**

删 `TERMINAL_FONT_FAMILY` / `TERMINAL_FONT_SIZE` 常量（line 38-40）。useTerminalSession 的 opts 加 `fontSize?: number` + `fontFamily?: string`：

```typescript
// useTerminalSession opts 加：
fontSize?: number;
fontFamily?: string;
```

Terminal 构造（line 153-154）改用 opts：
```typescript
const term = new Terminal({
  fontFamily: opts.fontFamily ?? '"SF Mono", Menlo, Monaco, "Cascadia Code", monospace',
  fontSize: opts.fontSize ?? 13,
  // ... 其余不变
```

- [ ] **Step 2: session 暴露 setFontSize / setFontFamily 运行时改字体方法**

在 return 对象加：

```typescript
setFontSize: (size: number) => {
  const term = termRef.current;
  if (!term) return;
  term.options.fontSize = size;
  fitAddonRef.current?.fit();
  term.refresh(0, term.rows - 1);
},
setFontFamily: (family: string) => {
  const term = termRef.current;
  if (!term) return;
  term.options.fontFamily = family;
  term.refresh(0, term.rows - 1);
},
```

注意：`fitAddonRef` 需确认是否存在（或用闭包的 fitAddon 变量）。若 fitAddon 是闭包内变量，直接用。

TerminalSession 类型加 `setFontSize` / `setFontFamily`。

- [ ] **Step 3: tsc + vitest**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npx vitest run
```

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts
git commit -m "feat(terminal): useTerminalSession 读 config 字体 + 运行时改字体方法"
```

---

### Task 3: index.tsx Cmd+=/- 快捷键 + get_config 读字体

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Terminal/index.tsx`
- Modify: `crates/desktop/frontend/src/pages/Terminal/keymap.ts`

- [ ] **Step 1: keymap.ts 加 isFontShortcut 纯函数**

```typescript
/** Cmd/Ctrl + = / - 快捷键（字号 +/-）。不区分平台。 */
export function isFontShortcut(e: { metaKey: boolean; ctrlKey: boolean; key: string }): "increase" | "decrease" | null {
  if (!(e.metaKey || e.ctrlKey)) return null;
  if (e.key === "=" || e.key === "+") return "increase";
  if (e.key === "-") return "decrease";
  return null;
}
```

加测试（keymap.test.ts）：
```typescript
it("Cmd+= → increase", () => { /* ... */ });
it("Ctrl+- → decrease", () => { /* ... */ });
it("无修饰 += → null", () => { /* ... */ });
```

- [ ] **Step 2: index.tsx 加 get_config 读字体 + 传给 useTerminalSession**

index.tsx mount 时读 config：
```typescript
const [fontSize, setFontSize] = useState(13);
const [fontFamily, setFontFamily] = useState('"SF Mono", Menlo, Monaco, "Cascadia Code", monospace');

useEffect(() => {
  invoke<{ config: Record<string, unknown> }>("get_config").then((res) => {
    const cfg = res.config;
    if (cfg.terminal_font_size) setFontSize(cfg.terminal_font_size as number);
    if (cfg.terminal_font_family) setFontFamily(cfg.terminal_font_family as string);
  }).catch(() => {});
}, []);
```

TerminalPane 传 fontSize + fontFamily props（透传给 useTerminalSession）。

- [ ] **Step 3: index.tsx 快捷键处理**

在 keydown handler（或 xterm attachCustomKeyEventHandler）加：
```typescript
const fontAction = isFontShortcut(e);
if (fontAction) {
  e.preventDefault();
  const next = Math.max(8, Math.min(32, fontSize + (fontAction === "increase" ? 1 : -1)));
  setFontSize(next);
  // 即时生效
  activeTerminalSession?.setFontSize(next);
  // 持久化（fire-and-forget）
  invoke("set_config", { key: "terminal_font_size", value: next }).catch(() => {});
}
```

- [ ] **Step 4: tsc + vitest**

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/
git commit -m "feat(terminal): Cmd+=/- 快捷键调字号 + get_config 读字体配置"
```

---

### Task 4: 设置页字号 + 字体族 UI

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/`（SystemPanel 或 General tab）

- [ ] **Step 1: 加字号输入 + 字体族下拉 + 自定义**

在设置页加：
- 字号：数字输入（8-32）或 slider，onchange 调 `set_config`
- 字体族：`<select>` 下拉（SF Mono/Menlo/Monaco/Cascadia/JetBrains/Fira/Roboto/自定义）+ 自定义时显示 `<input>`

onchange 调：
```typescript
await invoke("set_config", { key: "terminal_font_size", value: newSize });
// 或
await invoke("set_config", { key: "terminal_font_family", value: newFamily });
```

- [ ] **Step 2: tsc + vitest + build**

- [ ] **Step 3: Commit**

---

### Task 5: e2e 验证 + 文档同步

- [ ] **Step 1: e2e**
  - 终端 Cmd+= / Cmd+- 字号实时变化
  - 设置页改字号/字体 → 重开终端生效
  - 字号 8-32 clamp
  - 自定义字体名 fallback

- [ ] **Step 2: architecture.md + research 同步**

- [ ] **Step 3: Commit + Review plan**

---

## Self-Review 记录

**Spec 覆盖**：
- ✅ AppConfig 两字段 → Task 1
- ✅ useTerminalSession 读 config + 运行时改 → Task 2
- ✅ Cmd+=/- 快捷键 → Task 3
- ✅ 设置页 UI → Task 4
- ✅ 字号 8-32 clamp → Task 3 setFontSize

**已知实现注意**：
- Task 2 的 `fitAddonRef`：需确认 useTerminalSession 是否有 fitAddon 的 ref（或用闭包变量）。fitAddon 在 `term.open().then()` 回调内创建，可能是闭包变量——setFontSize 方法也在 return 对象（闭包内），应该能访问。
- Task 3 的 `activeTerminalSession`：index.tsx 需要拿到当前活跃 tab 的 session 对象。可能通过 TerminalPane 的 onPtyId 或新增 ref 上报 session 引用。
- Task 4 的设置页位置：需确认 SystemPanel 还是其他 tab 最合适。
