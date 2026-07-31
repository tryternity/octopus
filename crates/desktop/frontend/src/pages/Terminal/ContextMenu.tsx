/**
 * 通用右键浮层菜单组件。
 *
 * position: fixed 浮层，点击外部 / Esc / 滚动 关闭。不引入 radix-ui（轻量自绘）。
 * 由调用方管理 open 状态 + 坐标，传 items 数组。
 */

import { useEffect, useRef } from "react";

export type MenuItem = {
  label: string;
  action: () => void;
  disabled?: boolean;
};

export type MenuPosition = { x: number; y: number } | null;

type Props = {
  position: MenuPosition;
  items: MenuItem[];
  onClose: () => void;
};

export function ContextMenu({ position, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!position) return;

    // 点击外部关闭——同时监听 mousedown（xterm 等可能消费 click）和 click（兜底）
    const handlePointerDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        onClose();
      }
    };
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const handleScroll = () => onClose();

    // 立即注册（不延迟）——contextmenu 事件本身不会触发 mousedown/click，
    // 所以不会误关闭。capture 阶段确保在 xterm 等组件之前捕获。
    window.addEventListener("mousedown", handlePointerDown, true);
    window.addEventListener("click", handlePointerDown, true);
    window.addEventListener("keydown", handleKey, true);
    window.addEventListener("wheel", handleScroll, true);
    window.addEventListener("contextmenu", handlePointerDown, true);

    return () => {
      window.removeEventListener("mousedown", handlePointerDown, true);
      window.removeEventListener("click", handlePointerDown, true);
      window.removeEventListener("keydown", handleKey, true);
      window.removeEventListener("wheel", handleScroll, true);
      window.removeEventListener("contextmenu", handlePointerDown, true);
    };
  }, [position, onClose]);

  if (!position) return null;

  return (
    <div
      ref={ref}
      className="context-menu"
      style={{ left: position.x, top: position.y }}
    >
      {items.map((item, i) => (
        <div
          key={i}
          className={`context-menu-item ${item.disabled ? "context-menu-item-disabled" : ""}`}
          onClick={(e) => {
            e.stopPropagation();
            if (item.disabled) return;
            item.action();
            onClose();
          }}
        >
          {item.label}
        </div>
      ))}
    </div>
  );
}
