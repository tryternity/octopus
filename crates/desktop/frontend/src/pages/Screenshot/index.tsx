import { useState, useRef, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";

interface Selection {
  x: number; y: number; w: number; h: number;
}

type Mode = "idle" | "selecting" | "selected" | "move" | "resize";
type Tool = "none" | "rect" | "line" | "arrow" | "pen" | "text" | "number";

interface Annotation {
  type: "rect" | "line" | "arrow" | "pen" | "text" | "number";
  x1: number; y1: number; x2: number; y2: number;
  text?: string;
  points?: number[][];
  color?: string;
  lineWidth?: number;
  fontSize?: number;
  number?: number;
  circleSize?: number;
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
  const textDraftRef = useRef<{ x: number; y: number; val: string } | null>(null);
  const toolColorRef = useRef("#ef4444");
  const toolFontSizeRef = useRef(16);
  const [selectedAnn, setSelectedAnn] = useState<number | null>(null);
  const [toolColor, setToolColorState] = useState("#ef4444");
  const [toolWidth, setToolWidth] = useState(3);
  const [toolFontSize, setToolFontSizeState] = useState(16);
  const setToolColor = (c: string) => { toolColorRef.current = c; setToolColorState(c); };
  const setToolFontSize = (s: number) => { toolFontSizeRef.current = s; setToolFontSizeState(s); };
  const [numberCounter, setNumberCounter] = useState(1);
  const [toolCircleSize, setToolCircleSize] = useState(24);
  const annMoveStartRef = useRef<{ idx: number; mx: number; my: number; anns: Annotation[] } | null>(null);

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

      for (let i = 0; i < annotations.length; i++) {
        drawAnnotation(ctx, annotations[i]);
        if (selectedAnn === i) {
          const b = annBounds(annotations[i]);
          ctx.strokeStyle = "#3b82f6";
          ctx.lineWidth = 1;
          ctx.setLineDash([4, 4]);
          ctx.strokeRect(b.x, b.y, b.w, b.h);
          ctx.setLineDash([]);
        }
      }
      if (drawingRef.current) {
        drawAnnotation(ctx, drawingRef.current);
      }
      // textDraft 不在 Canvas 画（DOM textarea 已显示），避免重影

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
  }, [sel, mode, ready, dpr, annotations, textDraft, tool, selectedAnn]);

  useEffect(() => { draw(); }, [draw]);

  function drawAnnotation(ctx: CanvasRenderingContext2D, ann: Annotation) {
    const color = ann.color || "#ef4444";
    const lw = ann.lineWidth || 3;
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineWidth = lw;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";

    if (ann.type === "rect") {
      const x = Math.min(ann.x1, ann.x2);
      const y = Math.min(ann.y1, ann.y2);
      const w = Math.abs(ann.x2 - ann.x1);
      const h = Math.abs(ann.y2 - ann.y1);
      ctx.strokeRect(x, y, w, h);
    } else if (ann.type === "line") {
      ctx.beginPath();
      ctx.moveTo(ann.x1, ann.y1);
      ctx.lineTo(ann.x2, ann.y2);
      ctx.stroke();
    } else if (ann.type === "arrow") {
      const dx = ann.x2 - ann.x1;
      const dy = ann.y2 - ann.y1;
      const len = Math.sqrt(dx * dx + dy * dy);
      if (len < 5) return;
      ctx.beginPath();
      ctx.moveTo(ann.x1, ann.y1);
      ctx.lineTo(ann.x2, ann.y2);
      ctx.stroke();
      const angle = Math.atan2(dy, dx);
      const headLen = 12;
      ctx.beginPath();
      ctx.moveTo(ann.x2, ann.y2);
      ctx.lineTo(ann.x2 - headLen * Math.cos(angle - Math.PI / 6), ann.y2 - headLen * Math.sin(angle - Math.PI / 6));
      ctx.lineTo(ann.x2 - headLen * Math.cos(angle + Math.PI / 6), ann.y2 - headLen * Math.sin(angle + Math.PI / 6));
      ctx.closePath();
      ctx.fill();
    } else if (ann.type === "pen" && ann.points) {
      ctx.beginPath();
      for (let i = 0; i < ann.points.length; i++) {
        const [px, py] = ann.points[i];
        if (i === 0) ctx.moveTo(px, py);
        else ctx.lineTo(px, py);
      }
      ctx.stroke();
    } else if (ann.type === "text" && ann.text) {
      const fs = ann.fontSize || 16;
      ctx.font = `${fs}px -apple-system, sans-serif`;
      ctx.textBaseline = "top";
      ctx.fillText(ann.text, ann.x1, ann.y1);
    } else if (ann.type === "number" && ann.number) {
      const r = (ann.circleSize || 24) / 2;
      const fs = (ann.circleSize || 24) * 0.6;
      ctx.beginPath();
      ctx.arc(ann.x1, ann.y1, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = "#ffffff";
      ctx.font = `bold ${fs}px -apple-system, sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(ann.number), ann.x1, ann.y1);
      ctx.textAlign = "start";
    }
  }

  // 合成到原图分辨率时用——坐标、线宽、字号全部 × scale
  function drawAnnotationScaled(ctx: CanvasRenderingContext2D, ann: Annotation, scale: number) {
    const color = ann.color || "#ef4444";
    const lw = (ann.lineWidth || 3) * scale;
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineWidth = lw;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";

    if (ann.type === "rect") {
      const x = Math.min(ann.x1, ann.x2) * scale;
      const y = Math.min(ann.y1, ann.y2) * scale;
      const w = Math.abs(ann.x2 - ann.x1) * scale;
      const h = Math.abs(ann.y2 - ann.y1) * scale;
      ctx.strokeRect(x, y, w, h);
    } else if (ann.type === "line") {
      ctx.beginPath();
      ctx.moveTo(ann.x1 * scale, ann.y1 * scale);
      ctx.lineTo(ann.x2 * scale, ann.y2 * scale);
      ctx.stroke();
    } else if (ann.type === "arrow") {
      const ax1 = ann.x1 * scale, ay1 = ann.y1 * scale;
      const ax2 = ann.x2 * scale, ay2 = ann.y2 * scale;
      const dx = ax2 - ax1, dy = ay2 - ay1;
      const len = Math.sqrt(dx * dx + dy * dy);
      if (len < 5 * scale) return;
      ctx.beginPath();
      ctx.moveTo(ax1, ay1);
      ctx.lineTo(ax2, ay2);
      ctx.stroke();
      const angle = Math.atan2(dy, dx);
      const headLen = 12 * scale;
      ctx.beginPath();
      ctx.moveTo(ax2, ay2);
      ctx.lineTo(ax2 - headLen * Math.cos(angle - Math.PI / 6), ay2 - headLen * Math.sin(angle - Math.PI / 6));
      ctx.lineTo(ax2 - headLen * Math.cos(angle + Math.PI / 6), ay2 - headLen * Math.sin(angle + Math.PI / 6));
      ctx.closePath();
      ctx.fill();
    } else if (ann.type === "pen" && ann.points) {
      ctx.beginPath();
      for (let i = 0; i < ann.points.length; i++) {
        const [px, py] = ann.points[i];
        if (i === 0) ctx.moveTo(px * scale, py * scale);
        else ctx.lineTo(px * scale, py * scale);
      }
      ctx.stroke();
    } else if (ann.type === "text" && ann.text) {
      const fs = (ann.fontSize || 16) * scale;
      ctx.font = `${fs}px -apple-system, sans-serif`;
      ctx.textBaseline = "top";
      ctx.fillText(ann.text, ann.x1 * scale, ann.y1 * scale);
    } else if (ann.type === "number" && ann.number) {
      const r = ((ann.circleSize || 24) * scale) / 2;
      const fs = ((ann.circleSize || 24) * scale) * 0.6;
      const cx = ann.x1 * scale;
      const cy = ann.y1 * scale;
      ctx.beginPath();
      ctx.arc(cx, cy, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = "#ffffff";
      ctx.font = `bold ${fs}px -apple-system, sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(ann.number), cx, cy);
      ctx.textAlign = "start";
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

  function annBounds(ann: Annotation): { x: number; y: number; w: number; h: number } {
    if (ann.type === "text") {
      return { x: ann.x1 - 2, y: ann.y1 - 2, w: 200, h: (ann.fontSize || 16) + 6 };
    }
    if (ann.type === "number") {
      const r = (ann.circleSize || 24) / 2 + 2;
      return { x: ann.x1 - r, y: ann.y1 - r, w: r * 2, h: r * 2 };
    }
    if (ann.type === "pen" && ann.points && ann.points.length > 0) {
      const xs = ann.points.map(p => p[0]);
      const ys = ann.points.map(p => p[1]);
      return {
        x: Math.min(...xs) - 4, y: Math.min(...ys) - 4,
        w: Math.max(...xs) - Math.min(...xs) + 8,
        h: Math.max(...ys) - Math.min(...ys) + 8,
      };
    }
    return {
      x: Math.min(ann.x1, ann.x2) - 4,
      y: Math.min(ann.y1, ann.y2) - 4,
      w: Math.abs(ann.x2 - ann.x1) + 8,
      h: Math.abs(ann.y2 - ann.y1) + 8,
    };
  }

  function hitTestAnnotation(mx: number, my: number): number | null {
    for (let i = annotations.length - 1; i >= 0; i--) {
      const b = annBounds(annotations[i]);
      if (mx >= b.x && mx <= b.x + b.w && my >= b.y && my <= b.y + b.h) {
        return i;
      }
    }
    return null;
  }

  // 鼠标事件
  function onMouseDown(e: React.MouseEvent) {
    const mx = e.clientX;
    const my = e.clientY;
    startPtRef.current = { x: mx, y: my };

    // 文字标注正在输入时，点击其他地方 = 确认当前文字
    if (textDraftRef.current) {
      const draft = textDraftRef.current;
      if (draft.val.trim()) {
        setAnnotations(prev => [...prev, { type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: toolColorRef.current, fontSize: toolFontSizeRef.current }]);
      }
      textDraftRef.current = null;
      setTextDraft(null);
      // 如果点击的还是选区内 + 文字工具，开新的文字输入
      if (tool === "text" && sel && inSelection(mx, my)) {
        setTextDraft({ x: mx, y: my, val: "" });
        textDraftRef.current = { x: mx, y: my, val: "" };
        setTimeout(() => textInputRef.current?.focus(), 10);
      }
      return;
    }

    // 任何工具状态下：优先检测是否点中了已有标注（选中+拖动）
    if (sel && inSelection(mx, my)) {
      const annIdx = hitTestAnnotation(mx, my);
      if (annIdx !== null) {
        setSelectedAnn(annIdx);
        annMoveStartRef.current = { idx: annIdx, mx, my, anns: [...annotations] };
        return;
      }
    }

    // 标注工具激活时，在选区内绘制新标注
    if (tool !== "none" && sel && inSelection(mx, my)) {
      if (tool === "text") {
        setTextDraft({ x: mx, y: my, val: "" });
        textDraftRef.current = { x: mx, y: my, val: "" };
        setTimeout(() => textInputRef.current?.focus(), 10);
        return;
      }
      if (tool === "number") {
        setAnnotations(prev => [...prev, {
          type: "number", x1: mx, y1: my, x2: mx, y2: my,
          number: numberCounter, color: toolColorRef.current, circleSize: toolCircleSize,
        }]);
        setNumberCounter(numberCounter + 1);
        return;
      }
      if (tool === "pen") {
        drawingRef.current = { type: "pen", x1: mx, y1: my, x2: mx, y2: my, points: [[mx, my]], color: toolColor, lineWidth: toolWidth };
      } else {
        drawingRef.current = { type: tool, x1: mx, y1: my, x2: mx, y2: my, color: toolColor, lineWidth: toolWidth };
      }
      return;
    }

    // tool 为 none 时：选区内空白点击取消选中
    if (tool === "none" && sel && inSelection(mx, my)) {
      setSelectedAnn(null);
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
      if (drawingRef.current.type === "pen" && drawingRef.current.points) {
        drawingRef.current.points.push([mx, my]);
      } else {
        drawingRef.current = { ...drawingRef.current, x2: mx, y2: my };
      }
      draw();
      return;
    }

    // 标注拖动中
    if (annMoveStartRef.current) {
      const { idx, mx: sx, my: sy, anns } = annMoveStartRef.current;
      const dx = mx - sx;
      const dy = my - sy;
      const orig = anns[idx];
      const moved: Annotation = {
        ...orig,
        x1: orig.x1 + dx, y1: orig.y1 + dy,
        x2: orig.x2 + dx, y2: orig.y2 + dy,
      };
      const newAnns = [...anns];
      newAnns[idx] = moved;
      setAnnotations(newAnns);
      return;
    }

    if (mode === "idle" || mode === "selected") {
      // 悬停在标注上显示 move 光标
      if (sel && inSelection(mx, my) && hitTestAnnotation(mx, my) !== null) {
        (e.currentTarget as HTMLCanvasElement).style.cursor = "move";
      } else {
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
    if (annMoveStartRef.current) {
      annMoveStartRef.current = null;
      return;
    }

    if (drawingRef.current) {
      const ann = drawingRef.current;
      drawingRef.current = null;
      // 过滤太小的
      if (ann.type === "rect") {
        if (Math.abs(ann.x2 - ann.x1) > 5 && Math.abs(ann.y2 - ann.y1) > 5) {
          setAnnotations(prev => [...prev, ann]);
        }
      } else if (ann.type === "line" || ann.type === "arrow") {
        const dx = ann.x2 - ann.x1;
        const dy = ann.y2 - ann.y1;
        if (Math.sqrt(dx * dx + dy * dy) > 10) {
          setAnnotations(prev => [...prev, ann]);
        }
      } else if (ann.type === "pen" && ann.points) {
        if (ann.points.length > 2) {
          setAnnotations(prev => [...prev, ann]);
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

  function doSaveFile() {
    if (!sel || !bgImgRef.current) return;
    const bg = bgImgRef.current;
    const cssW = window.innerWidth;
    const natW = bg.naturalWidth;
    const scale = natW / cssW;

    const tmpCanvas = document.createElement("canvas");
    tmpCanvas.width = bg.naturalWidth;
    tmpCanvas.height = bg.naturalHeight;
    const tmpCtx = tmpCanvas.getContext("2d")!;
    tmpCtx.drawImage(bg, 0, 0);
    for (const ann of annotations) {
      drawAnnotationScaled(tmpCtx, ann, scale);
    }

    const px = Math.round(sel.x * scale);
    const py = Math.round(sel.y * scale);
    const pw = Math.round(sel.w * scale);
    const ph = Math.round(sel.h * scale);
    const croppedCanvas = document.createElement("canvas");
    croppedCanvas.width = pw;
    croppedCanvas.height = ph;
    const croppedCtx = croppedCanvas.getContext("2d")!;
    croppedCtx.drawImage(tmpCanvas, px, py, pw, ph, 0, 0, pw, ph);

    const dataUrl = croppedCanvas.toDataURL("image/png");
    const base64 = dataUrl.split(",")[1];
    // 弹系统保存对话框
    invoke("save_screenshot_dialog", {
      pngBase64: base64,
    }).catch(() => {});
  }

  function doConfirm() {
    if (!sel || !bgImgRef.current) return;
    const bg = bgImgRef.current;
    const cssW = window.innerWidth;
    const natW = bg.naturalWidth;
    const natH = bg.naturalHeight;
    const scale = natW / cssW; // 原图/显示比例（标注线宽和字号需放大）

    // 临时 Canvas = 原图原始分辨率，1:1 无缩放
    const tmpCanvas = document.createElement("canvas");
    tmpCanvas.width = natW;
    tmpCanvas.height = natH;
    const tmpCtx = tmpCanvas.getContext("2d")!;
    tmpCtx.drawImage(bg, 0, 0);

    // 标注：坐标 × scale 转原图像素，线宽和字号也 × scale
    for (const ann of annotations) {
      drawAnnotationScaled(tmpCtx, ann, scale);
    }

    // 裁剪选区（CSS 坐标 × scale → 原图像素）
    const px = Math.round(sel.x * scale);
    const py = Math.round(sel.y * scale);
    const pw = Math.round(sel.w * scale);
    const ph = Math.round(sel.h * scale);
    const croppedCanvas = document.createElement("canvas");
    croppedCanvas.width = pw;
    croppedCanvas.height = ph;
    const croppedCtx = croppedCanvas.getContext("2d")!;
    croppedCtx.drawImage(tmpCanvas, px, py, pw, ph, 0, 0, pw, ph);

    // 转 base64 → 发送给后端
    const dataUrl = croppedCanvas.toDataURL("image/png");
    const base64 = dataUrl.split(",")[1];
    invoke("confirm_screenshot_with_data", {
      label: winLabel,
      pngBase64: base64,
      width: pw,
      height: ph,
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
          onChange={(e) => {
            const updated = { ...textDraft, val: e.target.value };
            textDraftRef.current = updated;
            setTextDraft(updated);
          }}
          onBlur={() => {
            const draft = textDraftRef.current;
            if (draft && draft.val.trim()) {
              setAnnotations(prev => [...prev, { type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: toolColorRef.current, fontSize: toolFontSizeRef.current }]);
            }
            textDraftRef.current = null;
            setTextDraft(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") { textDraftRef.current = null; setTextDraft(null); }
            e.stopPropagation();
          }}
          style={{
            position: "fixed",
            left: textDraft.x,
            top: textDraft.y,
            fontSize: toolFontSize,
            color: toolColor,
            background: "transparent",
            border: `1px dashed ${toolColor}`,
            outline: "none",
            resize: "none",
            padding: "2px 4px",
            minHeight: toolFontSize + 8,
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
          <ToolButton active={tool === "line"} onClick={() => setTool(tool === "line" ? "none" : "line")} label="直线" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><line x1="3" y1="15" x2="15" y2="3" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/></svg>
          } />
          <ToolButton active={tool === "arrow"} onClick={() => setTool(tool === "arrow" ? "none" : "arrow")} label="箭头" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><path d="M3 15L15 3M15 3L10 3M15 3L15 8" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/></svg>
          } />
          <ToolButton active={tool === "pen"} onClick={() => setTool(tool === "pen" ? "none" : "pen")} label="画笔" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><path d="M3 14C5 12 7 10 9 8C11 6 13 5 15 3" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/><circle cx="3.5" cy="14.5" r="1.5" fill="currentColor"/></svg>
          } />
          <ToolButton active={tool === "text"} onClick={() => setTool(tool === "text" ? "none" : "text")} label="文字" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><text x="4" y="14" fontSize="14" fontWeight="bold" fill="currentColor">A</text></svg>
          } />
          <ToolButton active={tool === "number"} onClick={() => { setTool(tool === "number" ? "none" : "number"); setNumberCounter(1); }} label="序号" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="6.5" fill="currentColor"/><text x="9" y="13" fontSize="10" fontWeight="bold" fill="white" textAnchor="middle">1</text></svg>
          } />
          <div style={{ width: 1, height: 20, background: "rgba(0,0,0,0.1)", margin: "0 4px" }} />
          <ToolButton onClick={() => setAnnotations(annotations.slice(0, -1))} label="撤销" icon={
            <svg width="18" height="18" viewBox="0 0 18 18" fill="none"><path d="M4 8H11C13 8 15 10 15 12C15 14 13 16 11 16H7" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/><path d="M6 4L2 8L6 12" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/></svg>
          } />
          <div style={{ width: 1, height: 20, background: "rgba(0,0,0,0.1)", margin: "0 4px" }} />
          <button onClick={doSaveFile} title="保存到文件" style={{ padding: "4px 8px", borderRadius: 6, border: "1px solid rgba(0,0,0,0.15)", background: "#fff", color: "#333", fontSize: 13, cursor: "pointer", display: "flex", alignItems: "center", gap: 4 }}>
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none"><path d="M7 1V9M7 9L4 6M7 9L10 6M2 11V13H12V11" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/></svg>
            保存
          </button>
          <button onClick={doConfirm} style={{ padding: "4px 12px", borderRadius: 6, border: "none", background: "#3b82f6", color: "#fff", fontSize: 13, fontWeight: 600, cursor: "pointer" }}>
            ✓ 确认
          </button>
          <button onClick={() => invoke("cancel_screenshot").catch(() => {})} style={{ padding: "4px 10px", borderRadius: 6, border: "1px solid rgba(0,0,0,0.15)", background: "#fff", color: "#333", fontSize: 13, cursor: "pointer" }}>
            ✕
          </button>
        </div>
      )}

      {/* 工具属性浮窗 */}
      {sel && mode === "selected" && tool !== "none" && (
        <ToolPropsPopover
          x={toolbarX}
          y={toolbarY + 44}
          color={toolColor}
          width={toolWidth}
          fontSize={toolFontSize}
          circleSize={toolCircleSize}
          isText={tool === "text"}
          isNumber={tool === "number"}
          onColorChange={setToolColor}
          onWidthChange={setToolWidth}
          onFontSizeChange={setToolFontSize}
          onCircleSizeChange={setToolCircleSize}
        />
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

const PRESET_COLORS = ["#ef4444", "#f97316", "#eab308", "#22c55e", "#3b82f6", "#8b5cf6", "#000000", "#ffffff"];

function ToolPropsPopover({
  x, y, color, width, fontSize, circleSize, isText, isNumber, onColorChange, onWidthChange, onFontSizeChange, onCircleSizeChange,
}: {
  x: number; y: number;
  color: string; width: number; fontSize: number; circleSize: number; isText: boolean; isNumber: boolean;
  onColorChange: (c: string) => void;
  onWidthChange: (w: number) => void;
  onFontSizeChange: (s: number) => void;
  onCircleSizeChange: (s: number) => void;
}) {
  const sizeValue = isText ? fontSize : isNumber ? circleSize : width;
  const setSize = isText ? onFontSizeChange : isNumber ? onCircleSizeChange : onWidthChange;
  const min = isText ? 10 : isNumber ? 16 : 1;
  const max = isText ? 48 : isNumber ? 60 : 10;
  const label = isText ? "字号" : isNumber ? "圆圈" : "粗细";

  return (
    <div
      style={{
        position: "fixed",
        left: x,
        top: y,
        padding: "8px 10px",
        background: "rgba(255,255,255,0.97)",
        borderRadius: 8,
        boxShadow: "0 4px 16px rgba(0,0,0,0.2)",
        zIndex: 101,
        display: "flex",
        flexDirection: "column",
        gap: 8,
        width: 200,
      }}
    >
      {/* 第一行：粗细/字号滑轨 */}
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <span style={{ fontSize: 10, color: "#888", width: 24 }}>{label}</span>
        <input
          type="range"
          min={min}
          max={max}
          value={sizeValue}
          onChange={(e) => setSize(Number(e.target.value))}
          style={{ flex: 1, height: 3, accentColor: color, cursor: "pointer" }}
        />
        <span style={{ fontSize: 10, color: "#555", fontVariantNumeric: "tabular-nums", width: 20, textAlign: "right" }}>{sizeValue}</span>
      </div>

      {/* 第二行：当前色 + 预设色 */}
      <div style={{ display: "flex", alignItems: "center", gap: 5 }}>
        <div style={{
          width: 18, height: 18, borderRadius: 4,
          background: color,
          border: "2px solid #fff",
          boxShadow: "0 0 0 1px rgba(0,0,0,0.2)",
          flexShrink: 0,
        }} />
        {PRESET_COLORS.map((c) => (
          <button
            key={c}
            onClick={() => onColorChange(c)}
            style={{
              width: 16, height: 16, borderRadius: 4,
              background: c,
              border: c === "#ffffff" ? "1px solid #ddd" : "none",
              cursor: "pointer",
              padding: 0,
              opacity: color === c ? 1 : 0.6,
              transition: "opacity 0.15s",
            }}
          />
        ))}
      </div>

      {/* 第三行：自定义调色板 */}
      <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
        <label style={{ position: "relative", display: "flex", alignItems: "center", gap: 4, cursor: "pointer" }}>
          <span style={{ fontSize: 10, color: "#888" }}>自定义</span>
          <input
            type="color"
            value={color}
            onChange={(e) => onColorChange(e.target.value)}
            style={{ width: 24, height: 18, border: "1px solid #ddd", borderRadius: 4, cursor: "pointer", padding: 0 }}
          />
        </label>
      </div>
    </div>
  );
}
