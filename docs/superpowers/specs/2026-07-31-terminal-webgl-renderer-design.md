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

`useTerminalSession` 的 `useEffect([active])`：
- active true→false：`webglRef.current?.dispose()` + 置 null（释放 context，Canvas 兜底渲染保留 scrollback）
- active false→true：`attachWebgl(term)` + 存 ref（切回 tab 重连 WebGL）

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
