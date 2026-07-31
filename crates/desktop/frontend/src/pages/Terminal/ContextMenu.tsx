/**
 * 通用右键浮层菜单组件。
 *
 * position: fixed 浮层，点击外部关闭。不引入 radix-ui（轻量自绘）。
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
    // 点击外部关闭
    const handleClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        onClose();
      }
    };
    // Escape 关闭
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    // 延迟一帧注册，避免触发 contextmenu 的同一次 click 立即关闭
    const id = requestAnimationFrame(() => {
      window.addEventListener("mousedown", handleClick, true);
      window.addEventListener("keydown", handleKey, true);
    });
    return () => {
      cancelAnimationFrame(id);
      window.removeEventListener("mousedown", handleClick, true);
      window.removeEventListener("keydown", handleKey, true);
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
          onClick={() => {
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
