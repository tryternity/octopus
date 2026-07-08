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
// 字符串的 code-point 个数（代理对算 1）。按 code-point 迭代计数（codePointAt 检测代理对），
// 不物化数组——替代 Array.from(s).length，用于高频 measureCaretPx 路径。
function codePointLen(s: string): number {
  let n = 0;
  let i = 0;
  while (i < s.length) {
    const code = s.codePointAt(i)!;
    i += code >= 0x10000 ? 2 : 1;
    n++;
  }
  return n;
}

// 字符串前 cpOff 个 code-point 对应的 UTF-16 code-unit 下标（代理对占 2 unit）。
// cpOff<=0 → 0；cpOff 超出 → s.length。等价 Array.from(s).slice(0,cpOff).reduce((a,ch)=>a+ch.length)，
// 但按 code-point 迭代，不物化数组。
function cpToUtf16(s: string, cpOff: number): number {
  if (cpOff <= 0) return 0;
  let cp = 0;
  let i = 0;
  while (i < s.length) {
    if (cp === cpOff) return i;
    const code = s.codePointAt(i)!;
    i += code >= 0x10000 ? 2 : 1;
    cp++;
  }
  return s.length;
}

function locateCpOffset(
  container: HTMLElement,
  pos: number | null,
): { node: Text; utf16Offset: number } | null {
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  // TreeWalker 不可回退，先收集 text node（仅存 node 引用，不物化字符串数组）。
  const nodes: Text[] = [];
  let n: Node | null;
  while ((n = walker.nextNode())) nodes.push(n as Text);
  if (nodes.length === 0) return null;
  // 长度基于 DOM 实际文本节点（非 React state text）：textRef 由 imperative textContent 同步写
  // （React 对 contentEditable children 的 commit 不更新 DOM），用 state text 会与 DOM 不一致 → clamp 错位。
  // 零分配：旧版 nodes.map(Array.from) 物化每 node 的 code-point 数组（measureCaretPx 每 tick 高频），
  // 现改为 codePointLen/cpToUtf16 迭代计数；cpLens 缓存各 node 长度避免第二遍重算。
  const cpLens: number[] = new Array(nodes.length);
  let total = 0;
  for (let i = 0; i < nodes.length; i++) {
    const len = codePointLen(nodes[i].nodeValue ?? "");
    cpLens[i] = len;
    total += len;
  }
  const target = pos == null ? total : Math.min(pos, total);
  let consumed = 0;
  for (let i = 0; i < nodes.length; i++) {
    const len = cpLens[i];
    const isLast = i === nodes.length - 1;
    if (target <= consumed + len || isLast) {
      const offsetInNode = Math.min(Math.max(target - consumed, 0), len); // code-point，clamp 进 [0, len]
      // Range API 的 offset 是 UTF-16 code unit；code-point → code unit（代理对跳 2）。
      const utf16Offset = cpToUtf16(nodes[i].nodeValue ?? "", offsetInNode);
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
  // WebKit/Blink 对空文本节点或尾部 \n 的 collapsed range 可能返回全零 rect。
  // 此时 left/top = -cRect.left/-cRect.top 会指向视口左上角——视为无效测量。
  if (rect.width === 0 && rect.height === 0 && rect.left === 0 && rect.top === 0) {
    return { left: 0, top: 0, height: 18 };
  }
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
