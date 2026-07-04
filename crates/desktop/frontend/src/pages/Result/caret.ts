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

// 定位 container 内第 pos 个 code-point 落在哪个 text node 的哪个 UTF-16 offset。
// pos=null → 末尾。返回 null 表示容器无文本节点。measureCaretPx（量像素）与 placeCaretAtCodePoint
// （设选区）共用此遍历——多 text node（whitespace-pre-wrap 多行 / 编辑残留）下累加各 node 长度定位，
// 单 node（textContent 写入主路径）行为与旧实现一致。
function locateCpOffset(
  container: HTMLElement,
  pos: number | null,
): { node: Text; utf16Offset: number } | null {
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  let n: Node | null;
  while ((n = walker.nextNode())) nodes.push(n as Text);
  if (nodes.length === 0) return null;
  // 长度基于 DOM 实际文本节点（非 React state text）：textRef 由 imperative textContent 同步写
  // （React 对 contentEditable children 的 commit 不更新 DOM），用 state text 会与 DOM 不一致 → clamp 错位。
  const cps = nodes.map((t) => Array.from(t.nodeValue ?? ""));
  const total = cps.reduce((acc, cp) => acc + cp.length, 0);
  const target = pos == null ? total : Math.min(pos, total);
  let consumed = 0;
  for (let i = 0; i < nodes.length; i++) {
    const len = cps[i].length;
    const isLast = i === nodes.length - 1;
    if (target <= consumed + len || isLast) {
      const offsetInNode = Math.min(Math.max(target - consumed, 0), len); // code-point，clamp 进 [0, len]
      // Range API 的 offset 是 UTF-16 code unit；code-point → code unit 累加。
      const utf16Offset = cps[i].slice(0, offsetInNode).reduce((acc, ch) => acc + ch.length, 0);
      return { node: nodes[i], utf16Offset };
    }
    consumed += len;
  }
  return null; // 不可达：循环 isLast 分支必返回
}

// 量 container 内第 pos 个 code-point 处光标的相对像素位置（pos=null/超出 → 末尾）。
export function measureCaretPx(
  container: HTMLElement | null,
  pos: number | null,
): { left: number; top: number; height: number } | null {
  if (!container) return null;
  const cRect = container.getBoundingClientRect();
  const loc = locateCpOffset(container, pos);
  if (!loc) return { left: 0, top: 0, height: 18 };
  const r = document.createRange();
  r.setStart(loc.node, loc.utf16Offset);
  r.collapse(true);
  const rect = r.getBoundingClientRect();
  return { left: rect.left - cRect.left, top: rect.top - cRect.top, height: rect.height || 18 };
}

// 把光标（collapsed Selection）定位到 container 内第 pos 个 code-point 处（进入编辑态恢复点击位用）。
// pos 超出总长 → 末尾。返回 false 表示容器无文本节点（调用方应自行兜底，如置末尾）。
export function placeCaretAtCodePoint(container: HTMLElement, pos: number): boolean {
  const loc = locateCpOffset(container, pos);
  if (!loc) return false;
  const r = document.createRange();
  r.setStart(loc.node, loc.utf16Offset);
  r.collapse(true);
  const sel = window.getSelection();
  sel?.removeAllRanges();
  sel?.addRange(r);
  return true;
}
