import { useState, useRef, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";

interface Selection {
  x: number; y: number; w: number; h: number;
}

type Mode = "idle" | "selecting" | "selected" | "move" | "resize" | "scrolling";
type Tool = "none" | "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";

interface Annotation {
  type: "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";
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

  const setModeSafe = (m: Mode) => { modeRef.current = m; setMode(m); };
  const [mode, setMode] = useState<Mode>("idle");
  const [sel, setSel] = useState<Selection | null>(null);
  const [resizeHandle, setResizeHandle] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [tool, setTool] = useState<Tool>("none");
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [textDraft, setTextDraft] = useState<{ x: number; y: number; val: string } | null>(null);
  const textDraftRef = useRef<{ x: number; y: number; val: string } | null>(null);
  const modeRef = useRef<Mode>("idle");
  const toolColorRef = useRef("#ef4444");
  const toolFontSizeRef = useRef(16);
  const editTextColorRef = useRef<string | null>(null);
  const editTextFontSizeRef = useRef<number | null>(null);
  const editTextOrigRef = useRef<{ idx: number; text: string; color: string; fontSize: number } | null>(null);
  const [selectedAnn, setSelectedAnn] = useState<number | null>(null);
  const [scrollPreview, setScrollPreview] = useState<string | null>(null);
  const [scrollHeight, setScrollHeight] = useState(0);
  const scrollFrameRef = useRef<HTMLImageElement | null>(null);
  const [toolColor, setToolColorState] = useState("#ef4444");
  const [toolWidth, setToolWidth] = useState(3);
  const [toolFontSize, setToolFontSizeState] = useState(16);
  const setToolColor = (c: string) => { toolColorRef.current = c; setToolColorState(c); };
  const setToolFontSize = (s: number) => { toolFontSizeRef.current = s; setToolFontSizeState(s); };
  const scrollSaveAfterStopRef = useRef(false);
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
          setTimeout(() => { invoke("show_screenshot_window").catch(() => {}); }, 50);
        };
        img.src = `data:image/jpeg;base64,${data.image}`;
      })
      .catch((e) => console.error("Failed to get screenshot image:", e));
  }, []);

  // 全局 Escape 监听（保险：Canvas 未获取焦点时也能 ESC 取消）
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape" && modeRef.current === "idle") {
        invoke("cancel_screenshot").catch(() => {});
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // 滚动截图事件监听
  useEffect(() => {
    let unlistenFrame: (() => void) | undefined;
    let unlistenDone: (() => void) | undefined;
    listen<{ frame: string; preview: string; height: number; phys_height: number }>("scroll://frame", (e) => {
      // 实时选区画面：加载为 Image → Canvas 重绘
      const img = new Image();
      img.onload = () => { scrollFrameRef.current = img; draw(); };
      img.src = `data:image/jpeg;base64,${e.payload.frame}`;
      setScrollPreview(e.payload.preview);
      setScrollHeight(e.payload.phys_height);
    }).then((fn) => { unlistenFrame = fn; });
    listen("scroll://done", (e: { payload: { id?: string; png_base64?: string } }) => {
      setScrollPreview(null);
      setModeSafe("selected");
      // 如果用户点的是"保存"，触发保存文件对话框
      if (scrollSaveAfterStopRef.current && e.payload.png_base64) {
        scrollSaveAfterStopRef.current = false;
        invoke("save_screenshot_dialog", { pngBase64: e.payload.png_base64 }).catch(() => {});
      }
      scrollSaveAfterStopRef.current = false;
    }).then((fn) => { unlistenDone = fn; });
    return () => { unlistenFrame?.(); unlistenDone?.(); };
  }, []);

  // 初始化 Canvas 尺寸（仅一次，避免高频重分配 GPU 缓冲区）
  const canvasInitedRef = useRef(false);
  useEffect(() => {
    if (!ready || canvasInitedRef.current) return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const cssW = window.innerWidth;
    const cssH = window.innerHeight;
    canvas.width = cssW * dpr;
    canvas.height = cssH * dpr;
    canvasInitedRef.current = true;
  }, [ready, dpr]);

  // 绘制
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const bg = bgImgRef.current;
    if (!canvas || !bg || !ready) return;

    const cssW = window.innerWidth;
    const cssH = window.innerHeight;
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);

    // 滚动模式：Canvas 只画绿色边框，遮罩用 DOM div 实现（避免 Canvas clearRect 残留）
    if (mode === "scrolling" && sel) {
      const { x, y, w, h } = sel;
      // 全屏清空（Canvas 不画遮罩，避免选区内有任何像素残留）
      ctx.clearRect(0, 0, cssW, cssH);
      // 绿色边框
      ctx.strokeStyle = "#22c55e";
      ctx.lineWidth = 2;
      ctx.strokeRect(x, y, w, h);
      return;
    }

    // 普通模式
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

        // 尺寸标注（左上角或左下角，取决于工具栏位置）
        const label = `${Math.round(w * dpr)} × ${Math.round(h * dpr)}`;
        ctx.font = "12px -apple-system, sans-serif";
        const tw = ctx.measureText(label).width;
        const labelY = toolbarBelow ? (y - 18) : (y + h + 4);
        const labelVisibleY = Math.max(0, labelY);
        ctx.fillStyle = "rgba(255, 255, 255, 0.9)";
        ctx.fillRect(x, labelVisibleY, tw + 8, 18);
        ctx.fillStyle = "#1a1a1a";
        ctx.fillText(label, x + 4, labelVisibleY + 13);
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
    } else if (ann.type === "oval") {
      const cx = (ann.x1 + ann.x2) / 2;
      const cy = (ann.y1 + ann.y2) / 2;
      const rx = Math.max(1, Math.abs(ann.x2 - ann.x1) / 2);
      const ry = Math.max(1, Math.abs(ann.y2 - ann.y1) / 2);
      ctx.beginPath();
      ctx.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
      ctx.stroke();
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
      drawMultilineText(ctx, ann.text, ann.x1, ann.y1, 200, fs);
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

  // 多行文字绘制：支持 \n 换行 + 超宽自动折行
  function drawMultilineText(ctx: CanvasRenderingContext2D, text: string, x: number, y: number, maxWidth: number, fontSize: number) {
    const lineHeight = fontSize * 1.3;
    // 先按 \n 切割为段落，再每段按 maxWidth 自动折行
    const paragraphs = text.split("\n");
    let cy = y;
    for (const para of paragraphs) {
      if (para === "") {
        cy += lineHeight;
        continue;
      }
      // 按字符测宽折行（适用于 CJK + ASCII 混合）
      let line = "";
      for (const ch of para) {
        const test = line + ch;
        if (ctx.measureText(test).width > maxWidth && line.length > 0) {
          ctx.fillText(line, x, cy);
          cy += lineHeight;
          line = ch;
        } else {
          line = test;
        }
      }
      if (line) {
        ctx.fillText(line, x, cy);
        cy += lineHeight;
      }
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
    } else if (ann.type === "oval") {
      const cx = (ann.x1 + ann.x2) / 2 * scale;
      const cy = (ann.y1 + ann.y2) / 2 * scale;
      const rx = Math.max(1, Math.abs(ann.x2 - ann.x1) / 2 * scale);
      const ry = Math.max(1, Math.abs(ann.y2 - ann.y1) / 2 * scale);
      ctx.beginPath();
      ctx.ellipse(cx, cy, rx, ry, 0, 0, Math.PI * 2);
      ctx.stroke();
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
      drawMultilineText(ctx, ann.text, ann.x1 * scale, ann.y1 * scale, 200 * scale, fs);
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

  // 精确命中：空心标注（rect/oval/line/arrow）检查到线条的距离，填充标注用 bounding box
  const HIT_DIST = 8;
  function hitTestAnnotationPrecise(mx: number, my: number): number | null {
    for (let i = annotations.length - 1; i >= 0; i--) {
      const ann = annotations[i];
      if (ann.type === "rect") {
        // 矩形：检查到四条边的距离
        const x = Math.min(ann.x1, ann.x2);
        const y = Math.min(ann.y1, ann.y2);
        const w = Math.abs(ann.x2 - ann.x1);
        const h = Math.abs(ann.y2 - ann.y1);
        const onEdge = (Math.abs(mx - x) <= HIT_DIST || Math.abs(mx - (x + w)) <= HIT_DIST) && my >= y - HIT_DIST && my <= y + h + HIT_DIST
          || (Math.abs(my - y) <= HIT_DIST || Math.abs(my - (y + h)) <= HIT_DIST) && mx >= x - HIT_DIST && mx <= x + w + HIT_DIST;
        if (onEdge) return i;
      } else if (ann.type === "oval") {
        // 椭圆：检查到椭圆轮廓的距离
        const cx = (ann.x1 + ann.x2) / 2;
        const cy = (ann.y1 + ann.y2) / 2;
        const rx = Math.abs(ann.x2 - ann.x1) / 2;
        const ry = Math.abs(ann.y2 - ann.y1) / 2;
        if (rx < 1 || ry < 1) continue;
        const dx = (mx - cx) / rx;
        const dy = (my - cy) / ry;
        const dist = Math.abs(Math.sqrt(dx * dx + dy * dy) - 1) * Math.min(rx, ry);
        if (dist <= HIT_DIST) return i;
      } else if (ann.type === "line" || ann.type === "arrow") {
        // 线段：点到线段的距离
        if (pointToSegmentDist(mx, my, ann.x1, ann.y1, ann.x2, ann.y2) <= HIT_DIST) return i;
      } else if (ann.type === "pen" && ann.points) {
        // 自由曲线：检查到任意一段的距离
        for (let j = 1; j < ann.points.length; j++) {
          const [px1, py1] = ann.points[j - 1];
          const [px2, py2] = ann.points[j];
          if (pointToSegmentDist(mx, my, px1, py1, px2, py2) <= HIT_DIST) return i;
        }
      } else {
        // 文字/序号：bounding box
        const b = annBounds(ann);
        if (mx >= b.x && mx <= b.x + b.w && my >= b.y && my <= b.y + b.h) return i;
      }
    }
    return null;
  }

  function pointToSegmentDist(px: number, py: number, x1: number, y1: number, x2: number, y2: number): number {
    const dx = x2 - x1, dy = y2 - y1;
    const lenSq = dx * dx + dy * dy;
    if (lenSq === 0) return Math.sqrt((px - x1) ** 2 + (py - y1) ** 2);
    let t = ((px - x1) * dx + (py - y1) * dy) / lenSq;
    t = Math.max(0, Math.min(1, t));
    const projX = x1 + t * dx;
    const projY = y1 + t * dy;
    return Math.sqrt((px - projX) ** 2 + (py - projY) ** 2);
  }

  // 鼠标事件
  function onMouseDown(e: React.MouseEvent) {
    if (e.button !== 0) return;
    if (mode === "scrolling") return;
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

    // 任何工具状态下：优先检测选区手柄（保证随时可调整选区大小）
    if (mode === "selected" || mode === "idle") {
      const handle = hitTest(mx, my);
      if (handle) {
        setResizeHandle(handle);
        setModeSafe("resize");
        if (sel) selStartRef.current = { ...sel };
        return;
      }
    }

    // tool === "none" 时：检测是否点中了已有标注（精确命中，空心标注内部不算命中）
    if (tool === "none" && sel && inSelection(mx, my)) {
      const annIdx = hitTestAnnotationPrecise(mx, my);
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

    // tool 为 none 时：选区内空白点击取消选中 + 允许平移选区
    if (tool === "none" && sel && inSelection(mx, my)) {
      setSelectedAnn(null);
      setModeSafe("move");
      moveStartRef.current = { x: mx, y: my };
      selStartRef.current = { ...sel };
      return;
    }

    // 选区外左键点击：已确定选区时忽略（避免误操作丢失标注），ESC 或取消按钮退出
    if (sel && mode === "selected" && !inSelection(mx, my)) {
      return;
    }

    setSel({ x: mx, y: my, w: 0, h: 0 });
    setModeSafe("selecting");
  }

  function onMouseMove(e: React.MouseEvent) {
    const mx = e.clientX;
    const my = e.clientY;

    // 滚动模式：后端每帧检查鼠标位置自动切换 cursor 穿透，前端无需处理
    if (mode === "scrolling") return;

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
        points: orig.points ? orig.points.map(([px, py]) => [px + dx, py + dy] as number[]) : undefined,
      };
      const newAnns = [...anns];
      newAnns[idx] = moved;
      setAnnotations(newAnns);
      return;
    }

    if (mode === "idle" || mode === "selected") {
      // 悬停在标注上显示 move 光标
      if (sel && inSelection(mx, my) && hitTestAnnotationPrecise(mx, my) !== null) {
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
      let added = false;
      // 过滤太小的
      if (ann.type === "rect" || ann.type === "oval") {
        if (Math.abs(ann.x2 - ann.x1) > 5 && Math.abs(ann.y2 - ann.y1) > 5) {
          setAnnotations(prev => [...prev, ann]);
          added = true;
        }
      } else if (ann.type === "line" || ann.type === "arrow") {
        const dx = ann.x2 - ann.x1;
        const dy = ann.y2 - ann.y1;
        if (Math.sqrt(dx * dx + dy * dy) > 10) {
          setAnnotations(prev => [...prev, ann]);
          added = true;
        }
      } else if (ann.type === "pen" && ann.points) {
        if (ann.points.length > 2) {
          setAnnotations(prev => [...prev, ann]);
          added = true;
        }
      }
      // 丢弃时重绘 Canvas 消除残留影像
      if (!added) {
        draw();
      }
      return;
    }

    if (mode === "selecting" && sel) {
      if (sel.w < MIN_SIZE || sel.h < MIN_SIZE) { setSel(null); setModeSafe("idle"); }
      else { setModeSafe("selected"); }
    } else if (mode === "move" || mode === "resize") {
      setModeSafe("selected");
      setResizeHandle(null);
    }
  }

  function onKeyDown(e: React.KeyboardEvent) {
    if (textDraft) return;
    if (mode === "scrolling") {
      if (e.key === "Escape") { stopScroll(); }
      return;
    }
    if (e.key === "Escape") {
      if (tool !== "none") { setTool("none"); return; }
      invoke("cancel_screenshot").catch(() => {});
    } else if (e.key === "Enter" && sel && sel.w >= MIN_SIZE && sel.h >= MIN_SIZE) {
      doConfirm();
    } else if ((e.metaKey || e.ctrlKey) && e.key === "z") {
      // 撤销：删除最后一个标注，序号回退
      if (annotations.length > 0) {
        const removed = annotations[annotations.length - 1];
        if (removed.type === "number" && removed.number === numberCounter - 1) {
          setNumberCounter(numberCounter - 1);
        }
        setAnnotations(annotations.slice(0, -1));
      }
    } else if ((e.key === "Delete" || e.key === "Backspace") && selectedAnn !== null) {
      // 删除选中的标注
      const removed = annotations[selectedAnn];
      if (removed.type === "number" && removed.number === numberCounter - 1) {
        setNumberCounter(numberCounter - 1);
      }
      setAnnotations(annotations.filter((_, i) => i !== selectedAnn));
      setSelectedAnn(null);
    }
  }

  function onDoubleClick(e: React.MouseEvent) {
    if (e.button !== 0) return;
    const mx = e.clientX;
    const my = e.clientY;
    if (!sel || !inSelection(mx, my)) return;
    const annIdx = hitTestAnnotationPrecise(mx, my);
    if (annIdx === null) return;
    const ann = annotations[annIdx];
    if (ann.type !== "text" || !ann.text) return;
    // 记住原标注（ESC 可恢复），不立即删除
    const origColor = ann.color || "#ef4444";
    const origFontSize = ann.fontSize || 16;
    editTextOrigRef.current = { idx: annIdx, text: ann.text, color: origColor, fontSize: origFontSize };
    editTextColorRef.current = origColor;
    editTextFontSizeRef.current = origFontSize;
    // 隐藏原标注（标记为编辑中，Canvas 不绘制）
    setAnnotations(prev => prev.map((a, i) => i === annIdx ? { ...a, text: "" } : a));
    setSelectedAnn(null);
    setTextDraft({ x: ann.x1, y: ann.y1, val: ann.text });
    textDraftRef.current = { x: ann.x1, y: ann.y1, val: ann.text };
    setTimeout(() => textInputRef.current?.focus(), 10);
  }

  function onContextMenu(e: React.MouseEvent) {
    e.preventDefault();
    // idle 模式（未框选）右键取消截图
    if (mode === "idle") {
      invoke("cancel_screenshot").catch(() => {});
    }
  }

  function startScroll() {
    if (!sel) return;
    setModeSafe("scrolling");
    setTool("none");
    setScrollPreview(null);
    setScrollHeight(0);

    // 计算交互区域（只有预览窗，scrolling 模式下工具栏已隐藏）
    const interactiveRects: Array<{x: number; y: number; width: number; height: number}> = [];
    // 预览窗（右侧优先，空间不足放左侧）
    const previewLeft = sel.x + sel.w + 12 + 200 <= window.innerWidth
      ? sel.x + sel.w + 12
      : sel.x - 12 - 200;
    // 预览窗底部固定，高度最大 80vh
    interactiveRects.push({ x: previewLeft, y: window.innerHeight * 2 / 10, width: 200, height: window.innerHeight * 8 / 10 });

    invoke("start_scroll_recording", {
      x: sel.x, y: sel.y, w: sel.w, h: sel.h,
      winLabel: winLabel,
      interactiveRects,
    }).catch(() => setModeSafe("selected"));
  }

  function stopScroll() {
    invoke("stop_scroll_recording").catch(() => {});
  }

  function composeAndCrop(): string | null {
    if (!sel || !bgImgRef.current) return null;
    const bg = bgImgRef.current;
    const scale = bg.naturalWidth / window.innerWidth;

    // 合并已确认标注 + 未提交的文字输入（避免 onBlur 竞态丢失）
    const allAnns = [...annotations];
    const draft = textDraftRef.current;
    if (draft && draft.val.trim()) {
      allAnns.push({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: editTextColorRef.current || toolColorRef.current, fontSize: editTextFontSizeRef.current || toolFontSizeRef.current });
    }

    const tmpCanvas = document.createElement("canvas");
    tmpCanvas.width = bg.naturalWidth;
    tmpCanvas.height = bg.naturalHeight;
    const tmpCtx = tmpCanvas.getContext("2d")!;
    tmpCtx.drawImage(bg, 0, 0);
    for (const ann of allAnns) {
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

    return croppedCanvas.toDataURL("image/png").split(",")[1];
  }

  function doOcr() {
    const base64 = composeAndCrop();
    if (!base64) return;
    invoke("ocr_screenshot", { pngBase64: base64 }).catch(() => {});
  }

  function doSaveFile() {
    const base64 = composeAndCrop();
    if (!base64) return;
    invoke("save_screenshot_dialog", { pngBase64: base64 }).catch(() => {});
  }

  function doPin() {
    if (!sel) return;
    invoke("pin_screenshot", { label: winLabel, x: sel.x, y: sel.y, w: sel.w, h: sel.h }).catch(() => {});
  }

  function doConfirm() {
    const base64 = composeAndCrop();
    if (!base64) return;
    invoke("confirm_screenshot_with_data", {
      label: winLabel,
      pngBase64: base64,
      width: 0,
      height: 0,
    }).catch(() => {});
  }

  function normalize(x1: number, y1: number, x2: number, y2: number): Selection {
    const cx = Math.max(0, Math.min(Math.min(x1, x2), window.innerWidth));
    const cy = Math.max(0, Math.min(Math.min(y1, y2), window.innerHeight));
    const cw = Math.min(Math.abs(x2 - x1), window.innerWidth - cx);
    const ch = Math.min(Math.abs(y2 - y1), window.innerHeight - cy);
    return { x: cx, y: cy, w: cw, h: ch };
  }

  function resizeSel(start: Selection, handle: string, mx: number, my: number): Selection {
    const clampX = (v: number) => Math.max(0, Math.min(v, window.innerWidth));
    const clampY = (v: number) => Math.max(0, Math.min(v, window.innerHeight));
    let { x, y, w, h } = start;
    if (handle.includes("w")) { const rx = x + w; x = clampX(Math.min(mx, rx - MIN_SIZE)); w = rx - x; }
    if (handle.includes("e")) { w = Math.max(MIN_SIZE, clampX(mx) - x); }
    if (handle.includes("n")) { const by = y + h; y = clampY(Math.min(my, by - MIN_SIZE)); h = by - y; }
    if (handle.includes("s")) { h = Math.max(MIN_SIZE, clampY(my) - y); }
    return { x, y, w, h };
  }

  // 工具栏位置：默认选区下方居中，下方空间不够时放上方居中
  const belowSpace = sel ? window.innerHeight - (sel.y + sel.h + 8) : 0;
  const toolbarBelow = sel ? belowSpace >= 44 : true;
  const toolbarY = sel
    ? Math.max(0, Math.min(
        toolbarBelow ? sel.y + sel.h + 8 : sel.y - 48,
        window.innerHeight - 44
      ))
    : 0;
  // 用选区中心 + translateX(-50%) 实现真正居中，不受工具栏实际宽度影响
  const toolbarCenterX = sel ? sel.x + sel.w / 2 : 0;
  const popoverY = toolbarBelow ? toolbarY + 44 : Math.max(0, toolbarY - 100);

  if (!ready) {
    return <div style={{ width: "100vw", height: "100vh", background: "rgba(0,0,0,0.5)" }} />;
  }

  // scrolling 模式下让整个页面背景透明（消除 WebView 白底透过来的感觉）
  const pageBg = mode === "scrolling" ? "transparent" : undefined;

  return (
    <>
      <div style={{ position: "fixed", inset: 0, background: pageBg }} />
      <canvas
        ref={canvasRef}
        style={{ position: "fixed", top: 0, left: 0, width: "100vw", height: "100vh", cursor: "crosshair", outline: "none" }}
        tabIndex={0}
        autoFocus
        onMouseDown={onMouseDown}
        onDoubleClick={onDoubleClick}
        onMouseMove={onMouseMove}
        onMouseUp={onMouseUp}
        onKeyDown={onKeyDown}
        onContextMenu={onContextMenu}
      />

      {/* 滚动模式：选区外暗遮罩（DOM div，不经过 Canvas，避免选区内有像素残留导致变暗） */}
      {mode === "scrolling" && sel && (
        <>
          <div style={{ position: "fixed", left: 0, top: 0, right: 0, height: sel.y, background: "rgba(0,0,0,0.5)", pointerEvents: "none", zIndex: 50 }} />
          <div style={{ position: "fixed", left: 0, top: sel.y + sel.h, right: 0, bottom: 0, background: "rgba(0,0,0,0.5)", pointerEvents: "none", zIndex: 50 }} />
          <div style={{ position: "fixed", left: 0, top: sel.y, width: sel.x, height: sel.h, background: "rgba(0,0,0,0.5)", pointerEvents: "none", zIndex: 50 }} />
          <div style={{ position: "fixed", left: sel.x + sel.w, top: sel.y, right: 0, height: sel.h, background: "rgba(0,0,0,0.5)", pointerEvents: "none", zIndex: 50 }} />
        </>
      )}

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
            const editColor = editTextColorRef.current || toolColorRef.current;
            const editFontSize = editTextFontSizeRef.current || toolFontSizeRef.current;
            const editOrig = editTextOrigRef.current;
            if (draft && draft.val.trim()) {
              const newText = draft.val;
              if (editOrig) {
                // 编辑模式：更新原标注
                setAnnotations(prev => prev.map((a, i) => i === editOrig.idx ? { ...a, text: newText, color: editColor, fontSize: editFontSize } : a));
              } else {
                // 新建模式
                setAnnotations(prev => [...prev, { type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: newText, color: editColor, fontSize: editFontSize }]);
              }
            } else if (editOrig) {
              // 内容为空：恢复原标注
              setAnnotations(prev => prev.map((a, i) => i === editOrig.idx ? { ...a, text: editOrig.text, color: editOrig.color, fontSize: editOrig.fontSize } : a));
            }
            textDraftRef.current = null;
            setTextDraft(null);
            editTextColorRef.current = null;
            editTextFontSizeRef.current = null;
            editTextOrigRef.current = null;
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              const editOrig = editTextOrigRef.current;
              if (editOrig) {
                // ESC 恢复原标注
                setAnnotations(prev => prev.map((a, i) => i === editOrig.idx ? { ...a, text: editOrig.text, color: editOrig.color, fontSize: editOrig.fontSize } : a));
              }
              textDraftRef.current = null;
              setTextDraft(null);
              editTextColorRef.current = null;
              editTextFontSizeRef.current = null;
              editTextOrigRef.current = null;
            }
            e.stopPropagation();
          }}
          style={{
            position: "fixed",
            left: textDraft.x,
            top: textDraft.y,
            fontSize: editTextFontSizeRef.current || toolFontSize,
            color: editTextColorRef.current || toolColor,
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

      {/* 工具栏（scrolling 模式下隐藏，操作按钮在预览图中） */}
      {sel && mode !== "scrolling" && (
        <div
          style={{
            position: "fixed",
            left: toolbarCenterX,
            top: toolbarY,
            transform: "translateX(-50%)",
            display: "flex",
            gap: 4,
            padding: "6px 8px",
            background: "#fff",
            borderRadius: 8,
            boxShadow: "0 4px 16px rgba(0,0,0,0.3)",
            zIndex: 100,
            alignItems: "center",
          }}
        >
          <ToolButton active={tool === "none"} onClick={() => setTool("none")} label="选择" icon={
            <img src="icons/arrow-pointer.svg" alt="选择" className="w-[18px] h-[18px]" style={{ filter: tool === "none" ? "brightness(0) invert(1)" : "none" }} />
          } />
          <ToolButton active={tool === "rect"} onClick={() => setTool(tool === "rect" ? "none" : "rect")} label="矩形" icon={
            <img src="icons/square.svg" alt="矩形" className="w-[18px] h-[18px]" style={{ filter: tool === "rect" ? "brightness(0) invert(1)" : "none" }} />
          } />
          <ToolButton active={tool === "oval"} onClick={() => setTool(tool === "oval" ? "none" : "oval")} label="椭圆" icon={
            <img src="icons/oval-vertical.svg" alt="椭圆" className="w-[18px] h-[18px]" style={{ filter: tool === "oval" ? "brightness(0) invert(1)" : "none" }} />
          } />
          <ToolButton active={tool === "line"} onClick={() => setTool(tool === "line" ? "none" : "line")} label="直线" icon={
            <img src="icons/straight-line.svg" alt="直线" className="w-[18px] h-[18px]" style={{ filter: tool === "line" ? "brightness(0) invert(1)" : "none" }} />
          } />
          <ToolButton active={tool === "arrow"} onClick={() => setTool(tool === "arrow" ? "none" : "arrow")} label="箭头" icon={
            <img src="icons/arrow-line.svg" alt="箭头" className="w-[18px] h-[18px]" style={{ filter: tool === "arrow" ? "brightness(0) invert(1)" : "none" }} />
          } />
          <ToolButton active={tool === "pen"} onClick={() => setTool(tool === "pen" ? "none" : "pen")} label="画笔" icon={
            <img src="icons/sketching.svg" alt="画笔" className="w-[18px] h-[18px]" style={{ filter: tool === "pen" ? "brightness(0) invert(1)" : "none" }} />
          } />
          <ToolButton active={tool === "text"} onClick={() => setTool(tool === "text" ? "none" : "text")} label="文字" icon={
            <img src="icons/text.svg" alt="文字" className="w-[18px] h-[18px]" style={{ filter: tool === "text" ? "brightness(0) invert(1)" : "none" }} />
          } />
          <ToolButton active={tool === "number"} onClick={() => { setTool(tool === "number" ? "none" : "number"); setNumberCounter(1); }} label="序号" icon={
            <img src="icons/sequence-note.svg" alt="序号" className="w-[18px] h-[18px]" style={{ filter: tool === "number" ? "brightness(0) invert(1)" : "none" }} />
          } />
          <div style={{ width: 1, height: 20, background: "rgba(0,0,0,0.1)", margin: "0 4px" }} />
          <ToolButton onClick={() => {
            if (annotations.length > 0) {
              const removed = annotations[annotations.length - 1];
              if (removed.type === "number" && removed.number === numberCounter - 1) {
                setNumberCounter(numberCounter - 1);
              }
              setAnnotations(annotations.slice(0, -1));
            }
          }} label="撤销" icon={
            <img src="icons/restore.svg" alt="撤销" className="w-[18px] h-[18px]" />
          } />
          <ToolButton onClick={doOcr} label="OCR" icon={
            <img src="icons/ocr-ai.svg" alt="OCR" className="w-[18px] h-[18px]" />
          } />
          <div style={{ width: 1, height: 20, background: "rgba(0,0,0,0.1)", margin: "0 4px" }} />
          <button onClick={startScroll} title="滚动截图" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
            <img src="icons/scroll.svg" alt="滚动截图" className="w-[18px] h-[18px]" />
          </button>
          <button onClick={doSaveFile} title="保存到文件" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
            <img src="icons/save.svg" alt="保存" className="w-[18px] h-[18px]" />
          </button>
          <button onClick={doConfirm} title="确认" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "#3b82f6", cursor: "pointer" }}>
            <img src="icons/copy.svg" alt="确认" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) invert(1)" }} />
          </button>
          <button onClick={() => invoke("cancel_screenshot").catch(() => {})} title="取消" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
            <img src="icons/close.svg" alt="取消" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) saturate(100%) invert(40%) sepia(94%) saturate(7470%) hue-rotate(346deg) brightness(95%) contrast(91%)" }} />
          </button>
        </div>
      )}

      {/* 贴图按钮：选区右上角（工具栏在上方时放右下角） */}
      {sel && mode !== "scrolling" && (
        <button onClick={doPin} title="贴图" style={{
          position: "fixed",
          left: sel.x + sel.w - 32,
          top: toolbarBelow ? (sel.y - 36) : (sel.y + sel.h + 4),
          width: 32, height: 32,
          display: "flex", alignItems: "center", justifyContent: "center",
          borderRadius: 6, border: "none",
          background: "rgba(255,255,255,0.9)",
          boxShadow: "0 2px 8px rgba(0,0,0,0.15)",
          cursor: "pointer", zIndex: 101,
        }}>
          <img src="icons/pin.svg" alt="贴图" className="w-[18px] h-[18px]" />
        </button>
      )}

      {/* 工具属性浮窗 */}
      {sel && mode === "selected" && tool !== "none" && (
        <ToolPropsPopover
          x={toolbarCenterX}
          y={popoverY}
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

      {/* 滚动预览浮层 */}
      {mode === "scrolling" && scrollPreview && sel && (
        <div style={{
          position: "fixed",
          left: sel.x + sel.w + 12 + 200 <= window.innerWidth
            ? sel.x + sel.w + 12
            : sel.x - 12 - 200,
          bottom: window.innerHeight - sel.y - sel.h,
          width: 200,
          maxHeight: "80vh",
          background: "rgba(15,15,17,0.92)",
          backdropFilter: "blur(16px)",
          WebkitBackdropFilter: "blur(16px)",
          borderRadius: 10,
          padding: 10,
          display: "flex",
          flexDirection: "column",
          gap: 8,
          zIndex: 102,
          overflow: "hidden",
          boxShadow: "0 8px 32px rgba(0,0,0,0.5), 0 0 0 1px rgba(255,255,255,0.06)",
        }}>
          {/* 状态条：脉冲录制点 + 等宽高度计数器 */}
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", padding: "0 2px" }}>
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <div style={{
                width: 7, height: 7, borderRadius: "50%", background: "#f59e0b",
                boxShadow: "0 0 6px #f59e0b",
                animation: "pulse 1.5s ease-in-out infinite",
              }} />
              <span style={{ fontSize: 10, color: "#f59e0b", fontWeight: 600, letterSpacing: 0.3 }}>REC</span>
            </div>
            <span style={{ fontSize: 11, color: "rgba(255,255,255,0.55)", fontFamily: "SF Mono, Menlo, monospace", fontVariantNumeric: "tabular-nums" }}>
              {scrollHeight}px
            </span>
          </div>
          {/* 预览图 */}
          <div style={{ flex: 1, overflow: "hidden", borderRadius: 6, display: "flex", flexDirection: "column", justifyContent: "flex-end", background: "rgba(0,0,0,0.3)" }}>
            <img src={`data:image/png;base64,${scrollPreview}`} alt="preview" style={{ width: "100%", display: "block" }} />
          </div>
          {/* 按钮区：保存 复制 取消 */}
          <div style={{ display: "flex", gap: 6, height: 32 }}>
            <button onClick={() => invoke("stop_scroll_recording_with_mode", { mode: "save" }).catch(() => {})} style={{
              flex: 1, borderRadius: 6, border: "none",
              background: "#3b82f6", color: "#fff",
              fontSize: 12, fontWeight: 600, cursor: "pointer",
              transition: "background 0.15s",
            }}
            onMouseEnter={(e) => e.currentTarget.style.background = "#2563eb"}
            onMouseLeave={(e) => e.currentTarget.style.background = "#3b82f6"}>
              保存
            </button>
            <button onClick={() => invoke("stop_scroll_recording_with_mode", { mode: "copy" }).catch(() => {})} style={{
              flex: 1, borderRadius: 6, border: "none",
              background: "#22c55e", color: "#fff",
              fontSize: 12, fontWeight: 600, cursor: "pointer",
              transition: "background 0.15s",
            }}
            onMouseEnter={(e) => e.currentTarget.style.background = "#16a34a"}
            onMouseLeave={(e) => e.currentTarget.style.background = "#22c55e"}>
              复制
            </button>
            <button onClick={() => invoke("stop_scroll_recording_with_mode", { mode: "cancel" }).catch(() => {})} style={{
              flex: 1, borderRadius: 6,
              border: "1px solid rgba(255,255,255,0.15)",
              background: "transparent", color: "rgba(255,255,255,0.5)",
              fontSize: 12, cursor: "pointer",
              transition: "all 0.15s",
            }}
            onMouseEnter={(e) => { e.currentTarget.style.borderColor = "rgba(255,255,255,0.3)"; e.currentTarget.style.color = "rgba(255,255,255,0.8)"; }}
            onMouseLeave={(e) => { e.currentTarget.style.borderColor = "rgba(255,255,255,0.15)"; e.currentTarget.style.color = "rgba(255,255,255,0.5)"; }}>
              取消
            </button>
          </div>
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
        padding: "10px 12px",
        background: "#fff",
        borderRadius: 10,
        boxShadow: "0 8px 24px -4px rgba(0,0,0,0.2), 0 2px 8px -2px rgba(0,0,0,0.1)",
        zIndex: 101,
        display: "flex",
        flexDirection: "column",
        gap: 10,
        width: 240,
      }}
    >
      {/* 第一行：粗细滑轨 + 当前色（最右） */}
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <span style={{ fontSize: 10, color: "#999", width: 20, fontWeight: 500, flexShrink: 0 }}>{label}</span>
        <input
          type="range"
          min={min}
          max={max}
          value={sizeValue}
          onChange={(e) => setSize(Number(e.target.value))}
          style={{ flex: 1, height: 4, borderRadius: 2, cursor: "pointer", accentColor: color }}
        />
        <span style={{ fontSize: 10, color: "#666", fontVariantNumeric: "tabular-nums", width: 18, textAlign: "center", fontWeight: 600 }}>{sizeValue}</span>
        {/* 当前色 — 带粗白边 + 阴影，和下方预设色区分 */}
        <div style={{
          width: 20, height: 20, borderRadius: "50%",
          background: color,
          border: "3px solid #fff",
          boxShadow: "0 0 0 1.5px rgba(0,0,0,0.2), 0 1px 3px rgba(0,0,0,0.15)",
          flexShrink: 0,
        }} />
      </div>

      {/* 分隔线 */}
      <div style={{ height: 1, background: "rgba(0,0,0,0.06)", margin: "0 -4px" }} />

      {/* 第二行：预设色 + 调色板 */}
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        {PRESET_COLORS.map((c) => (
          <button
            key={c}
            onClick={() => onColorChange(c)}
            style={{
              width: 18, height: 18, borderRadius: 5,
              background: c,
              border: c === "#ffffff" ? "1px solid #e0e0e0" : "none",
              cursor: "pointer",
              padding: 0,
              opacity: color.toLowerCase() === c.toLowerCase() ? 1 : 0.45,
              transform: color.toLowerCase() === c.toLowerCase() ? "scale(1.1)" : "scale(1)",
              transition: "opacity 0.15s, transform 0.15s",
            }}
          />
        ))}
        {/* 调色板 */}
        <label style={{ cursor: "pointer", display: "flex", alignItems: "center", flexShrink: 0, marginLeft: 2 }}>
          <div style={{
            width: 18, height: 18, borderRadius: 5,
            background: "conic-gradient(from 0deg, #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000)",
            border: "1px solid rgba(0,0,0,0.1)",
          }} />
          <input
            type="color"
            value={color}
            onChange={(e) => onColorChange(e.target.value)}
            style={{ width: 0, height: 0, opacity: 0, position: "absolute" }}
          />
        </label>
      </div>
    </div>
  );
}
