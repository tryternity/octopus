import { useState, useEffect } from "react";
import { measureCaretPx } from "./caret";

// 闪烁光标：绝对定位到 pos 处的像素位置（相对文本容器）。
// 依赖 text/pos 变化重新量像素；container 经 textRef.current 透传。
// containerRef 而非 container：editing 切换致 textRef 重挂载（key edit→view）时，render 阶段
// 求值的 textRef.current 是**即将卸载的旧 div**（ref 在 commit 后才更新），传其 .current 会测到
// detached 旧 div → getBoundingClientRect 返回 (0,0) → 光标错落首位且不再重测。改传 RefObject，
// effect（commit 后执行）内读 .current 拿到已挂载的新 view div，量到真实末尾。
export function CaretBlink({
  containerRef,
  text,
  pos,
}: {
  containerRef: React.RefObject<HTMLDivElement | null>;
  text: string;
  pos: number | null;
}) {
  const [px, setPx] = useState<{ left: number; top: number; height: number } | null>(null);
  useEffect(() => {
    const el = containerRef.current;
    const measure = () => setPx(measureCaretPx(el, pos));
    let raf = requestAnimationFrame(measure);
    if (!el) {
      cancelAnimationFrame(raf);
      return;
    }
    let scrollRaf = 0;
    const onScroll = () => {
      if (scrollRaf) return;
      scrollRaf = requestAnimationFrame(() => { scrollRaf = 0; measure(); });
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      cancelAnimationFrame(raf);
      el.removeEventListener("scroll", onScroll);
      if (scrollRaf) cancelAnimationFrame(scrollRaf);
    };
  }, [containerRef, text, pos]);
  if (!px) return null;
  const el = containerRef.current;
  if (el && (px.top < -2 || px.top > el.clientHeight + 2)) return null;
  return (
    <span
      className="asr-caret"
      style={{ left: px.left, top: px.top, height: px.height }}
    />
  );
}
