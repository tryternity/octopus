// ASR 光标测量 helpers —— 纯函数，从 Result/index.tsx 抽出以便单测。
// 所有 offset 按 code-point 计数（Array.from 语义），与后端 Rust char 对齐。

// 容器起点 → (node, offset) 的 code-point 计数（与后端 Rust char 对齐）。
export function codePointOffsetTo(container: HTMLElement, node: Node, offset: number): number {
  const pre = document.createRange();
  pre.selectNodeContents(container);
  pre.setEnd(node, offset);
  const str = pre.toString();
  return Array.from(str).length;
}

// 点击处 → 容器起始的 code-point offset。
// 用 Range 量从容器起点到点击点的纯文本，按 code-point 计数（与后端 Rust char 对齐）。
export function codePointOffsetBefore(container: HTMLElement, range: Range): number {
  return codePointOffsetTo(container, range.startContainer, range.startOffset);
}

// 量 container 内第 pos 个 code-point 处光标的相对像素位置（长度读 DOM firstText.nodeValue）。
// pos=null/超出 → 末尾。code-point 计数（Array.from 语义），UTF-16 offset 转换为 Range API 所需。
export function measureCaretPx(
  container: HTMLElement | null,
  pos: number | null,
): { left: number; top: number; height: number } | null {
  if (!container) return null;
  const cRect = container.getBoundingClientRect();
  // 长度基于 DOM 实际文本（firstText.nodeValue），不用 text 参数：text 来自 React state，而 textRef
  // 的 DOM 由 imperative textContent 同步写（React 对 contentEditable children 的 commit 不更新 DOM），
  // 用 state text 算 target 会与 DOM 不一致 → clamp 到 DOM 旧末尾 → 光标错位。pos（中插 code-point
  // offset，与后端 char 对齐）按参数定位；imperative 已保证 DOM == state，pos 对应 DOM 同位。
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  const firstText = walker.nextNode() as Text | null;
  if (!firstText) return { left: 0, top: 0, height: 18 };
  const cp = Array.from(firstText.nodeValue ?? "");
  const target = pos == null ? cp.length : Math.min(pos, cp.length);
  const offsetInNode = target;
  // Range API 的 offset 是 UTF-16 code unit；code-point → code unit 累加。
  const utf16Offset = cp.slice(0, offsetInNode).reduce((acc, ch) => acc + ch.length, 0);
  const r = document.createRange();
  r.setStart(firstText, utf16Offset);
  r.collapse(true);
  const rect = r.getBoundingClientRect();
  return { left: rect.left - cRect.left, top: rect.top - cRect.top, height: rect.height || 18 };
}
