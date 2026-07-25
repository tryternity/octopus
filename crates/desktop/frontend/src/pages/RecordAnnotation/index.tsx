/**
 * RecordAnnotation —— 录屏标注 overlay（最小验证版）。
 *
 * 录屏开始后显示在选区位置，用户可以：
 * - 拖动画矩形标注（mousedown/move/up）
 * - 按 A 切换标注/透传模式（透传时鼠标穿透到下层应用）
 *
 * 标注通过 Canvas 绘制，因为 overlay 是普通窗口 level（非 always_on_top），
 * SCK 会录到 overlay 的内容（spike7/8 验证）。
 *
 * 这是验证版——确认「画的矩形被录进视频」后，再补全 9 种工具 + 颜色/线宽等。
 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export default function RecordAnnotation() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [rects, setRects] = useState<Rect[]>([]);
  const [drawing, setDrawing] = useState<Rect | null>(null);
  const [passthrough, setPassthrough] = useState(false);
  const startRef = useRef<{ x: number; y: number } | null>(null);
  const drawingRef = useRef<Rect | null>(null);
  const rectsRef = useRef<Rect[]>([]);
  const passthroughRef = useRef(false);

  useEffect(() => { rectsRef.current = rects; }, [rects]);
  useEffect(() => { drawingRef.current = drawing; }, [drawing]);
  useEffect(() => { passthroughRef.current = passthrough; }, [passthrough]);

  const draw = () => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const w = window.innerWidth;
    const h = window.innerHeight;
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    ctx.clearRect(0, 0, w, h);
    // 画所有已完成的矩形（红色边框）
    ctx.strokeStyle = "#ef4444";
    ctx.lineWidth = 3;
    for (const r of rectsRef.current) {
      ctx.strokeRect(r.x, r.y, r.w, r.h);
    }
    // 画正在拖的矩形
    if (drawingRef.current) {
      ctx.strokeRect(drawingRef.current.x, drawingRef.current.y, drawingRef.current.w, drawingRef.current.h);
    }
  };

  useEffect(() => { draw(); }, [rects, drawing]);

  const onMouseDown = (e: React.MouseEvent) => {
    if (passthroughRef.current) return; // 透传模式不画
    if (e.button !== 0) return;
    startRef.current = { x: e.clientX, y: e.clientY };
    setDrawing({ x: e.clientX, y: e.clientY, w: 0, h: 0 });
  };

  const onMouseMove = (e: React.MouseEvent) => {
    if (!startRef.current) return;
    const s = startRef.current;
    setDrawing({
      x: Math.min(s.x, e.clientX),
      y: Math.min(s.y, e.clientY),
      w: Math.abs(e.clientX - s.x),
      h: Math.abs(e.clientY - s.y),
    });
  };

  const onMouseUp = () => {
    const d = drawingRef.current;
    if (d && d.w > 5 && d.h > 5) {
      setRects([...rectsRef.current, d]);
    }
    setDrawing(null);
    startRef.current = null;
  };

  // A 键切换标注/透传
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "a" || e.key === "A") {
        e.preventDefault();
        const next = !passthroughRef.current;
        setPassthrough(next);
        invoke("set_annotation_passthrough", { passthrough: next }).catch(() => {});
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <>
      <canvas
        ref={canvasRef}
        style={{
          position: "fixed",
          inset: 0,
          width: "100vw",
          height: "100vh",
          cursor: passthrough ? "default" : "crosshair",
          display: "block",
        }}
        onMouseDown={onMouseDown}
        onMouseMove={onMouseMove}
        onMouseUp={onMouseUp}
      />
      {/* 顶部工具栏（最小版：只显示模式状态）*/}
      <div
        style={{
          position: "fixed",
          top: 8,
          left: "50%",
          transform: "translateX(-50%)",
          padding: "4px 12px",
          borderRadius: 6,
          background: passthrough ? "rgba(0,0,0,0.4)" : "rgba(239,68,68,0.9)",
          color: "#fff",
          fontSize: 11,
          fontFamily: "-apple-system, sans-serif",
          pointerEvents: "none",
          whiteSpace: "nowrap",
        }}
      >
        {passthrough ? "透传模式（按 A 切回标注）" : "标注模式（按 A 切透传）"}
      </div>
    </>
  );
}
