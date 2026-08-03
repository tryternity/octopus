# 终端 WebGL Renderer（默认启用 + 自动降级）

> 内嵌终端性能增强。spec 风险 #3（Phase 1 用 Canvas，性能不足切 WebGL）的落地。

**日期**：2026-07-31
**蓝本**：Terax `rendererPool.ts::attachWebgl`（context loss 恢复机制）
**关联**：[内嵌终端 spec](2026-07-31-embedded-terminal-design.md) 风险 #3

## 目标

终端默认用 WebGL renderer（GPU 加速字符渲染），大量快速输出（`find /`、`cat` 大文件、TUI 重绘）时 CPU 占用显著降低。GPU 不可用或上下文丢失时自动降级回 Canvas，用户无感。

## 范围

- ✅ 默认启用 WebGL renderer（无需配置项，用户无感）
- ✅ GPU 不可用 / attach 失败 → 静默降级 Canvas
- ✅ WKWebView context loss（sleep/wake / GPU reset）→ 自动重连
- ✅ 隐藏 tab 释放 WebGL context（避免撞 WKWebView ~16 context 上限）
- ❌ 配置开关（Terax 的 `terminalWebglEnabled` preference）——YAGNI，默认开 + 自动降级足够
- ❌ rendererPool 池化（Terax 的 slot 复用 + dormantRing）——作为后续实测后的演进备选

## 架构

### 数据流与组件接口

```
index.tsx (tabs[])
  └─ TerminalPane (active={tab.id === activeId})   ← 新增 active prop
       └─ useTerminalSession({ container, cwd, active, onExit })  ← 新增 active
            ├─ new Terminal() + FitAddon + WebLinksAddon（不变）
            ├─ openPty → onData/onResize 接线（不变）
            └─ attachWebgl(term)  ← 新增
                 ├─ active=true → try WebglAddon + onContextLoss 恢复
                 └─ active=false → dispose WebGL（Canvas 兜底）
```

**接口变更**：
- `TerminalPane` 加 `active: boolean` prop（父组件已知 `tab.id === activeId`）
- `useTerminalSession` opts 加 `active: boolean`（默认 true，向后兼容）

### attachWebgl 实现（核心，参考 Terax rendererPool.ts:766）

提取为独立函数（便于 mock 测试降级路径）。`currentWebglRef` 指 `useTerminalSession` 内部的 `useRef<WebglAddon | null>`：

```typescript
function attachWebgl(term: Terminal): WebglAddon | null {
  try {
    const webgl = new WebglAddon();
    webgl.onContextLoss(() => {
      try { webgl.dispose(); } catch {}
      // WebKit sleep/wake 或 GPU reset 后有短暂的 context 丢失窗口，
      // 250ms 后重连（值来自 Terax 实测 WEBGL_RECOVERY_DELAY_MS）。
      setTimeout(() => {
        if (currentWebglRef.current) return; // 已被其他路径重连
        const reattached = attachWebgl(term);
        if (reattached) {
          currentWebglRef.current = reattached;
          try { term.refresh(0, term.rows - 1); } catch {}
        }
      }, 250);
    });
    term.loadAddon(webgl);
    return webgl;
  } catch (e) {
    console.warn("[terminal-webgl] unavailable, fallback to canvas:", e);
    return null; // 降级 Canvas，不抛错
  }
}
```

### active 变化的 attach/dispose

`useTerminalSession` 的 `useEffect([active])`（逻辑封装在 `applyActive` 纯函数里，便于单测）：
- active true→false：`webglRef.current?.dispose()` + 置 null（释放 context，Canvas 兜底渲染保留 scrollback）
- active false→true：`attachWebgl(term)` + 存 ref（切回 tab 重连 WebGL）+ **`term.focus()`**

**`term.focus()` 是 cursor blink 修复的关键（2026-08-03）**：`CursorBlinkStateManager` 构造函数（`addon-webgl/src/CursorBlinkStateManager.ts:31-33`）只在 `_coreBrowserService.isFocused === true` 时启动 600ms blink 定时器，否则 `isPaused` 永久 true → `restartBlinkAnimation()` 早退（`:55-57`）→ 光标停在 `isCursorVisible=true` 的静态 solid block（可见但不闪，无报错）。唯一恢复路径：`term.focus()` → textarea focus 事件 → `CoreBrowserTerminal._handleTextAreaFocus` → `onFocus` → `RenderService.handleFocus` → `renderer.handleFocus` → `CursorBlinkStateManager.resume()`。

**核心根因——冷启动焦点时序（2026-08-03 第三轮， ActionBar agent 场景）**：通过 ActionBar 调 agent（tolaria pi）时，**第一个命令触发终端窗口冷启动建窗**（`terminal_window.rs:286` `WebviewWindowBuilder` + `visible(false)` → `show()` + `set_focus()`）。第一个 tab（占位 tab 被 `consumeFirstTab` 复用）的 `useEffect([])` 在窗口刚 show 时跑，`attachWebgl` 构造 `WebglRenderer` → 构造 `CursorBlinkStateManager`，**此时 WKWebView 窗口刚显示，textarea 还没收到 focus 事件** → `isFocused=false` → blink 定时器不启动 → 永久 paused。后续 tab（`addTab`）创建时窗口已稳定聚焦，renderer 构造时 `isFocused=true` → 正常。**症状**：第一个 tab 跑 Pi（alternate screen TUI）时光标由 Pi 渲染正常；`/quit` 退出 Pi 切回 normal screen 后轮到 xterm renderer 画光标，但 blink manager 一直 paused → 光标不闪。第二三个 tab 同样退出 Pi 后光标正常闪。

**5 个触发点全部补 focus**：所有让 WebGL renderer 在 `isFocused=false` 状态构造/重建的场景都必须 `term.focus()`：
1. **切 tab**（`applyActive`）——切走 tab `visibility:hidden` 让 textarea blur，切回 tab 新 attach renderer。
2. **窗口/webview 失焦再回来**（切其他 app、最小化恢复、合盖开盖）——`useEffect([])` 监听 `window focus` + `document visibilitychange`，窗口重新可见时仅 active pane 调 `term.focus()`（`activeRef` 读当前 active）。窗口失焦时 renderer `handleBlur → pause()`，回来需 `resume()`。
3. **context loss 重连**（`attachWebgl` 内 `onContextLoss` 回调，250ms 后重 attach）——重连后 `term.refresh` + `term.focus()`。
4. **`setFontFamily` 重 attach**（字体族变化 dispose + 重 attach 重建字符 atlas）——重 attach 后 `term.focus()`。
5. **冷启动 mount**（`useEffect([])` attach WebGL 后）——**用 `requestAnimationFrame` 延迟一帧** `term.focus()`：等 WKWebView 窗口真正可见后再聚焦 textarea。openPty.then 里的同步 focus 对快 PTY 有效，但慢窗口就绪场景可能太早（textarea 还不能接收焦点）；rAF 保证在下一帧渲染前（窗口已可见）执行。这是第一个 tab 冷启动场景的关键修复。

回归测试覆盖：1（`applyActive` 4 用例）、3（`attachWebgl > context loss` 断言 focus 被调）。2/4/5 属 UI 集成层（监听器/rAF/effect 闭包内），靠 e2e 实测验证。

代价：切回 tab 极短重连闪烁（WebGL 重建 < 50ms，可接受）。

### 降级矩阵

| 场景 | 行为 |
|---|---|
| GPU 可用 | WebGL renderer |
| GPU 不可用（构造抛错） | Canvas，console.warn |
| context loss（sleep/wake）| dispose + 250ms 重连，重连失败则 Canvas |
| tab 隐藏 | dispose WebGL（Canvas 兜底渲染保留 scrollback） |
| tab 重显 | attach WebGL |

## 不变量

1. attachWebgl 永不抛错——失败返回 null（降级是正常路径）
2. context loss 回调里 dispose 当前 addon 后才重连，不泄漏旧 context
3. tab 隐藏时 WebGL 必释放（防 ~16 context 上限）；Canvas renderer 由 xterm 内部保活渲染 scrollback
4. 重连后 `term.refresh(0, rows-1)` 重绘，确保内容可见

## 测试策略

WebGL addon 依赖真实 DOM + GPU，无法纯单测。防护：
- **attachWebgl 降级路径**：mock `WebglAddon` 构造抛错 → 断言返回 null + 不抛——可单测
- **context loss 时序**：fake timer + mock onContextLoss 触发 → 断言 250ms 后重连调用——可单测
- **active 切换**：mock term.dispose/loadAddon → 断言 active false dispose / true attach——可单测
- 实际 GPU 渲染靠 e2e 冒烟（大量输出无卡顿 + sleep/wake 后恢复）

## 依赖

| 新增 | 版本 | 用途 |
|---|---|---|
| `@xterm/addon-webgl` | ^0.19.0（对齐 Terax + 兼容 @xterm/xterm 6） | WebGL renderer |

## 演进备选（不在本次范围）

若实测发现隐藏 tab 释放/重连闪烁明显，或多窗口 context 仍紧张，再引入 Terax 的 **rendererPool**（slot 池化 + dormantRing，隐藏 tab 保持 WebGL 活但 park 渲染）。当前方案是简化版，先验证 WebGL 在 octopus WKWebView 的实际收益。

## 风险

1. **WKWebView WebGL 兼容性**：Terax 已验证 Tauri 2 + macOS WKWebView 全链路可行，风险低
2. **context loss 恢复时序**：250ms 来自 Terax 实测，octopus WKWebView 版本一致，应可直接采用；若恢复失败，降级 Canvas 兜底
3. **切 tab 闪烁**：< 50ms 重连，可接受；若实测明显，演进到 rendererPool
