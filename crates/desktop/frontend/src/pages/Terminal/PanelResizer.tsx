/**
 * panel 拖拽手柄——4px 宽，绝对定位贴在 panel 边缘。
 *
 * 只负责 pointer 事件转发（不持有宽度状态），宽度逻辑由父用 usePanelWidth hook 控制。
 * 侧边条手柄（side="right" 贴右边缘，sidebar 用）；side="left" 贴左边缘（file-tree 用）。
 *
 * 参考实现：CompactEditor MarkdownPane.tsx:205-216（pointer capture + dragging class）。
 */
import { useRef } from "react";

type Props = {
  side: "left" | "right";
  onStart: () => void;
  onMove: (clientX: number) => void;
  onEnd: () => void;
};

export function PanelResizer({ side, onStart, onMove, onEnd }: Props) {
  const draggingRef = useRef(false);

  const handleDown = (e: React.PointerEvent<HTMLDivElement>) => {
    draggingRef.current = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    onStart();
  };

  const handleMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    onMove(e.clientX);
  };

  const handleUp = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    try {
      (e.target as HTMLElement).releasePointerCapture(e.pointerId);
    } catch {
      /* 已 release */
    }
    onEnd();
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      className={`terminal-panel-resizer terminal-panel-resizer-side-${side}`}
      onPointerDown={handleDown}
      onPointerMove={handleMove}
      onPointerUp={handleUp}
      onPointerCancel={handleUp}
    />
  );
}
