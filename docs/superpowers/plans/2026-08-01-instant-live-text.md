# 实施计划：instant 模式实时显示识别文本

> **对应 spec**：[2026-08-01-instant-live-text.md](../specs/2026-08-01-instant-live-text.md)
> **分支**：`bugfix/pr-0801`（worktree `.worktrees/bugfix_pr_0801`）
> **状态**：✅ 已完成

## 任务分解 + 实施记录

### Task 1：`update-result` handler 路由 + recordModeRef ✅

文件：`crates/desktop/frontend/src/pages/Result/index.tsx`

- 新增 `recordModeRef`（`useRef<"toggle"|"instant">("toggle")` + 同步 effect），避免 update-result handler 的 React 闭包陷阱（handler 在 `[refreshActive]` effect 内注册）
- `update-result` handler：`recordModeRef.current === "instant"` 时 `setInstantText(payload.text)`

### Task 2：InstantView 尾部最新内容 ✅

文件：`crates/desktop/frontend/src/pages/Result/InstantView.tsx`

- 新增 `LISTENING_TAIL_CHARS = 28` 常量
- `showText` listening 态：`text.length > 28 ? text.slice(-28) : text`（尾部最新）
- done 态保持完整（`truncate` 开头截断）

### Task 3：前端 build + 影响面 ✅

`cd crates/desktop/frontend && npm run build` → `✓ built in 880ms`，0 error。

影响面：
- `recordMode` / `recordModeRef`：6 处消费点（state 定义 + ref + effect + handler 判断 + 2 处 display 切换），均已正确处理
- InstantView `showText` / `LISTENING_TAIL`：纯展示组件，props 接口不变

### Task 4：文档同步 ✅

- 新 spec：`docs/superpowers/specs/2026-08-01-instant-live-text.md`
- 本 plan：`docs/superpowers/plans/2026-08-01-instant-live-text.md`
- merge-asr-windows spec line 80 补注记（设计假设 vs 实现差距已补）
- `docs/pr/0801.md` 标记问题 3 完成

## 与计划的偏差

无偏差。本次是纯前端小改动（2 个文件），无 Rust 代码变化。

## 不在本次范围

- 问题 1（已完成）、问题 2（已完成）
- InstantView 更复杂 UX（多行/光标/动画）
