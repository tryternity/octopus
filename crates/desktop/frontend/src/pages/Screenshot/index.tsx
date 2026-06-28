import { useState, useRef, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";

interface Selection {
  x: number; y: number; w: number; h: number;
}

type Mode = "idle" | "selecting" | "selected" | "move" | "resize";
type Tool = "none" | "rect" | "arrow" | "text";

interface Annotation {
  type: "rect" | "arrow" | "text";
  x1: number; y1: number; x2: number; y2: number;
  text?: string;
}

const HANDLE_SIZE = 8;
const MIN_SIZE = 10;

export default function Screenshot() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const bgImgRef = useRef<HTMLImageElement | null>(null);
  const startPtRef = useRef({ x: 0, y: 0 });
  const moveStartRef = useRef({ x: 0, y: 0 });
  const selStartRef = useRef<Selection>({ x: 0, y: 0, w: 0, h: 0 });
  const drawingRef = useRef<Annotation | null>(null);
  const textInputRef = useRef<HTMLTextAreaElement | null>(null);

  const [mode, setMode] = useState<Mode>("idle");
  const [sel, setSel] = useState<Selection | null>(null);
  const [resizeHandle, setResizeHandle] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [tool, setTool] = useState<Tool>("none");
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [textDraft, setTextDraft] = useState<{ x: number; y: number; val: string } | null>(null);

  const dpr = window.devicePixelRatio || 1;

  const winLabel = (() => {
    try { return getCurrentWindow().label; } catch { return "screenshot_window"; }
  })();

  useEffect(() => {
    invoke<{ image: string; width: number; height: number }>("get_screenshot_image", { label: winLabel })
      .then((data) => {
        const img = new Image();
        img.onload = () => {
          bgImgRef.current = img;
          setReady(true);
          setTimeout(() => { invoke("show_screenshot_window", { label: winLabel }).catch(() => {}); }, 50);
        };
        img.src = `data:image/png;base64,${data.image}`;
      })
      .catch((e) => console.error("Failed to get screenshot image:", e));
  }, []);

  // 绘制
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const bg = bgImgRef.current;
    if (!canvas || !bg || !ready) return;

    const cssW = window.innerWidth;
    const cssH = window.innerHeight;
    canvas.width = cssW * dpr;
    canvas.height = cssH * dpr;
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.drawImage(bg, 0, 0, cssW, cssH);

    // 暗遮罩
    ctx.fillStyle = "rgba(0, 0, 0, 0.5)";
    if (sel) {
      const { x, y, w, h } = sel;
      ctx.fillRect(0, 0, cssW, y);
      ctx.fillRect(0, y + h, cssW, cssH - y - h);
      ctx.fillRect(0, y, x, h);
      ctx.fillRect(x + w, y, cssW - x - w, h);

      // 选区内：先绘制标注（裁剪到选区）
      ctx.save();
      ctx.beginPath();
      ctx.rect(x, y, w, h);
      ctx.clip();

      for (const ann of annotations) {
        drawAnnotation(ctx, ann);
      }
      if (drawingRef.current) {
        drawAnnotation(ctx, drawingRef.current);
      }
      if (textDraft) {
        ctx.font = "16px -apple-system, sans-serif";
        ctx.fillStyle = "#ef4444";
        ctx.fillText(textDraft.val, textDraft.x, textDraft.y + 16);
      }

      ctx.restore();

      // 选区边框 + 手柄
      ctx.strokeStyle = "#3b82f6";
      ctx.lineWidth = 2;
      ctx.strokeRect(x, y, w, h);

      if (mode === "selected" || mode === "move" || mode === "resize") {
        ctx.fillStyle = "#3b82f6";
        const handles = getHandles(sel);
        for (const [hx, hy] of handles) {
          ctx.fillRect(hx - HANDLE_SIZE / 2, hy - HANDLE_SIZE / 2, HANDLE_SIZE, HANDLE_SIZE);
        }

        // 尺寸标注
        const label = `${Math.round(w * dpr)} × ${Math.round(h * dpr)}`;
        ctx.font = "12px -apple-system, sans-serif";
        const tw = ctx.measureText(label).width;
        ctx.fillStyle = "rgba(255, 255, 255, 0.9)";
        ctx.fillRect(x + w - tw - 8, y + h + 4, tw + 8, 18);
        ctx.fillStyle = "#1a1a1a";
        ctx.fillText(label, x + w - tw - 4, y + h + 17);
      }
    } else {
      ctx.fillRect(0, 0, cssW, cssH);
    }
  }, [sel, mode, ready, dpr, annotations, textDraft, tool]);

  useEffect(() => { draw(); }, [draw]);

  function drawAnnotation(ctx: CanvasRenderingContext2D, ann: Annotation) {
    ctx.strokeStyle = "#ef4444";
    ctx.fillStyle = "#ef4444";
    ctx.lineWidth = 3;

    if (ann.type === "rect") {
      const x = Math.min(ann.x1, ann.x2);
      const y = Math.min(ann.y1, ann.y2);
      const w = Math.abs(ann.x2 - ann.x1);
      const h = Math.abs(ann.y2 - ann.y1);
      ctx.strokeRect(x, y, w, h);
    } else if (ann.type === "arrow") {
      const dx = ann.x2 - ann.x1;
      const dy = ann.y2 - ann.y1;
      const len = Math.sqrt(dx * dx + dy * dy);
      if (len < 5) return;
      ctx.beginPath();
      ctx.moveTo(ann.x1, ann.y1);
      ctx.lineTo(ann.x2, ann.y2);
      ctx.stroke();
      // 箭头头部
      const angle = Math.atan2(dy, dx);
      const headLen = 12;
      ctx.beginPath();
      ctx.moveTo(ann.x2, ann.y2);
      ctx.lineTo(ann.x2 - headLen * Math.cos(angle - Math.PI / 6), ann.y2 - headLen * Math.sin(angle - Math.PI / 6));
      ctx.lineTo(ann.x2 - headLen * Math.cos(angle + Math.PI / 6), ann.y2 - headLen * Math.sin(angle + Math.PI / 6));
      ctx.closePath();
      ctx.fill();
    } else if (ann.type === "text" && ann.text) {
      ctx.font = "16px -apple-system, sans-serif";
      ctx.fillText(ann.text, ann.x1, ann.y1 + 16);
    }
  }

  function getHandles(s: Selection): [number, number][] {
    return [
      [s.x, s.y], [s.x + s.w / 2, s.y], [s.x + s.w, s.y],
      [s.x + s.w, s.y + s.h / 2], [s.x + s.w, s.y + s.h],
      [s.x + s.w / 2, s.y + s.h], [s.x, s.y + s.h], [s.x, s.y + s.h / 2],
    ];
  }

  function hitTest(mx: number, my: number): string | null {
    if (!sel) return null;
    const handles = getHandles(sel);
    const names = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
    for (let i = 0; i < handles.length; i++) {
      const [hx, hy] = handles[i];
      if (Math.abs(mx - hx) < HANDLE_SIZE && Math.abs(my - hy) < HANDLE_SIZE) return names[i];
    }
    return null;
  }

  function inSelection(mx: number, my: number): boolean {
    if (!sel) return false;
    return mx >= sel.x && mx <= sel.x + sel.w && my >= sel.y && my <= sel.y + sel.h;
  }

  // 鼠标事件
  function onMouseDown(e: React.MouseEvent) {
    const mx = e.clientX;
    const my = e.clientY;
    startPtRef.current = { x: mx, y: my };

    // 标注工具激活时，在选区内绘制
    if (tool !== "none" && sel && inSelection(mx, my)) {
      if (tool === "text") {
        setTextDraft({ x: mx, y: my, val: "" });
        setTimeout(() => textInputRef.current?.focus(), 10);
        return;
      }
      drawingRef.current = { type: tool, x1: mx, y1: my, x2: mx, y2: my };
      return;
    }

    if (mode === "selected" || mode === "idle") {
      const handle = hitTest(mx, my);
      if (handle) {
        setResizeHandle(handle);
        setMode("resize");
        if (sel) selStartRef.current = { ...sel };
        return;
      }
      if (sel && inSelection(mx, my)) {
        setMode("move");
        moveStartRef.current = { x: mx, y: my };
        selStartRef.current = { ...sel };
        return;
      }
    }

    setSel({ x: mx, y: my, w: 0, h: 0 });
    setMode("selecting");
  }

  function onMouseMove(e: React.MouseEvent) {
    const mx = e.clientX;
    const my = e.clientY;

    // 标注绘制中
    if (drawingRef.current && tool !== "none") {
      drawingRef.current = { ...drawingRef.current, x2: mx, y2: my };
      draw();
      return;
    }

    if (mode === "idle" || mode === "selected") {
      const handle = hitTest(mx, my);
      if (handle) {
        const cursors: Record<string, string> = {
          nw: "nwse-resize", se: "nwse-resize", ne: "nesw-resize", sw: "nesw-resize",
          n: "ns-resize", s: "ns-resize", e: "ew-resize", w: "ew-resize",
        };
        (e.currentTarget as HTMLCanvasElement).style.cursor = tool !== "none" ? "crosshair" : (cursors[handle] || "crosshair");
      } else if (sel && inSelection(mx, my)) {
        (e.currentTarget as HTMLCanvasElement).style.cursor = tool !== "none" ? "crosshair" : "move";
      } else {
        (e.currentTarget as HTMLCanvasElement).style.cursor = "crosshair";
      }
    }

    if (mode === "selecting") {
      setSel(normalize(startPtRef.current.x, startPtRef.current.y, mx, my));
    } else if (mode === "move" && sel) {
      const dx = mx - moveStartRef.current.x;
      const dy = my - moveStartRef.current.y;
      const ns = selStartRef.current;
      let nx = Math.max(0, Math.min(ns.x + dx, window.innerWidth - ns.w));
      let ny = Math.max(0, Math.min(ns.y + dy, window.innerHeight - ns.h));
      setSel({ ...ns, x: nx, y: ny });
    } else if (mode === "resize" && sel && resizeHandle) {
      setSel(resizeSel(selStartRef.current, resizeHandle, mx, my));
    }
  }

  function onMouseUp() {
    if (drawingRef.current) {
      const ann = drawingRef.current;
      drawingRef.current = null;
      // 过滤太小的
      if (ann.type === "rect") {
        if (Math.abs(ann.x2 - ann.x1) > 5 && Math.abs(ann.y2 - ann.y1) > 5) {
          setAnnotations([...annotations, ann]);
        }
      } else if (ann.type === "arrow") {
        const dx = ann.x2 - ann.x1;
        const dy = ann.y2 - ann.y1;
        if (Math.sqrt(dx * dx + dy * dy) > 10) {
          setAnnotations([...annotations, ann]);
        }
      }
      return;
    }

    if (mode === "selecting" && sel) {
      if (sel.w < MIN_SIZE || sel.h < MIN_SIZE) { setSel(null); setMode("idle"); }
      else { setMode("selected"); }
    } else if (mode === "move" || mode === "resize") {
      setMode("selected");
      setResizeHandle(null);
    }
  }

  function onKeyDown(e: React.KeyboardEvent) {
    if (textDraft) return; // 文字输入时不处理
    if (e.key === "Escape") {
      if (tool !== "none") { setTool("none"); return; }
      invoke("cancel_screenshot").catch(() => {});
    } else if (e.key === "Enter" && sel && sel.w >= MIN_SIZE && sel.h >= MIN_SIZE) {
      doConfirm();
    } else if ((e.metaKey || e.ctrlKey) && e.key === "z") {
      setAnnotations(annotations.slice(0, -1));
    }
  }

  function onContextMenu(e: React.MouseEvent) {
    e.preventDefault();
    invoke("cancel_screenshot").catch(() => {});
  }

  function doConfirm() {
    if (!sel) return;
    // 合成标注到 Canvas 并导出为带标注的 PNG → 传给后端裁剪
    // 后端裁剪不含标注（标注在选区内绘制），
    // 需要把标注合成进去。用临时 canvas 合成。
    if (annotations.length > 0 && bgImgRef.current && sel) {
      const tmpCanvas = document.createElement("canvas");
      tmpCanvas.width = bgImgRef.current.naturalWidth;
      tmpCanvas.height = bgImgRef.current.naturalHeight;
      const tmpCtx = tmpCanvas.getContext("2d")!;
      tmpCtx.drawImage(bgImgRef.current, 0, 0);
      // 标注坐标是 CSS 像素，需要乘 dpr 转物理
      for (const ann of annotations) {
        drawAnnotation(tmpCtx, {
          ...ann,
          x1: ann.x1 * dpr, y1: ann.y1 * dpr,
          x2: ann.x2 * dpr, y2: ann.y2 * dpr,
        });
      }
      // 标注合成后的全图存回 bgImgRef，后端裁剪时就会包含标注
      const dataUrl = tmpCanvas.toDataURL("image/png");
      const newImg = new Image();
      newImg.onload = () => {
        bgImgRef.current = newImg;
        draw();
        // 实际发送裁剪请求
        sendConfirm();
      };
      newImg.src = dataUrl;
    } else {
      sendConfirm();
    }
  }

  function sendConfirm() {
    if (!sel) return;
    invoke("confirm_screenshot", {
      label: winLabel,
      x: Math.round(sel.x * dpr),
      y: Math.round(sel.y * dpr),
      w: Math.round(sel.w * dpr),
      h: Math.round(sel.h * dpr),
    }).catch(() => {});
  }

  function normalize(x1: number, y1: number, x2: number, y2: number): Selection {
    return { x: Math.min(x1, x2), y: Math.min(y1, y2), w: Math.abs(x2 - x1), h: Math.abs(y2 - y1) };
  }

  function resizeSel(start: Selection, handle: string, mx: number, my: number): Selection {
    let { x, y, w, h } = start;
    if (handle.includes("w")) { const rx = x + w; x = Math.min(mx, rx - MIN_SIZE); w = rx - x; }
    if (handle.includes("e")) { w = Math.max(MIN_SIZE, mx - x); }
    if (handle.includes("n")) { const by = y + h; y = Math.min(my, by - MIN_SIZE); h = by - y; }
    if (handle.includes("s")) { h = Math.max(MIN_SIZE, my - y); }
    return { x, y, w, h };
  }

  // 工具栏位置（选区下方，如果空间不够放上方）
  const toolbarY = sel ? (sel.y + sel.h + 8 + 44 < window.innerHeight ? sel.y + sel.h + 8 : sel.y - 48) : 0;
  const toolbarX = sel ? Math.min(sel.x, window.innerWidth - 280) : 0;

  if (!ready) {
    return <div style={{ width: "100vw", height: "100vh", background: "rgba(0,0,0,0.5)" }} />;
  }

  return (
    <>
      <canvas
        ref={canvasRef}
        style={{ position: "fixed", top: 0, left: 0, width: "100vw", height: "100vh", cursor: "crosshair", outline: "none" }}
        tabIndex={0}
        autoFocus
        onMouseDown={onMouseDown}
        onMouseMove={onMouseMove}
        onMouseUp={onMouseUp}
        onKeyDown={onKeyDown}
        onContextMenu={onContextMenu}
      />

      {/* 文字输入浮层 */}
      {textDraft && (
        <textarea
          ref={textInputRef}
          value={textDraft.val}
          onChange={(e) => { setTextDraft({ ...textDraft, val: e.target.value }); draw(); }}
          onBlur={() => {
            if (textDraft.val.trim()) {
              setAnnotations([...annotations, { type: "text", x1: textDraft.x, y1: textDraft.y, x2: textDraft.x, y2: textDraft.y, text: textDraft.val }]);
            }
            setTextDraft(null);
          }}
          onKeyDown={(e) => { if (e.key === "Escape") { setTextDraft(null); } e.stopPropagation(); }}
          style={{
            position: "fixed",
            left: textDraft.x,
            top: textDraft.y,
            fontSize: 16,
            color: "#ef4444",
            background: "transparent",
            border: "1px dashed #ef4444",
            outline: "none",
            resize: "none",
            padding: "2px 4px",
            minHeight: 24,
            width: 200,
          }}
        />
      )}

      {/* 工具栏 */}
      {sel && mode === "selected" && (
        <div
          style={{
            position: "fixed",
            left: toolbarX,
            top: toolbarY,
            display: "flex",
            gap: 4,
            padding: "6px 8px",
            background: "rgba(255,255,255,0.95)",
            borderRadius: 8,
            boxShadow: "0 4px 16px rgba(0,0,0,0.25)",
            zIndex: 100,
            alignItems: "center",
          }}
        >
          <ToolButton active={tool === "rect"} onClick={() => setTool(tool === "rect" ? "none" : "rect")} label="矩形" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><rect x="3" y="4" width="12" height="10" rx="1" stroke="currentColor" strokeWidth="2"/></svg>
          } />
          <ToolButton active={tool === "arrow"} onClick={() => setTool(tool === "arrow" ? "none" : "arrow")} label="箭头" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><path d="M3 15L15 3M15 3L10 3M15 3L15 8" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/></svg>
          } />
          <ToolButton active={tool === "text"} onClick={() => setTool(tool === "text" ? "none" : "text")} label="文字" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><text x="4" y="14" fontSize="14" fontWeight="bold" fill="currentColor">A</text></svg>
          } />
          <div style={{ width: 1, height: 20, background: "rgba(0,0,0,0.1)", margin: "0 4px" }} />
          <ToolButton onClick={() => setAnnotations(annotations.slice(0, -1))} label="撤销" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><path d="M4 8H11C13 8 15 10 15 12C15 14 13 16 11 16H7" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/><path d="M6 4L2 8L6 12" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/></svg>
          } />
          <div style={{ width: 1, height: 20, background: "rgba(0,0,0,0.1)", margin: "0 4px" }} />
          <button onClick={doConfirm} style={{ padding: "4px 12px", borderRadius: 6, border: "none", background: "#3b82f6", color: "#fff", fontSize: 13, fontWeight: 600, cursor: "pointer" }}>
            ✓ 确认
          </button>
          <button onClick={() => invoke("cancel_screenshot").catch(() => {})} style={{ padding: "4px 10px", borderRadius: 6, border: "1px solid rgba(0,0,0,0.15)", background: "#fff", color: "#333", fontSize: 13, cursor: "pointer" }}>
            ✕
          </button>
        </div>
      )}
    </>
  );
}

function ToolButton({ active, onClick, label, icon }: { active?: boolean; onClick: () => void; label: string; icon: React.ReactNode }) {
  return (
    <button
      onClick={onClick}
      title={label}
      style={{
        width: 32, height: 32,
        display: "flex", alignItems: "center", justifyContent: "center",
        borderRadius: 6,
        border: "none",
        background: active ? "#3b82f6" : "transparent",
        color: active ? "#fff" : "#333",
        cursor: "pointer",
        transition: "background 0.15s",
      }}
    >
      {icon}
    </button>
  );
}
