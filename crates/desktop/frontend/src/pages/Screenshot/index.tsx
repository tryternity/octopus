import { useState, useRef, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";
import { type Annotation, type Tool, drawAnnotation, drawAnnotationScaled, drawMosaic, annBounds, hitTestAnnotationPrecise } from "@/lib/annotation";
import { ToolButton } from "./ToolButton";
import { ToolPropsPopover } from "./ToolPropsPopover";
import { ScrollPreview } from "./ScrollPreview";

interface Selection {
  x: number; y: number; w: number; h: number;
}

type Mode = "idle" | "selecting" | "selected" | "move" | "resize" | "scrolling";

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
  const isPinningRef = useRef(false);

  const setModeSafe = (m: Mode) => { modeRef.current = m; setMode(m); };
  const [mode, setMode] = useState<Mode>("idle");
  const [sel, setSel] = useState<Selection | null>(null);
  const [resizeHandle, setResizeHandle] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const [tool, setTool] = useState<Tool>("none");
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const redoStackRef = useRef<Annotation[]>([]);
  const [redoAvailable, setRedoAvailable] = useState(false);
  const [showPopover, setShowPopover] = useState(false);
  const [popoverX, setPopoverX] = useState(0);
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
  const [toolFilled, setToolFilled] = useState(false);
  const toolFilledRef = useRef(false);
  const setToolColor = (c: string) => { toolColorRef.current = c; setToolColorState(c); };
  const setToolFontSize = (s: number) => { toolFontSizeRef.current = s; setToolFontSizeState(s); };
  const scrollSaveAfterStopRef = useRef(false);
  const [numberCounter, setNumberCounter] = useState(1);
  const [toolCircleSize, setToolCircleSize] = useState(24);
  // OCR 全局互斥：他处正在识别时本入口被拒 → 屏幕中央短暂提示 1.8s
  const [ocrWarn, setOcrWarn] = useState(false);
  const annMoveStartRef = useRef<{ idx: number; mx: number; my: number; anns: Annotation[] } | null>(null);
  // 窗口识别吸附（v1）：winOrigin=本窗全局逻辑原点；snapRef=当前悬停吸附候选（本窗 CSS）；lastHitRef=节流时间戳。
  const winOriginRef = useRef<{ x: number; y: number } | null>(null);
  const snapRef = useRef<{ x: number; y: number; w: number; h: number } | null>(null);
  const lastHitRef = useRef(0);
  const reqSeqRef = useRef(0);

  const dpr = window.devicePixelRatio || 1;

  const winLabel = (() => {
    try { return getCurrentWindow().label; } catch { return "screenshot_window"; }
  })();

  useEffect(() => {
    invoke<ArrayBuffer>("get_screenshot_image", { label: winLabel })
      .then((buf) => {
        const img = new Image();
        const blob = new Blob([buf], { type: "image/jpeg" });
        const url = URL.createObjectURL(blob);
        img.onload = () => {
          bgImgRef.current = img;
          setReady(true);
          URL.revokeObjectURL(url); // onload 后图片已解码到内存，释放 Object URL
          setTimeout(() => { invoke("show_screenshot_window").catch(() => {}); }, 50);
        };
        img.src = url;
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

  // onMount：缓存本窗全局逻辑原点（outerPosition 物理 / scaleFactor → 逻辑 points）。
  // 截图覆盖窗定位后 show，onMount 时位置已稳定。
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const win = getCurrentWindow();
        const factor = await win.scaleFactor();
        const pos = await win.outerPosition(); // PhysicalPosition
        if (!cancelled && factor > 0) {
          winOriginRef.current = { x: pos.x / factor, y: pos.y / factor };
        }
      } catch { /* winOrigin 取不到 → 吸附自动失效（snapRef 永远 null），回退纯手动 */ }
    })();
    return () => { cancelled = true; };
  }, []);

  // 滚动截图事件监听
  // listen 是异步的：若组件在 Promise resolve 前卸载（用户快速 ESC/右键取消截图），
  // cleanup 时 unlisten* 仍为 undefined → 注销失效、监听器永久遗留在 Tauri 事件总线。
  // cancelled 哨兵：resolve 时若已卸载则立即调 fn() 自注销（对齐 ImagePreview ocr 监听范式）。
  useEffect(() => {
    let unlistenFrame: (() => void) | undefined;
    let unlistenDone: (() => void) | undefined;
    let cancelled = false;
    listen<{ frame: string; preview: string; height: number; phys_height: number }>("scroll://frame", (e) => {
      const img = new Image();
      img.onload = () => { scrollFrameRef.current = img; draw(); };
      img.src = `data:image/jpeg;base64,${e.payload.frame}`;
      setScrollPreview(e.payload.preview);
      setScrollHeight(e.payload.phys_height);
    }).then((fn) => { if (cancelled) fn(); else unlistenFrame = fn; });
    listen("scroll://done", () => {
      setScrollPreview(null);
      setModeSafe("selected");
      // 保存模式由 Rust 端直接弹对话框，前端不再中转 base64
      scrollSaveAfterStopRef.current = false;
    }).then((fn) => { if (cancelled) fn(); else unlistenDone = fn; });
    return () => { cancelled = true; unlistenFrame?.(); unlistenDone?.(); };
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

  // undo/redo
  const undoAnnotation = () => {
    setAnnotations((prev) => {
      if (prev.length === 0) return prev;
      const removed = prev[prev.length - 1];
      redoStackRef.current.push(removed);
      setRedoAvailable(true);
      if (removed.type === "number" && removed.number === numberCounter - 1) {
        setNumberCounter(numberCounter - 1);
      }
      return prev.slice(0, -1);
    });
  };
  const redoAnnotation = () => {
    const ann = redoStackRef.current.pop();
    if (ann) {
      if (ann.type === "number") setNumberCounter(numberCounter + 1);
      setAnnotations((prev) => [...prev, ann]);
      setRedoAvailable(redoStackRef.current.length > 0);
    }
  };
  const addAnnotation = (ann: Annotation) => {
    redoStackRef.current = [];
    setRedoAvailable(false);
    setAnnotations((prev) => [...prev, ann]);
  };

  // 绘制
  // 工具按钮点击：切换工具 + 记录按钮中心 x（浮窗跟随按钮）
  const onToolSelect = (e: React.MouseEvent, t: Tool, extra?: () => void) => {
    const btn = e.currentTarget as HTMLElement;
    const rect = btn.getBoundingClientRect();
    setPopoverX(rect.left + rect.width / 2);
    if (tool === t) {
      if (showPopover) { setShowPopover(false); setTool("none"); }
      else { setShowPopover(true); }
    } else {
      setTool(t); setShowPopover(true);
      extra?.();
    }
  };

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
        const labelY = toolbarBelow ? (y - 24) : (y + h + 6);
        const labelVisibleY = Math.max(0, labelY);
        ctx.fillStyle = "rgba(255, 255, 255, 0.9)";
        ctx.fillRect(x, labelVisibleY, tw + 8, 18);
        ctx.fillStyle = "#1a1a1a";
        ctx.fillText(label, x + 4, labelVisibleY + 13);
      }
    } else {
      ctx.fillRect(0, 0, cssW, cssH);
    }

    // 窗口识别吸附高亮：idle 悬停时画候选窗口描边 + 5% 填充
    if (mode === "idle" && !sel && snapRef.current) {
      const s = snapRef.current;
      if (s.w > 0 && s.h > 0) {
        ctx.save();
        ctx.fillStyle = "rgba(59, 130, 246, 0.08)";   // 蓝 5% 填充
        ctx.fillRect(s.x, s.y, s.w, s.h);
        ctx.strokeStyle = "rgba(59, 130, 246, 0.9)";   // 蓝描边
        ctx.lineWidth = 2;
        ctx.setLineDash([6, 4]);
        ctx.strokeRect(s.x + 1, s.y + 1, s.w - 2, s.h - 2);
        ctx.restore();
      }
    }
  }, [sel, mode, ready, dpr, annotations, textDraft, tool, selectedAnn]);

  useEffect(() => { draw(); }, [draw]);

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
    if (e.button !== 0) return;
    if (mode === "scrolling") return;
    setShowPopover(false);  // 用户开始操作 → 收起浮窗
    const mx = e.clientX;
    const my = e.clientY;
    startPtRef.current = { x: mx, y: my };

    // 文字标注正在输入时，点击其他地方 = 确认当前文字
    if (textDraftRef.current) {
      const draft = textDraftRef.current;
      if (draft.val.trim()) {
        addAnnotation({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: toolColorRef.current, fontSize: toolFontSizeRef.current, textWidth: 200 });
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
      const annIdx = hitTestAnnotationPrecise(mx, my, annotations);
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
        addAnnotation({
          type: "number", x1: mx, y1: my, x2: mx, y2: my,
          number: numberCounter, color: toolColorRef.current, circleSize: toolCircleSize,
        });
        setNumberCounter(numberCounter + 1);
        return;
      }
      if (tool === "pen") {
        drawingRef.current = { type: "pen", x1: mx, y1: my, x2: mx, y2: my, points: [[mx, my]], color: toolColor, lineWidth: toolWidth };
      } else {
        drawingRef.current = { type: tool, x1: mx, y1: my, x2: mx, y2: my, color: toolColor, lineWidth: toolWidth, filled: (tool === "rect" || tool === "oval" || tool === "diamond") ? toolFilledRef.current : undefined };
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

    // 窗口识别吸附：仅 idle（无选区、无标注工具）时悬停查询；Cmd 临时禁用。
    // 注：selecting/selected 不在此清 snapRef——必须保留供 onMouseUp 单击判定；
    //     draw() 的 mode==="idle" 守卫保证这些模式下不画吸附高亮。
    if (mode === "idle" && !sel && tool === "none") {
      if (e.metaKey) {
        if (snapRef.current) { snapRef.current = null; draw(); }
      } else if (winOriginRef.current) {
        const now = performance.now();
        // 20ms ≈ 50Hz：高于此节流窗的 IPC 徒增开销，低于则高亮明显滞后
        if (now - lastHitRef.current >= 20) {
          lastHitRef.current = now;
          const o = winOriginRef.current;
          const gx = o.x + mx;
          const gy = o.y + my;
          const seq = ++reqSeqRef.current; // in-flight 去重：新请求覆盖旧的，旧响应到达即丢弃
          invoke<{ x: number; y: number; w: number; h: number } | null>("hit_test_window", { gx, gy })
            .then((snap) => {
              if (seq !== reqSeqRef.current) return; // 过期响应丢弃（鼠标已移到别处）
              if (!snap) {
                if (snapRef.current) { snapRef.current = null; draw(); }
                return;
              }
              snapRef.current = { x: snap.x - o.x, y: snap.y - o.y, w: snap.w, h: snap.h };
              draw();
            })
            .catch(() => { /* 查询失败 → 不高亮，回退手动 */ });
        }
      }
    }

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
      if (sel && inSelection(mx, my) && hitTestAnnotationPrecise(mx, my, annotations) !== null) {
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
      if (ann.type === "rect" || ann.type === "oval" || ann.type === "diamond") {
        if (Math.abs(ann.x2 - ann.x1) > 5 && Math.abs(ann.y2 - ann.y1) > 5) {
          addAnnotation(ann);
          added = true;
        }
      } else if (ann.type === "line" || ann.type === "arrow") {
        const dx = ann.x2 - ann.x1;
        const dy = ann.y2 - ann.y1;
        if (Math.sqrt(dx * dx + dy * dy) > 10) {
          addAnnotation(ann);
          added = true;
        }
      } else if (ann.type === "pen" && ann.points) {
        if (ann.points.length > 2) {
          addAnnotation(ann);
          added = true;
        }
      } else if (ann.type === "blur") {
        if (Math.abs(ann.x2 - ann.x1) > 5 && Math.abs(ann.y2 - ann.y1) > 5) {
          addAnnotation(ann);
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
      if (sel.w < MIN_SIZE || sel.h < MIN_SIZE) {
        // 单击：若有吸附候选 → 选中整窗；否则清空回 idle（现状）
        if (snapRef.current && snapRef.current.w >= MIN_SIZE && snapRef.current.h >= MIN_SIZE) {
          const snapped = { ...snapRef.current };
          snapRef.current = null; // 选中后清，防残留高亮
          setSel(snapped);
          setModeSafe("selected");
        } else {
          setSel(null);
          setModeSafe("idle");
        }
      } else {
        setModeSafe("selected");
      }
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
    } else if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "z") {
      e.preventDefault();
      redoAnnotation();
    } else if ((e.metaKey || e.ctrlKey) && e.key === "z") {
      undoAnnotation();
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
    const annIdx = hitTestAnnotationPrecise(mx, my, annotations);
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

  async function composeAndCropBytes(): Promise<ArrayBuffer | null> {
    if (!sel || !bgImgRef.current) return null;
    const bg = bgImgRef.current;
    const scale = bg.naturalWidth / window.innerWidth;

    // 合并已确认标注 + 未提交的文字输入（避免 onBlur 竞态丢失）
    const allAnns = [...annotations];
    const draft = textDraftRef.current;
    if (draft && draft.val.trim()) {
      allAnns.push({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: editTextColorRef.current || toolColorRef.current, fontSize: editTextFontSizeRef.current || toolFontSizeRef.current, textWidth: 200 });
    }

    const tmpCanvas = document.createElement("canvas");
    tmpCanvas.width = bg.naturalWidth;
    tmpCanvas.height = bg.naturalHeight;
    const tmpCtx = tmpCanvas.getContext("2d")!;
    tmpCtx.drawImage(bg, 0, 0);
    // 先处理 blur（像素马赛克降采样），再画其他标注
    for (const ann of allAnns) {
      if (ann.type === "blur") drawMosaic(tmpCtx, ann, scale);
    }
    for (const ann of allAnns) {
      if (ann.type === "blur") continue; // blur 已由 drawMosaic 处理，跳过避免色块叠加两次
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

    const blob: Blob = await new Promise((resolve, reject) => croppedCanvas.toBlob((b) => b ? resolve(b) : reject("toBlob failed"), "image/png"));
    return await blob.arrayBuffer();
  }

  function doOcr() {
    composeAndCropBytes().then((bytes) => {
      if (!bytes) return;
      invoke("ocr_screenshot", bytes as unknown as Record<string, unknown>).catch((e) => {
        const msg = String(e);
        if (msg.includes("还未完成")) {
          setOcrWarn(true);
          setTimeout(() => setOcrWarn(false), 1800);
        } else {
          console.error(e);
        }
      });
    });
  }

  function doSaveFile() {
    composeAndCropBytes().then((bytes) => {
      if (!bytes) return;
      invoke("save_screenshot_dialog", bytes as unknown as Record<string, unknown>).catch(() => {});
    });
  }

  async function arrayBufferToBase64(buffer: ArrayBuffer): Promise<string> {
    return new Promise((resolve, reject) => {
      const blob = new Blob([buffer], { type: "image/png" });
      const reader = new FileReader();
      reader.onloadend = () => {
        const dataUrl = reader.result as string;
        const base64 = dataUrl.substring(dataUrl.indexOf(",") + 1);
        resolve(base64);
      };
      reader.onerror = () => {
        reject(reader.error || new Error("FileReader failed"));
      };
      reader.readAsDataURL(blob);
    });
  }

  function doPin() {
    if (!sel || isPinningRef.current) return;
    isPinningRef.current = true;
    composeAndCropBytes().then(async (bytes) => {
      if (!bytes) {
        isPinningRef.current = false;
        return;
      }
      try {
        const base64Str = await arrayBufferToBase64(bytes);
        invoke("pin_screenshot", {
          label: winLabel,
          x: sel.x,
          y: sel.y,
          w: sel.w,
          h: sel.h,
          imgBase64: base64Str,
        }).catch((e) => {
          console.error("Pin screenshot failed:", e);
          isPinningRef.current = false;
        });
      } catch (err) {
        console.error("Failed to convert arraybuffer to base64:", err);
        isPinningRef.current = false;
      }
    }).catch(() => {
      isPinningRef.current = false;
    });
  }

  function doConfirm() {
    composeAndCropBytes().then((bytes) => {
      if (!bytes) return;
      invoke("confirm_screenshot_with_data", bytes as unknown as Record<string, unknown>).catch(() => {});
    });
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
  const popoverY = toolbarY + 44;  // 浮窗始终在工具栏下方

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
                setAnnotations(prev => prev.map((a, i) => i === editOrig.idx ? { ...a, text: newText, color: editColor, fontSize: editFontSize, textWidth: 200 } : a));
              } else {
                // 新建模式
                addAnnotation({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: newText, color: editColor, fontSize: editFontSize, textWidth: 200 });
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
            background: "var(--color-surface)",
            color: "var(--color-foreground)",
            borderRadius: 8,
            boxShadow: "0 4px 16px rgba(0,0,0,0.3)",
            zIndex: 100,
            alignItems: "center",
          }}
        >
          <ToolButton active={tool === "none"} onClick={() => setTool("none")} label="选择" icon={
            <img src="icons/arrow-pointer.svg" alt="选择" className="w-[18px] h-[18px]" style={{ filter: tool === "none" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <ToolButton active={tool === "rect"} onClick={(e) => onToolSelect(e, "rect")} label="矩形" icon={
            <img src="icons/square.svg" alt="矩形" className="w-[18px] h-[18px]" style={{ filter: tool === "rect" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <ToolButton active={tool === "oval"} onClick={(e) => onToolSelect(e, "oval")} label="椭圆" icon={
            <img src="icons/oval-vertical.svg" alt="椭圆" className="w-[18px] h-[18px]" style={{ filter: tool === "oval" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <ToolButton active={tool === "diamond"} onClick={(e) => onToolSelect(e, "diamond")} label="菱形" icon={
            <img src="icons/diamond.svg" alt="菱形" className="w-[18px] h-[18px]" style={{ filter: tool === "diamond" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <ToolButton active={tool === "line"} onClick={(e) => onToolSelect(e, "line")} label="直线" icon={
            <img src="icons/straight-line.svg" alt="直线" className="w-[18px] h-[18px]" style={{ filter: tool === "line" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <ToolButton active={tool === "arrow"} onClick={(e) => onToolSelect(e, "arrow")} label="箭头" icon={
            <img src="icons/arrow-line.svg" alt="箭头" className="w-[18px] h-[18px]" style={{ filter: tool === "arrow" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <ToolButton active={tool === "pen"} onClick={(e) => onToolSelect(e, "pen")} label="画笔" icon={
            <img src="icons/sketching.svg" alt="画笔" className="w-[18px] h-[18px]" style={{ filter: tool === "pen" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <ToolButton active={tool === "text"} onClick={(e) => onToolSelect(e, "text")} label="文字" icon={
            <img src="icons/text.svg" alt="文字" className="w-[18px] h-[18px]" style={{ filter: tool === "text" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <ToolButton active={tool === "number"} onClick={(e) => onToolSelect(e, "number", () => setNumberCounter(1))} label="序号" icon={
            <img src="icons/sequence-note.svg" alt="序号" className="w-[18px] h-[18px]" style={{ filter: tool === "number" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <ToolButton active={tool === "blur"} onClick={(e) => onToolSelect(e, "blur")} label="马赛克" icon={
            <img src="icons/mosaic.svg" alt="马赛克" className="w-[18px] h-[18px]" style={{ filter: tool === "blur" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <div style={{ width: 1, height: 20, background: "var(--color-border)", margin: "0 4px" }} />
          <ToolButton onClick={undoAnnotation} label="撤销" icon={
            <img src="icons/restore.svg" alt="撤销" className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)", opacity: annotations.length > 0 ? 1 : 0.3 }} />
          } />
          <ToolButton onClick={redoAnnotation} label="重做" icon={
            <img src="icons/redo.svg" alt="重做" className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)", opacity: redoAvailable ? 1 : 0.3 }} />
          } />
          <ToolButton onClick={doOcr} label="OCR" icon={
            <img src="icons/ocr-ai.svg" alt="OCR" className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
          } />
          <div style={{ width: 1, height: 20, background: "var(--color-border)", margin: "0 4px" }} />
          <button onClick={startScroll} title="滚动截图" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
            <img src="icons/scroll.svg" alt="滚动截图" className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
          </button>
          <button onClick={doSaveFile} title="保存到文件" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
            <img src="icons/save.svg" alt="保存" className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
          </button>
          <button onClick={doConfirm} title="确认" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "var(--color-voice)", cursor: "pointer" }}>
            <img src="icons/copy.svg" alt="确认" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) invert(1)" }} />
          </button>
          <button onClick={() => invoke("cancel_screenshot").catch(() => {})} title="取消" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
            <img src="icons/close.svg" alt="取消" className="w-[18px] h-[18px]" style={{ filter: "brightness(0) saturate(100%) invert(40%) sepia(94%) saturate(7470%) hue-rotate(346deg) brightness(95%) contrast(91%)" }} />
          </button>
        </div>
      )}

      {/* 贴图按钮 */}
      {sel && mode !== "scrolling" && (
        <button onClick={doPin} title="贴图" style={{
          position: "fixed",
          left: sel.x + sel.w - 28,
          top: toolbarBelow ? (sel.y - 28) : (sel.y + sel.h + 6),
          width: 24, height: 24,
          display: "flex", alignItems: "center", justifyContent: "center",
          borderRadius: 5, border: "none",
          background: "var(--color-surface)",
          boxShadow: "0 2px 8px rgba(0,0,0,0.15)",
          cursor: "pointer", zIndex: 101,
        }}>
          <img src="icons/pin.svg" alt="贴图" style={{ width: 14, height: 14, filter: "var(--icon-filter)" }} />
        </button>
      )}

      {/* 工具属性浮窗 */}
      {sel && mode === "selected" && tool !== "none" && showPopover && (
        <ToolPropsPopover
          x={popoverX}
          y={popoverY}
          key={`${toolbarCenterX}-${popoverY}-${tool}`}
          color={toolColor}
          width={toolWidth}
          fontSize={toolFontSize}
          circleSize={toolCircleSize}
          isText={tool === "text"}
          isNumber={tool === "number"}
          isShape={tool === "rect" || tool === "oval" || tool === "diamond"}
          filled={toolFilled}
          onColorChange={setToolColor}
          onWidthChange={setToolWidth}
          onFontSizeChange={setToolFontSize}
          onCircleSizeChange={setToolCircleSize}
          onFilledChange={(f) => { setToolFilled(f); toolFilledRef.current = f; }}
        />
      )}

      {/* 滚动预览浮层 */}
      {mode === "scrolling" && scrollPreview && sel && (
        <ScrollPreview sel={sel} scrollPreview={scrollPreview} scrollHeight={scrollHeight} />
      )}

      {/* OCR 全局互斥提示：他处正在 OCR → 屏幕中央短暂提示稍后重试 */}
      {ocrWarn && (
        <div style={{
          position: "fixed", left: "50%", top: "50%", transform: "translate(-50%, -50%)",
          zIndex: 200, padding: "12px 20px", borderRadius: 10,
          background: "rgba(28,25,23,0.92)", color: "#fff",
          fontSize: 14, fontWeight: 500, boxShadow: "0 8px 24px rgba(0,0,0,0.3)",
          pointerEvents: "none",
        }}>
          前一个 OCR 还未完成，请稍后
        </div>
      )}
    </>
  );
}


