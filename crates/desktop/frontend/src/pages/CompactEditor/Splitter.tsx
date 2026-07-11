import { useCallback, useEffect, useRef, type ReactNode } from "react";

interface SplitterProps {
  left: ReactNode;
  right: ReactNode;
  ratio: number;
  onRatioChange: (r: number) => void;
  showRight: boolean;
}

const MIN_RATIO = 0.2;
const MAX_RATIO = 0.8;

export function Splitter({ left, right, ratio, onRatioChange, showRight }: SplitterProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  const onPointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    draggingRef.current = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    document.documentElement.classList.add("md-splitter-dragging");
  }, []);

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (!draggingRef.current || !containerRef.current) return;
      const rect = containerRef.current.getBoundingClientRect();
      const next = (e.clientX - rect.left) / rect.width;
      const clamped = Math.min(MAX_RATIO, Math.max(MIN_RATIO, next));
      onRatioChange(clamped);
    },
    [onRatioChange],
  );

  const stopDrag = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    try {
      (e.target as HTMLElement).releasePointerCapture(e.pointerId);
    } catch {
      // pointer may already be released
    }
    document.documentElement.classList.remove("md-splitter-dragging");
  }, []);

  useEffect(() => {
    return () => {
      document.documentElement.classList.remove("md-splitter-dragging");
    };
  }, []);

  if (!showRight) {
    return <div className="flex-1 flex flex-col min-h-0 min-w-0 overflow-hidden">{left}</div>;
  }

  const leftPct = `${ratio * 100}%`;
  const rightPct = `${(1 - ratio) * 100}%`;

  return (
    <div
      ref={containerRef}
      className="flex-1 grid min-h-0"
      style={{ gridTemplateColumns: `${leftPct} 1px ${rightPct}` }}
    >
      <div className="relative min-w-0 min-h-0 flex flex-col overflow-hidden">{left}</div>
      <div
        role="separator"
        aria-orientation="vertical"
        aria-valuenow={Math.round(ratio * 100)}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={stopDrag}
        onPointerCancel={stopDrag}
        className="relative bg-border cursor-col-resize select-none hover:bg-voice transition-colors"
      >
        <div className="absolute inset-y-0 -inset-x-[5px]" />
      </div>
      <div className="relative min-w-0 min-h-0 flex flex-col overflow-hidden">{right}</div>
    </div>
  );
}
