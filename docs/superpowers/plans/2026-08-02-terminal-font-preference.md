# 终端字号/字体偏好 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 终端字体从硬编码改为可配置——设置页调字号/字体族 + 终端窗口 Cmd+=/- 快速调字号。

**Architecture:** AppConfig 加两字段存 DB → 前端 get_config 读 + set_config 写 → Terminal 运行时改 options.fontSize/fontFamily + fitAddon.fit()。

**Tech Stack:** Rust（config.rs）+ TypeScript（React + xterm.js），零新依赖。

**Spec:** `docs/superpowers/specs/2026-08-02-terminal-font-preference-design.md`

## Global Constraints

- **配置存 DB n 表**（和 engine_mode/asr_shortcut 一致），通过 `get_config`/`set_config` 读写
- **字号范围 8-32px**，默认 13，clamp 到边界
- **字体族默认** `"Menlo"`（v4 从 SF Mono 改——字符紧凑相连无松散感）
- **字体族存单个族名**（非 CSS 降级链），浏览器自动 fallback monospace
- **运行时实时生效**：`term.options.fontSize` + `fitAddon.fit()` + `term.refresh()`，不重建实例
- **字体族变化必须 dispose + 重新 attach WebGL renderer**（字符 atlas 缓存问题，字号变化不需要）
- **跨窗口同步**：`set_config` emit `config-changed`，终端窗口 listen 重读 `get_config`
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

- [x] **Step 1: 加字段 + default 函数**

> **实施偏差（2026-08-02）**：默认字体族从 plan 写的 CSS 降级链 `'"SF Mono", Menlo, Monaco, "Cascadia Code", "Roboto Mono", monospace'` **改为单个族名 `"Menlo"`**（v4）。理由：① xterm `fontFamily` 接受 CSS font-family 后内部自己 fallback，存单个族名更清晰、下拉匹配更直接；② SF Mono 字宽大同行内容少像有空格，Menlo 紧凑相连。详见 spec §演进历史 v1-v5。

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

- [x] **Step 2: 确认 apply_config_value 是否需加分支**

Run: `rg -n "apply_config_value" crates/desktop/src/`

`set_config` 命令对已知 AppConfig 字段调 `apply_config_value`（处理副作用如快捷键重注册）。terminal_font_size/family 无副作用（不需重注册什么），但需确认 `apply_config_value` 不会因「未知字段」报错。若它有 match 分支 + 兜底，加一个 `_ => Ok(())` 或显式 `terminal_font_size | terminal_font_family => Ok(())` 分支。

- [x] **Step 3: 编译 + 测试**

```bash
cargo build --release -p octopus-desktop 2>&1 | tail -3
cargo test -p octopus-infra --lib 2>&1 | tail -3
```
Expected: build 0 error；infra test 全过。

- [x] **Step 4: Commit**

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

- [x] **Step 1: 删硬编码常量，改为参数**

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

- [x] **Step 2: session 暴露 setFontSize / setFontFamily 运行时改字体方法**

> **实施偏差（2026-08-02）**：`setFontFamily` 实现比 plan 多了 **WebGL renderer dispose + 重新 attach**。WebGL renderer 缓存字符 atlas（glyph texture cache），切换字体族后旧 atlas 字形仍是旧字体的，不重建会渲染错乱（字变小 + 间距大）。plan 原写的 `setFontFamily` 只有 `term.options.fontFamily = family + refresh()`，实际必须先 `webglRef.current.dispose()` 再 `attachWebgl()`。详见 spec §架构·关键约束。曾因遗漏此步导致「字体选了以后，终端显示就异常了，显示的字变的很小」。

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

- [x] **Step 3: tsc + vitest**

```bash
cd crates/desktop/frontend && npx tsc --noEmit && npx vitest run
```

- [x] **Step 4: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/useTerminalSession.ts
git commit -m "feat(terminal): useTerminalSession 读 config 字体 + 运行时改字体方法"
```

---

### Task 3: index.tsx Cmd+=/- 快捷键 + get_config 读字体

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Terminal/index.tsx`
- Modify: `crates/desktop/frontend/src/pages/Terminal/keymap.ts`

- [x] **Step 1: keymap.ts 加 isFontShortcut 纯函数**

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

- [x] **Step 2: index.tsx 加 get_config 读字体 + 传给 useTerminalSession**

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

- [x] **Step 3: index.tsx 快捷键处理**

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

- [x] **Step 4: tsc + vitest**

- [x] **Step 5: Commit**

```bash
git add crates/desktop/frontend/src/pages/Terminal/
git commit -m "feat(terminal): Cmd+=/- 快捷键调字号 + get_config 读字体配置"
```

---

### Task 4: 设置页字号 + 字体族 UI

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/`（SystemPanel 或 General tab）

> **实施偏差（2026-08-02，UI 大幅演进）**：plan 原写的「字号输入 + 字体族下拉（固定 7 预设）+ 自定义输入框」**全部废弃**，实际实现演进为：
> 1. **字体族下拉动态加载** `list_monospace_fonts` 后端命令（`fc-list :spacing=mono family` 拉系统等宽字体 + 过滤 `.` 前缀系统隐藏字体 + fallback 白名单），不再用固定预设——覆盖用户已装的所有编程字体（JetBrains Mono / Fira Code 等）
> 2. **去掉自定义输入框**——下拉已覆盖系统所有等宽字体，自定义多此一举
> 3. **加预览行**（用当前字号/字体渲染样例文字 `"The quick brown fox 123"`）
> 4. **加「恢复默认」按钮**（仅偏离默认时显示，点击一键回 13/Menlo）
> 5. **字号用 slider**（8-32，整数 step）而非数字输入
> 6. **新增 tab**：设置页加「字体字号」tab（内部 key `"font"`，为后续编辑器字体扩展预留），位于「语音」后
>
> 后端新增 `list_monospace_fonts` Tauri 命令 + `parse_monospace_fonts` 纯函数（fc-list 输出解析，8 单测）。前端提取 `isFontAtDefault` 纯函数（显隐条件，17 单测）。详见 spec §架构 + §演进历史 v3-v5。

- [x] **Step 1: 加字号输入 + 字体族下拉 + 自定义**

在设置页加：
- 字号：数字输入（8-32）或 slider，onchange 调 `set_config`
- 字体族：`<select>` 下拉（SF Mono/Menlo/Monaco/Cascadia/JetBrains/Fira/Roboto/自定义）+ 自定义时显示 `<input>`

onchange 调：
```typescript
await invoke("set_config", { key: "terminal_font_size", value: newSize });
// 或
await invoke("set_config", { key: "terminal_font_family", value: newFamily });
```

- [x] **Step 2: tsc + vitest + build**

- [x] **Step 3: Commit**

---

### Task 5: e2e 验证 + 文档同步

- [x] **Step 1: e2e**
  - 终端 Cmd+= / Cmd+- 字号实时变化
  - 设置页改字号/字体 → 重开终端生效
  - 字号 8-32 clamp
  - 自定义字体名 fallback

- [x] **Step 2: architecture.md + research 同步**

- [x] **Step 3: Commit + Review plan**

---

## Self-Review 记录

**Spec 覆盖**：
- ✅ AppConfig 两字段 → Task 1
- ✅ useTerminalSession 读 config + 运行时改 → Task 2
- ✅ Cmd+=/- 快捷键 → Task 3
- ✅ 设置页 UI → Task 4（大幅演进，见 Task 4 偏差注记）
- ✅ 字号 8-32 clamp → Task 3 setFontSize
- ✅ 跨窗口同步（config-changed 事件）→ 实际加的，plan 未列
- ✅ 字体族 WebGL renderer 重建 → Task 2 实际加的，plan 未列
- ✅ list_monospace_fonts 后端命令 + 过滤点前缀 → Task 4 实际加的，plan 未列
- ✅ 纯函数测试补录（isFontAtDefault 17 + parse_monospace_fonts 8）→ v5 加的，plan 未列

**已知实现注意**（实施后回顾）：
- Task 2 的 `fitAddonRef`：实际用 `fitAddonRef.current?.fit()`（RefObject，非闭包变量）✓
- Task 3 的 `activeTerminalSession`：实际通过 `sessionSelfRef`（TerminalPane 上报 session 引用到 index.tsx）+ `fontSizeRef`/`fontFamily` state effect 触发 `session.setFontSize/setFontFamily` ✓
- Task 4 的设置页位置：实际放在 GeneralPanel 新增「字体字号」tab（内部 key `"font"`，为后续编辑器字体扩展预留）✓

---

## 实施记录（2026-08-02 回写）

**状态**：✅ 全部 Task 1-5 已完成并验证。功能已上线（main 含完整实现）。

**演进历程**（plan 是 v1 草案，实现历经 5 个版本）：
1. **v1**：硬编码 CSS 降级链
2. **v2**：AppConfig 加字段 + 设置页固定 7 预设 + 自定义输入框
3. **v3**：去掉预设 + 自定义，下拉动态加载 `fc-list` 系统字体（过滤 `.` 前缀）+ 「恢复默认」按钮
4. **v4**：默认字体 SF Mono → Menlo（紧凑相连，SF Mono 字宽大像有空格）
5. **v5**：纯函数提取补单测（`isFontAtDefault` 17 tests + `parse_monospace_fonts` 8 tests）

**测试覆盖**：
- 后端 `parse_monospace_fonts`：8 个单测（过滤点前缀/sort dedup/补 SF Mono+Monaco/trim/空输入/空行/含空格名）
- 前端 `isFontAtDefault`：17 个单测（默认状态/偏离/边界异常/常量守护）
- 前端 `keymap.ts::isFontShortcut`：Cmd/Ctrl + =/- 测试
- e2e：用户已验证字号 slider、字体下拉、恢复默认、跨窗口同步、Cmd+=/- 快捷键

**与 plan 的关键偏差（3 处）**：
1. **Task 1 默认值**：CSS 降级链 → 单个族名 "Menlo"（v4，理由见 Global Constraints）
2. **Task 2 setFontFamily**：plan 只有 `term.options.fontFamily + refresh`，实际多了 WebGL dispose + 重新 attach（字符 atlas 缓存问题）
3. **Task 4 UI**：固定 7 预设 + 自定义输入框 → 动态系统字体下拉（`list_monospace_fonts`）+ 预览 + 恢复默认 + slider + 新 tab

**plan 未预见但实际实现的内容**：
- 跨窗口同步（`config-changed` 事件，终端窗口 listen 重读）
- `list_monospace_fonts` 后端命令 + `parse_monospace_fonts` 纯函数
- `isFontAtDefault` 前端纯函数 + 「恢复默认」按钮显隐
- 纯函数测试补录（v5）
