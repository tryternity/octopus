// 带左右溢出指示器的横向滚动容器。从 index.tsx 拆出（2026-07-30）。

import { useRef, useState, useLayoutEffect } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";

export default function ScrollRow({ children, className }: {
  children: React.ReactNode; className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [overflow, setOverflow] = useState({ left: false, right: false });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const check = () => {
      setOverflow({
        left: el.scrollLeft > 4,
        right: el.scrollLeft + el.clientWidth < el.scrollWidth - 4,
      });
    };
    check();
    el.addEventListener("scroll", check, { passive: true });
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => { el.removeEventListener("scroll", check); ro.disconnect(); };
  }, []);

  return (
    <div className={cn("relative", className)}>
      <div
        ref={ref}
        className="flex items-center gap-1 px-1.5 py-[3px] shrink-0 overflow-x-auto scrollbar-none"
      >
        {children}
      </div>
      {overflow.left && (
        <div className="absolute left-0 top-0 bottom-0 flex items-center pl-0.5 pointer-events-none bg-gradient-to-r from-background/95 to-transparent">
          <ChevronLeft className="w-3 h-3 text-voice" />
        </div>
      )}
      {overflow.right && (
        <div className="absolute right-0 top-0 bottom-0 flex items-center pr-0.5 pointer-events-none bg-gradient-to-l from-background/95 to-transparent">
          <ChevronRight className="w-3 h-3 text-voice" />
        </div>
      )}
    </div>
  );
}
