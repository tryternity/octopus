/**
 * 文件树 → 终端拖拽的共享状态（pointer events 方案）。
 *
 * 背景：HTML5 Drag and Drop（draggable/dataTransfer）在 WKWebView + xterm canvas
 * 下不可靠（drop 事件不触发，capture 阶段也不行）。改用 pointer events 模拟：
 * FileTreePanel 节点 mousedown 设 dragPath，TerminalPane canvas mouseup 读 dragPath
 * 并写入。document mouseup 兜底清除（拖到非终端区松开时）。
 *
 * 模块级单例——FileTreePanel 和 TerminalPane 是兄弟组件，用模块级 ref 共享最简。
 */

let dragPath: string | null = null;

/** 文件树节点 mousedown 时调：记录待拖拽的完整路径。 */
export function setDragPath(path: string): void {
  dragPath = path;
}

/**
 * 终端 canvas mouseup 时调：取出 dragPath（取出即清除，防重复写入）。
 * 返回 null 表示无拖拽进行中（普通点击松开）。
 */
export function takeDragPath(): string | null {
  const p = dragPath;
  dragPath = null;
  return p;
}

/** 清除拖拽状态（拖到非终端区松开时兜底）。 */
export function clearDragPath(): void {
  dragPath = null;
}
