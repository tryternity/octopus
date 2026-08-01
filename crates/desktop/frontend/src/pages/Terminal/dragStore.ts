/**
 * 文件树 → 终端拖拽的共享状态 + 自定义 ghost 影像（pointer events 方案）。
 *
 * 背景：HTML5 Drag and Drop（draggable/dataTransfer）在 WKWebView + xterm canvas
 * 下不可靠（drop 事件不触发，capture 阶段也不行）。改用 pointer events 模拟：
 * FileTreePanel 节点 mousedown 调 startDrag，TerminalPane canvas mouseup 调 takeDragPath
 * 并写入。document mouseup 兜底清除（拖到非终端区松开时）。
 *
 * 自定义 ghost：pointer events 方案无原生 ghost 影像，手动创建跟随鼠标的 div
 * 显示文件名，提供拖拽视觉反馈（近似 OS 原生体验）。
 *
 * 模块级单例——FileTreePanel 和 TerminalPane 是兄弟组件，用模块级状态共享最简。
 */

let dragPath: string | null = null;
let ghostEl: HTMLDivElement | null = null;
let moveHandler: ((e: MouseEvent) => void) | null = null;

/** 创建跟随鼠标的 ghost div（显示 label）。 */
function createGhost(label: string): HTMLDivElement {
  const el = document.createElement("div");
  el.className = "terminal-file-drag-ghost";
  el.textContent = label;
  // 初始位置在屏外，首次 mousemove 才定位（避免 mousedown 瞬间闪烁）
  el.style.cssText = "position: fixed; left: -9999px; top: -9999px;";
  document.body.appendChild(el);
  return el;
}

/**
 * 文件树节点 mousedown 时调：记录待拖拽路径 + 创建 ghost + 启动 mousemove 跟踪。
 * @param path 完整路径（终端写入用）
 * @param label ghost 显示的文件名（通常 path 的 basename）
 */
export function startDrag(path: string, label: string): void {
  dragPath = path;
  if (ghostEl) ghostEl.remove();
  ghostEl = createGhost(label);
  moveHandler = (e: MouseEvent) => {
    if (ghostEl) {
      // 偏移到鼠标右下，避免遮挡鼠标点击位置
      ghostEl.style.left = `${e.clientX + 8}px`;
      ghostEl.style.top = `${e.clientY + 8}px`;
    }
  };
  document.addEventListener("mousemove", moveHandler);
  document.body.classList.add("terminal-file-dragging");
}

/** 移除 ghost + 停止 mousemove 跟踪 + 清除 body class。 */
function cleanupGhost(): void {
  if (moveHandler) {
    document.removeEventListener("mousemove", moveHandler);
    moveHandler = null;
  }
  if (ghostEl) {
    ghostEl.remove();
    ghostEl = null;
  }
  document.body.classList.remove("terminal-file-dragging");
}

/**
 * 终端 canvas mouseup 时调：取出 dragPath（取出即清除 ghost + 状态，防重复写入）。
 * 返回 null 表示无拖拽进行中（普通点击松开）。
 */
export function takeDragPath(): string | null {
  const p = dragPath;
  dragPath = null;
  cleanupGhost();
  return p;
}
