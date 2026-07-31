/**
 * panel 宽度 hook：状态 + localStorage 持久化 + 拖动回调。
 *
 * - 初始值：localStorage 读取，缺失用 defaultWidth。不在此处 clamp（clamp 在
 *   updateFromPointer 拖动时 + index.tsx 启动时按容器尺寸做）。
 * - 拖动中只更新 state（不写 localStorage，避免逐帧 IO），pointerup 时 endDrag 落盘。
 * - 复用 CompactEditor MarkdownPane.tsx 的 ref + persist 模式（line 58-62, 110-116）。
 */
import { useCallback, useRef, useState } from "react";
import { clampPanelWidth } from "./clampPanelWidth";

export type PanelEdge = "left" | "right";

export const PANEL_MIN = 50;
export const TERMINAL_MIN = 320;

export function usePanelWidth(storageKey: string, defaultWidth: number) {
  const [width, setWidth] = useState<number>(() => {
    const saved = Number(localStorage.getItem(storageKey));
    return Number.isFinite(saved) && saved > 0 ? saved : defaultWidth;
  });
  const widthRef = useRef(width);
  const draggingRef = useRef(false);

  // widthRef 同步——endDrag 时读 ref 落盘，避免闭包陷阱（依赖数组遗漏 width）
  widthRef.current = width;

  const startDrag = useCallback(() => {
    draggingRef.current = true;
    document.documentElement.classList.add("terminal-resizing");
  }, []);

  const updateFromPointer = useCallback(
    (
      clientX: number,
      containerRect: DOMRect,
      panelEdge: PanelEdge,
      otherSideWidth: number,
    ) => {
      if (!draggingRef.current) return;
      // panelEdge="right"（sidebar，手柄在右边缘）：宽度 = clientX - 容器左
      // panelEdge="left"（file-tree，手柄在左边缘）：宽度 = 容器右 - clientX
      const raw =
        panelEdge === "right"
          ? clientX - containerRect.left
          : containerRect.right - clientX;
      const next = clampPanelWidth(
        raw,
        PANEL_MIN,
        containerRect.width,
        otherSideWidth,
        TERMINAL_MIN,
      );
      setWidth(next);
    },
    [],
  );

  const endDrag = useCallback(() => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    document.documentElement.classList.remove("terminal-resizing");
    localStorage.setItem(storageKey, String(widthRef.current));
  }, [storageKey]);

  /** 启动时按容器尺寸 clamp 已存宽度（不写 localStorage，只改本次渲染值）。
   *  场景：用户拖大 sidebar 后缩小窗口、重开——已存宽度可能让终端区 < TERMINAL_MIN。 */
  const clampTo = useCallback(
    (containerWidth: number, otherSideWidth: number) => {
      setWidth((prev) =>
        clampPanelWidth(prev, PANEL_MIN, containerWidth, otherSideWidth, TERMINAL_MIN),
      );
    },
    [],
  );

  return { width, startDrag, updateFromPointer, endDrag, clampTo };
}
