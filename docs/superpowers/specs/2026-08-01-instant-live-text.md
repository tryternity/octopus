# instant 模式实时显示识别文本

> **日期**：2026-08-01
> **状态**：✅ 已实现
> **背景**：`docs/pr/0801.md` 第 3 条——「instant 模式的语音识别，需要把识别到的文本即时显示到指示窗口，而不是一直一个正在聆听，最后给一个结果」
> **关联 spec**：[2026-08-01-merge-asr-windows-design.md](2026-08-01-merge-asr-windows-design.md) line 80 的设计假设，本次补实现

---

## 1. 问题复述

instant 模式（PTT/hands-free）录音时，指示窗口（InstantView 卡片）始终显示「正在聆听…」，看不到实时识别的文字，直到录音结束才给最终结果。用户期望边说边看到识别文本。

## 2. 根因

前端事件路由漏接：

- **后端正确发事件**：流式 partial 文本经 `tick.rs:148` → `result_window.rs:392` 发 `update-result` 事件（`{ text, insertion, caret }`），**不分 recordMode**——toggle / instant 模式都发。
- **前端 `update-result` handler 只写 toggle 视图**：`index.tsx` 的 `update-result` handler 只 `setText(payload.text)`（toggle 视图的 CM6 编辑器 state），不写 `instantText`。
- **instant 视图只订阅 `instant-state`**：`instant-state` handler 才 `setInstantText`。但 recording 期间 `instant-state` 只在开始时 emit 一次空文本（`session.rs::show_instant("listening", "")`），之后不再 emit——partial 文本走的是 `update-result`。
- **结果**：partial 文本喂给了 `display:none` 的 toggle 编辑器，可见的 InstantView 卡片因 `instantText` 始终为空，卡在「正在聆听…」占位符（`STATE_LABEL["listening"]`）。

InstantView 本身已支持 listening 态显示 text（`InstantView.tsx:61-63`：`showText = text && typedState === "listening" ? text : ""`），只是数据没喂进去。

## 3. 修复

### 3.1 `update-result` 路由到 instantText

`index.tsx` 的 `update-result` handler 在 `recordMode === "instant"` 时也写 `instantText`：

```tsx
["update-result", (p) => {
  const payload = p as { text: string; insertion: boolean; caret: number };
  caretRef.current = payload.insertion ? payload.caret : null;
  setText(payload.text);
  if (recordModeRef.current === "instant") {
    setInstantText(payload.text);
  }
}],
```

**React 闭包陷阱规避**：handler 在 `[]` 依赖 effect 内注册（`index.tsx:169` 的 listen effect，依赖仅 `[refreshActive]`），闭包捕获的 `recordMode` 是注册时的旧值。新增 `recordModeRef`（`useRef` + 同步 effect），handler 读 `recordModeRef.current` 拿最新值。对齐项目既有 ref 模式（`translateModeRef` / `toolbarVisibleRef` 等）。

toggle 模式不写 `instantText`——InstantView 被 `display:none` 隐藏，无需更新。

### 3.2 InstantView 显示尾部最新内容

`InstantView.tsx` 的 `showText` 在 listening 态取尾部最新内容：

```tsx
const LISTENING_TAIL_CHARS = 28;
const showText = typedState === "done"
  ? text
  : (text && typedState === "listening"
    ? (text.length > LISTENING_TAIL_CHARS ? text.slice(-LISTENING_TAIL_CHARS) : text)
    : "");
```

理由：InstantView 是底部小卡片（400px 宽，单行 `truncate`），完整 partial 是流式累积全文（可能很长）。用户说话时关心**最新说的词**（尾部），而非开头。截尾部 28 字符（卡片可见宽度 ≈ 340px − 图标 − padding，14px 中文字体 ≈ 24 字，取 28 留 buffer）。

done 态保持完整文本（短暂停留展示最终结果，配 `truncate` 开头截断即可——done 是终态，完整度优先）。

## 4. 不在本次范围

- InstantView 更复杂 UX（多行滚动、光标跟随、动画过渡）——本次只做单行尾部显示
- toggle 模式的实时文本（已由 CM6 编辑器承载，不在本问题范围）

## 5. 代码位置速查

| 位置 | 作用 |
|---|---|
| `crates/desktop/frontend/src/pages/Result/index.tsx` `recordModeRef` | 避免 update-result handler 闭包陷阱 |
| `crates/desktop/frontend/src/pages/Result/index.tsx` `update-result` handler | instant 模式路由 partial 到 instantText |
| `crates/desktop/frontend/src/pages/Result/InstantView.tsx` `showText` + `LISTENING_TAIL_CHARS` | listening 态尾部最新内容显示 |
| `crates/desktop/src/ui/result_window.rs::update_result` | 后端 emit `update-result`（不分模式，已存在） |
| `crates/desktop/src/engine/coordinator/tick.rs:148` | 流式 tick 调 update_result（已存在） |
