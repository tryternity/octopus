import { useState, useRef, useEffect, useLayoutEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";
import { invoke as rawInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { type Annotation, drawAnnotation, drawAnnotationScaled, drawBlur, annBounds, hitTestAnnotationPrecise } from "@/lib/annotation";
import { ToolButton } from "./ToolButton";
import { ScrollPreview } from "./ScrollPreview";
import { useAnnotationState, AnnotationToolbar, computeToolbarPosition, computeToolbarCenterX, TOOLBAR_H } from "@/components/Annotation";
import { useT } from "@/lib/i18n";

interface Selection {
  x: number; y: number; w: number; h: number;
}

type Mode = "idle" | "selecting" | "selected" | "move" | "resize" | "scrolling";

const HANDLE_SIZE = 8;
const MIN_SIZE = 10;

export default function Screenshot() {
  const t = useT();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const bgImgRef = useRef<HTMLImageElement | null>(null);
  // ImageBitmap 缓存——已解码的 GPU 友好位图，drawImage 比直接画 HTMLImageElement 快
  // （省每帧重新解码）。标注拖动时 draw() 每帧全屏 drawImage(bg)，是主要开销。
  const bgBitmapRef = useRef<ImageBitmap | null>(null);
  const startPtRef = useRef({ x: 0, y: 0 });
  const moveStartRef = useRef({ x: 0, y: 0 });
  const selStartRef = useRef<Selection>({ x: 0, y: 0, w: 0, h: 0 });
  const textInputRef = useRef<HTMLTextAreaElement | null>(null);
  const isPinningRef = useRef(false);

  // ── 标注状态（hook 抽取，与 RecordAnnotation 共用）────────────
  const annotation = useAnnotationState();
  const drawingRef = annotation.drawingRef;
  const annMoveStartRef = useRef<{ idx: number; mx: number; my: number; anns: Annotation[] } | null>(null);

  const setModeSafe = (m: Mode) => { modeRef.current = m; setMode(m); };
  const [mode, setMode] = useState<Mode>("idle");
  const [sel, setSel] = useState<Selection | null>(null);
  const [resizeHandle, setResizeHandle] = useState<string | null>(null);
  const [ready, setReady] = useState(false);

  // 文字输入草稿（业务侧独有，含双击编辑模式）
  const [textDraft, setTextDraft] = useState<{ x: number; y: number; val: string } | null>(null);
  const textDraftRef = useRef<{ x: number; y: number; val: string } | null>(null);
  const modeRef = useRef<Mode>("idle");
  const editTextColorRef = useRef<string | null>(null);
  const editTextFontSizeRef = useRef<number | null>(null);
  const editTextOrigRef = useRef<{ idx: number; text: string; color: string; fontSize: number } | null>(null);

  const [scrollPreview, setScrollPreview] = useState<string | null>(null);
  const [scrollHeight, setScrollHeight] = useState(0);
  // scrolling 模式录制时长（前端 setInterval，startScroll 启动 / scroll://done 停）。
  // 显示在 ScrollPreview 顶部「REC ● mm:ss」。
  const [scrollElapsed, setScrollElapsed] = useState(0);
  const scrollElapsedRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const scrollFrameRef = useRef<HTMLImageElement | null>(null);

  const scrollSaveAfterStopRef = useRef(false);
  // OCR 全局互斥：他处正在识别时本入口被拒 → 屏幕中央短暂提示 1.8s
  const [ocrWarn, setOcrWarn] = useState(false);
  // 第十五轮 P3-组4 #6：ocrWarn setTimeout 用 ref 管理（原裸 setTimeout 无 ref），
  // 防 unmount 后 setState + 连续 warn 时 timer stacking（旧未到期被新覆盖前并行跑）。
  const ocrWarnTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 二维码识别：就地白卡展示结果（null=不显示，string[]=结果，识别中由 qrScanning 区分）
  const [qrScanning, setQrScanning] = useState(false);
  const [qrResult, setQrResult] = useState<string[] | null>(null);

  // 工具栏实际宽度（useLayoutEffect 测量，用于 X 方向 clamp 防止跑出屏幕）
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [toolbarW, setToolbarW] = useState(0);
  useLayoutEffect(() => {
    if (toolbarRef.current) {
      setToolbarW(toolbarRef.current.offsetWidth);
    }
  }, [sel, annotation.tool]);

  const dpr = window.devicePixelRatio || 1;

  const winLabel = (() => {
    try { return getCurrentWindow().label; } catch { return "screenshot_window"; }
  })();

  useEffect(() => {
    // 2026-07-20 perf：后端直接传 RGBA bytes（省 ~3s JPEG 编码 + base64 round-trip）。
    // ImageData 构造需要宽高，并行拉 size。
    Promise.all([
      invoke<ArrayBuffer>("get_screenshot_image", { label: winLabel }),
      invoke<[number, number]>("get_screenshot_image_size", { label: winLabel }),
    ])
      .then(([buf, [w, h]]) => {
        const rgba = new Uint8ClampedArray(buf);
        const imgData = new ImageData(rgba, w, h);
        // createImageBitmap(ImageData) 直接 GPU-friendly，省去 Image onload 异步等待。
        return createImageBitmap(imgData);
      })
      .then((bm) => {
        bgBitmapRef.current = bm;
        setReady(true);
        // show 窗口（不再 setTimeout 50ms——RGBA 路径同步可用了）。
        invoke("show_screenshot_window").catch(() => {});
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

  // 第十五轮 P3-组4 #6：ocrWarn timer unmount cleanup（与 ocrWarnTimerRef 配套）。
  useEffect(() => () => {
    if (ocrWarnTimerRef.current) clearTimeout(ocrWarnTimerRef.current);
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
      // 停止 scroll 计时器
      if (scrollElapsedRef.current) {
        clearInterval(scrollElapsedRef.current);
        scrollElapsedRef.current = null;
      }
      setModeSafe("selected");
      // 保存模式由 Rust 端直接弹对话框，前端不再中转 base64
      scrollSaveAfterStopRef.current = false;
    }).then((fn) => { if (cancelled) fn(); else unlistenDone = fn; });
    return () => {
      cancelled = true; unlistenFrame?.(); unlistenDone?.();
      bgBitmapRef.current?.close(); // 释放 ImageBitmap 缓存
    };
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

  // add/undo/redo 已抽到 useAnnotationState（与 RecordAnnotation 共用）
  const { addAnnotation, undoAnnotation, redoAnnotation } = annotation;

  // 工具按钮点击的 onToolSelect 已抽到 AnnotationToolbar 内部
  // （业务侧通过 onToolChange 回调做透传，截图无需特殊处理）

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    // ImageBitmap 优先（已解码，drawImage 快），未就绪时回退 HTMLImageElement
    const bg: CanvasImageSource | undefined = bgBitmapRef.current ?? bgImgRef.current ?? undefined;
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

      for (let i = 0; i < annotation.annotations.length; i++) {
        drawAnnotation(ctx, annotation.annotations[i]);
        if (annotation.selectedAnn === i) {
          const b = annBounds(annotation.annotations[i]);
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

        // 尺寸标注位置：避免与工具栏重叠。
        //   工具栏 below → 数字放上方（选区顶部上方）
        //   工具栏 above → 数字放下方（选区底部下方）
        //   工具栏 inside（选区内部底部）→ 数字放上方（选区顶部，远离工具栏）
        // draw 内独立计算 placement（避免依赖 render 后期定义的 tbPos）
        const _tbPos = computeToolbarPosition({ x, y, w, h }, cssH);
        const label = `${Math.round(w * dpr)} × ${Math.round(h * dpr)}`;
        ctx.font = "12px -apple-system, sans-serif";
        const tw = ctx.measureText(label).width;
        // above 时 label 在下方；below / inside 时 label 在上方
        const labelY = _tbPos.placement === "above"
          ? (y + h + 6)  // 工具栏 above → 数字在下方
          : (y - 24);    // 工具栏 below 或 inside → 数字在上方
        const labelVisibleY = Math.max(0, labelY);
        ctx.fillStyle = "rgba(255, 255, 255, 0.9)";
        ctx.fillRect(x, labelVisibleY, tw + 8, 18);
        ctx.fillStyle = "#1a1a1a";
        ctx.fillText(label, x + 4, labelVisibleY + 13);
      }
    } else {
      ctx.fillRect(0, 0, cssW, cssH);
    }
  }, [sel, mode, ready, dpr, annotation.annotations, annotation.selectedAnn, annotation.tool, textDraft]);

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
    annotation.setShowPopover(false);  // 用户开始操作 → 收起浮窗
    const mx = e.clientX;
    const my = e.clientY;
    startPtRef.current = { x: mx, y: my };

    // 文字标注正在输入时，点击其他地方 = 确认当前文字
    if (textDraftRef.current) {
      const draft = textDraftRef.current;
      if (draft.val.trim()) {
        addAnnotation({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: annotation.toolColorRef.current, fontSize: annotation.toolFontSizeRef.current, textWidth: 200 });
      }
      textDraftRef.current = null;
      setTextDraft(null);
      // 如果点击的还是选区内 + 文字工具，开新的文字输入
      if (annotation.tool === "text" && sel && inSelection(mx, my)) {
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
    if (annotation.tool === "none" && sel && inSelection(mx, my)) {
      const annIdx = hitTestAnnotationPrecise(mx, my, annotation.annotations);
      if (annIdx !== null) {
        annotation.setSelectedAnn(annIdx);
        annMoveStartRef.current = { idx: annIdx, mx, my, anns: [...annotation.annotations] };
        return;
      }
    }

    // 标注工具激活时，在选区内绘制新标注
    if (annotation.tool !== "none" && sel && inSelection(mx, my)) {
      // eraser：mousedown 即开始擦除（划过即删）
      if (annotation.tool === "eraser") {
        annotation.eraseAnnotationAt(mx, my);
        return;
      }
      if (annotation.tool === "text") {
        setTextDraft({ x: mx, y: my, val: "" });
        textDraftRef.current = { x: mx, y: my, val: "" };
        setTimeout(() => textInputRef.current?.focus(), 10);
        return;
      }
      if (annotation.tool === "number") {
        addAnnotation({
          type: "number", x1: mx, y1: my, x2: mx, y2: my,
          number: annotation.numberCounter, color: annotation.toolColorRef.current, circleSize: annotation.toolCircleSize,
        });
        annotation.setNumberCounter(annotation.numberCounter + 1);
        return;
      }
      if (annotation.tool === "pen" || annotation.tool === "highlight") {
        drawingRef.current = { type: annotation.tool, x1: mx, y1: my, x2: mx, y2: my, points: [[mx, my]], color: annotation.toolColor, lineWidth: annotation.tool === "highlight" ? 15 : annotation.toolWidth };
      } else {
        drawingRef.current = { type: annotation.tool, x1: mx, y1: my, x2: mx, y2: my, color: annotation.toolColor, lineWidth: annotation.toolWidth, filled: (annotation.tool === "rect" || annotation.tool === "oval" || annotation.tool === "diamond") ? annotation.toolFilledRef.current : undefined };
      }
      return;
    }

    // tool 为 none 时：选区内空白点击取消选中 + 允许平移选区
    if (annotation.tool === "none" && sel && inSelection(mx, my)) {
      annotation.setSelectedAnn(null);
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

    // eraser：按住左键拖动时擦除（划过即删）
    if (annotation.tool === "eraser" && (e.buttons & 1)) {
      annotation.eraseAnnotationAt(mx, my);
      return;
    }

    // 标注绘制中
    if (drawingRef.current && annotation.tool !== "none") {
      if ((drawingRef.current.type === "pen" || drawingRef.current.type === "highlight") && drawingRef.current.points) {
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
      annotation.setAnnotations(newAnns);
      return;
    }

    if (mode === "idle" || mode === "selected") {
      // 悬停在标注上显示 move 光标
      if (sel && inSelection(mx, my) && hitTestAnnotationPrecise(mx, my, annotation.annotations) !== null) {
        (e.currentTarget as HTMLCanvasElement).style.cursor = "move";
      } else {
        const handle = hitTest(mx, my);
        if (handle) {
          const cursors: Record<string, string> = {
            nw: "nwse-resize", se: "nwse-resize", ne: "nesw-resize", sw: "nesw-resize",
            n: "ns-resize", s: "ns-resize", e: "ew-resize", w: "ew-resize",
          };
          (e.currentTarget as HTMLCanvasElement).style.cursor = annotation.tool !== "none" ? "crosshair" : (cursors[handle] || "crosshair");
        } else if (sel && inSelection(mx, my)) {
          (e.currentTarget as HTMLCanvasElement).style.cursor = annotation.tool !== "none" ? "crosshair" : "move";
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
      } else if ((ann.type === "pen" || ann.type === "highlight") && ann.points) {
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
      if (annotation.tool !== "none") { annotation.setTool("none"); return; }
      invoke("cancel_screenshot").catch(() => {});
    } else if (e.key === "Enter" && sel && sel.w >= MIN_SIZE && sel.h >= MIN_SIZE) {
      doConfirm();
    } else if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "z") {
      e.preventDefault();
      redoAnnotation();
    } else if ((e.metaKey || e.ctrlKey) && e.key === "z") {
      undoAnnotation();
    } else if ((e.key === "Delete" || e.key === "Backspace") && annotation.selectedAnn !== null) {
      // 删除选中的标注
      const removed = annotation.annotations[annotation.selectedAnn];
      if (removed.type === "number" && removed.number === annotation.numberCounter - 1) {
        annotation.setNumberCounter(annotation.numberCounter - 1);
      }
      annotation.setAnnotations(annotation.annotations.filter((_, i) => i !== annotation.selectedAnn));
      annotation.setSelectedAnn(null);
    }
  }

  function onDoubleClick(e: React.MouseEvent) {
    if (e.button !== 0) return;
    const mx = e.clientX;
    const my = e.clientY;
    if (!sel || !inSelection(mx, my)) return;
    const annIdx = hitTestAnnotationPrecise(mx, my, annotation.annotations);
    if (annIdx === null) return;
    const ann = annotation.annotations[annIdx];
    if (ann.type !== "text" || !ann.text) return;
    // 记住原标注（ESC 可恢复），不立即删除
    const origColor = ann.color || "#ef4444";
    const origFontSize = ann.fontSize || 16;
    editTextOrigRef.current = { idx: annIdx, text: ann.text, color: origColor, fontSize: origFontSize };
    editTextColorRef.current = origColor;
    editTextFontSizeRef.current = origFontSize;
    // 隐藏原标注（标记为编辑中，Canvas 不绘制）
    annotation.setAnnotations(prev => prev.map((a, i) => i === annIdx ? { ...a, text: "" } : a));
    annotation.setSelectedAnn(null);
    setTextDraft({ x: ann.x1, y: ann.y1, val: ann.text });
    textDraftRef.current = { x: ann.x1, y: ann.y1, val: ann.text };
    setTimeout(() => textInputRef.current?.focus(), 10);
  }

  function onContextMenu(e: React.MouseEvent) {
    e.preventDefault();
    // 右键取消规则：
    //   - idle（未框选）：任意位置右键取消截图
    //   - selected：选区外右键取消截图（选区内右键无操作，避免误触）
    //   - scrolling：选区外右键停止 scroll（选区内/预览窗内不处理——预览窗有按钮）
    //     注：scrolling 时选区外鼠标穿透，onContextMenu 收不到——后端 poller 兜底
    //     （见 screenshot_commands.rs 鼠标轮询的右键检测）
    const mx = e.clientX;
    const my = e.clientY;
    if (mode === "idle") {
      invoke("cancel_screenshot").catch(() => {});
    } else if (mode === "selected" && sel) {
      // 选区外右键取消
      const inSel = mx >= sel.x && mx <= sel.x + sel.w && my >= sel.y && my <= sel.y + sel.h;
      if (!inSel) {
        invoke("cancel_screenshot").catch(() => {});
      }
    } else if (mode === "scrolling" && sel) {
      // scrolling 时选区外穿透，前端理论上收不到；保留分支作为兜底
      // （若鼠标恰好在交互区域边缘的非穿透瞬间右键）
      const inSel = mx >= sel.x && mx <= sel.x + sel.w && my >= sel.y && my <= sel.y + sel.h;
      if (!inSel) {
        stopScroll();
      }
    }
  }

  function startScroll() {
    if (!sel) return;
    setModeSafe("scrolling");
    annotation.setTool("none");
    setScrollPreview(null);
    setScrollHeight(0);
    setScrollElapsed(0);
    // 启动 scroll 录制计时器（显示在 ScrollPreview 顶部）
    if (scrollElapsedRef.current) clearInterval(scrollElapsedRef.current);
    scrollElapsedRef.current = setInterval(() => {
      setScrollElapsed((s) => s + 1);
    }, 1000);

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
    // 停止 scroll 计时器（ESC/右键/按钮停止时；scroll://done listener 也会清，幂等）
    if (scrollElapsedRef.current) {
      clearInterval(scrollElapsedRef.current);
      scrollElapsedRef.current = null;
    }
    invoke("stop_scroll_recording").catch(() => {});
  }

  async function composeAndCropBytes(): Promise<ArrayBuffer | null> {
    // 2026-07-20 perf：bg 现在是 ImageBitmap（直接 RGBA），优先用它；
    // bgImgRef 仅 legacy 兜底（理论上不再被设置）。
    const bg = bgBitmapRef.current ?? bgImgRef.current;
    if (!sel || !bg) return null;
    const bgW = "naturalWidth" in bg ? bg.naturalWidth : bg.width;
    const bgH = "naturalHeight" in bg ? bg.naturalHeight : bg.height;
    const scale = bgW / window.innerWidth;

    // 合并已确认标注 + 未提交的文字输入（避免 onBlur 竞态丢失）
    const allAnns = [...annotation.annotations];
    const draft = textDraftRef.current;
    if (draft && draft.val.trim()) {
      allAnns.push({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: editTextColorRef.current || annotation.toolColorRef.current, fontSize: editTextFontSizeRef.current || annotation.toolFontSizeRef.current, textWidth: 200 });
    }

    const tmpCanvas = document.createElement("canvas");
    tmpCanvas.width = bgW;
    tmpCanvas.height = bgH;
    const tmpCtx = tmpCanvas.getContext("2d")!;
    tmpCtx.drawImage(bg, 0, 0);
    // 先处理 blur（像素马赛克/高斯/黑条），再画其他标注
    for (const ann of allAnns) {
      if (ann.type === "blur") drawBlur(tmpCtx, ann, scale);
    }
    for (const ann of allAnns) {
      if (ann.type === "blur") continue; // blur 已由 drawBlur 处理，跳过避免色块叠加两次
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
          if (ocrWarnTimerRef.current) clearTimeout(ocrWarnTimerRef.current);
          ocrWarnTimerRef.current = setTimeout(() => setOcrWarn(false), 1800);
        } else {
          console.error(e);
        }
      });
    });
  }

  // 二维码识别：与 doOcr 同 composeAndCropBytes 范式，调 scan_qrcode_screenshot。
  // 后端已写剪贴板，前端只负责就地白卡展示结果。
  function doQrScan() {
    if (!sel) return;
    setQrScanning(true);
    setQrResult(null);
    composeAndCropBytes().then((bytes) => {
      if (!bytes) {
        setQrScanning(false);
        return;
      }
      return invoke<string[]>("scan_qrcode_screenshot", bytes as unknown as Record<string, unknown>);
    }).then((codes) => {
      setQrScanning(false);
      setQrResult(codes ?? []);
    }).catch((e) => {
      setQrScanning(false);
      setQrResult([]);
      console.error("QR scan failed:", e);
    });
  }

  function doSaveFile() {
    composeAndCropBytes().then((bytes) => {
      if (!bytes) return;
      invoke("save_screenshot_dialog", bytes as unknown as Record<string, unknown>).catch(() => {});
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
        // 2026-07-20 perf：自定义二进制协议（省 base64 round-trip ~50-200ms）
        // 协议：[u32 BE: label_len][label UTF-8][f64 BE: x][f64 BE: y][f64 BE: w][f64 BE: h][PNG bytes]
        // 整个 ArrayBuffer 作为 invoke args，Tauri 走 application/octet-stream Raw body。
        const labelBytes = new TextEncoder().encode(winLabel);
        const headerLen = 4 + labelBytes.length + 32;  // u32 + label + 4×f64
        const buf = new ArrayBuffer(headerLen + bytes.byteLength);
        const view = new DataView(buf);
        let off = 0;
        view.setUint32(off, labelBytes.length); off += 4;
        new Uint8Array(buf, off, labelBytes.length).set(labelBytes); off += labelBytes.length;
        view.setFloat64(off, sel.x); off += 8;
        view.setFloat64(off, sel.y); off += 8;
        view.setFloat64(off, sel.w); off += 8;
        view.setFloat64(off, sel.h); off += 8;
        new Uint8Array(buf, off).set(new Uint8Array(bytes));
        rawInvoke("pin_screenshot", buf).catch((e) => {
          console.error("Pin screenshot failed:", e);
          isPinningRef.current = false;
        });
      } catch (err) {
        console.error("Failed to encode pin payload:", err);
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

  // 工具栏位置（computeToolbarPosition 纯函数，已踩过坑稳定）：
  //   - below（默认）：选区下方 8px 处
  //   - above：选区上方（下方空间不够时）
  //   - inside：选区内部底部（上下都不够时兜底，例如全屏截图场景）
  //
  // toolbarBelow / toolbarAbove 由 placement 派生，被以下位置依赖：
  //   - draw() 的选区尺寸 label 位置（labelY 在上还是在下）
  //   - pin 按钮位置（toolbarBelow 时按钮在选区上方，否则在下方）
  const tbPos = sel ? computeToolbarPosition(sel, window.innerHeight) : null;
  const toolbarY = tbPos ? tbPos.y : 0;
  const toolbarBelow = tbPos?.placement === "below";
  const toolbarCenterX = sel ? computeToolbarCenterX(sel, window.innerWidth, toolbarW) : 0;
  // 浮窗默认在工具栏下方。若工具栏在"选区内部底部"或屏幕底部，浮窗往下会超出屏幕，
  // 此时改放工具栏上方（toolbarY - 浮窗高度）。浮窗实际高度由内容决定，这里用 200 估算
  // （ToolPropsPopover 含色板/滑块，实测 < 200px），由 popover 组件内部 clamp 兜底。
  const popoverY = tbPos?.belowOrAbove
    ? toolbarY + TOOLBAR_H
    : Math.max(0, toolbarY - 200);  // 工具栏在选区内部时浮窗往上弹

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
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          value={textDraft.val}
          onChange={(e) => {
            const updated = { ...textDraft, val: e.target.value };
            textDraftRef.current = updated;
            setTextDraft(updated);
          }}
          onBlur={() => {
            const draft = textDraftRef.current;
            const editColor = editTextColorRef.current || annotation.toolColorRef.current;
            const editFontSize = editTextFontSizeRef.current || annotation.toolFontSizeRef.current;
            const editOrig = editTextOrigRef.current;
            if (draft && draft.val.trim()) {
              const newText = draft.val;
              if (editOrig) {
                // 编辑模式：更新原标注
                annotation.setAnnotations(prev => prev.map((a, i) => i === editOrig.idx ? { ...a, text: newText, color: editColor, fontSize: editFontSize, textWidth: 200 } : a));
              } else {
                // 新建模式
                addAnnotation({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: newText, color: editColor, fontSize: editFontSize, textWidth: 200 });
              }
            } else if (editOrig) {
              // 内容为空：恢复原标注
              annotation.setAnnotations(prev => prev.map((a, i) => i === editOrig.idx ? { ...a, text: editOrig.text, color: editOrig.color, fontSize: editOrig.fontSize } : a));
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
                annotation.setAnnotations(prev => prev.map((a, i) => i === editOrig.idx ? { ...a, text: editOrig.text, color: editOrig.color, fontSize: editOrig.fontSize } : a));
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
            fontSize: editTextFontSizeRef.current || annotation.toolFontSize,
            color: editTextColorRef.current || annotation.toolColor,
            background: "transparent",
            border: `1px dashed ${annotation.toolColor}`,
            outline: "none",
            resize: "none",
            padding: "2px 4px",
            minHeight: annotation.toolFontSize + 8,
            width: 200,
          }}
        />
      )}

      {/* 工具栏（AnnotationToolbar 渲染 9 工具 + undo/redo + children slot，scrolling 模式隐藏） */}
      {sel && mode !== "scrolling" && (
        <AnnotationToolbar
          state={annotation}
          toolbarRef={toolbarRef}
          top={toolbarY}
          left={toolbarCenterX}
          // popover 仅在 selected 模式显示（selecting/move/resize 时收起，避免遮挡手柄操作）
          popoverY={mode === "selected" ? popoverY : undefined}
          // popover X：跟随按钮中心（state.popoverX），未点按钮时 fallback 到选区中心
          popoverX={annotation.popoverX || (sel.x + sel.w / 2)}
        >
          {/* divider + OCR + QR（截图独有） */}
          <div style={{ width: 1, height: 20, background: "var(--color-border)", margin: "0 4px" }} />
          <ToolButton onClick={doOcr} label="OCR" icon={
            <img src="icons/ocr-ai.svg" alt="OCR" className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
          } />
          <ToolButton onClick={doQrScan} active={qrScanning || qrResult !== null} label={t("screenshot.tool.qrcode")} icon={
            <img src="icons/qr-code.svg" alt={t("screenshot.tool.qrcode")} className="w-[18px] h-[18px]" style={{ filter: qrScanning || qrResult !== null ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
          } />
          <div style={{ width: 1, height: 20, background: "var(--color-border)", margin: "0 4px" }} />
          <button onClick={startScroll} title={t("screenshot.scrollShot")} style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
            <img src="icons/scroll.svg" alt={t("screenshot.scrollShot")} className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
          </button>
          <button onClick={doSaveFile} title={t("screenshot.saveToFile")} style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
            <img src="icons/save.svg" alt={t("screenshot.save")} className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)" }} />
          </button>
          <button onClick={doConfirm} title={t("screenshot.confirm")} style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "var(--color-voice)", cursor: "pointer" }}>
            <img src="icons/copy.svg" alt={t("screenshot.confirm")} className="w-[18px] h-[18px]" style={{ filter: "brightness(0) invert(1)" }} />
          </button>
          <button onClick={() => invoke("cancel_screenshot").catch(() => {})} title={t("screenshot.cancel")} style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
            <img src="icons/close.svg" alt={t("screenshot.cancel")} className="w-[18px] h-[18px]" style={{ filter: "brightness(0) saturate(100%) invert(40%) sepia(94%) saturate(7470%) hue-rotate(346deg) brightness(95%) contrast(91%)" }} />
          </button>
        </AnnotationToolbar>
      )}

      {/* 贴图按钮 */}
      {sel && mode !== "scrolling" && (
        <button onClick={doPin} title={t("screenshot.pin")} style={{
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
          <img src="icons/pin.svg" alt={t("screenshot.pin")} style={{ width: 14, height: 14, filter: "var(--icon-filter)" }} />
        </button>
      )}

      {/* 滚动预览浮层 */}
      {mode === "scrolling" && scrollPreview && sel && (
        <ScrollPreview sel={sel} scrollPreview={scrollPreview} scrollHeight={scrollHeight} elapsed={scrollElapsed} />
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
          {t("screenshot.ocrBusy")}
        </div>
      )}

      {/* 二维码识别就地白卡：覆盖在选区上方（紧贴选区顶边），含关闭按钮 */}
      {(qrScanning || qrResult !== null) && sel && (
        <QrResultCard
          sel={sel}
          scanning={qrScanning}
          codes={qrResult}
          onClose={() => { setQrResult(null); setQrScanning(false); }}
          scanningText={t("screenshot.qrScanning")}
          noResultText={t("screenshot.qrNoResult")}
          copyAllText={t("screenshot.qrCopyAll")}
          onOpenUrl={(u) => openUrl(u).catch(() => {})}
        />
      )}
    </>
  );
}

/**
 * 二维码结果白卡：就地覆盖在选区顶部（紧贴选区上沿，向下占位；不超出选区宽度
 * 的延伸——宽度自适应内容，最大不超过选区宽 + 一点边距）。zIndex 高于工具栏。
 *
 * 定位策略：
 *   - 水平：居中于选区（left = sel.x + sel.w/2，translateX(-50%)）
 *   - 垂直：贴选区顶边内侧偏下（top = sel.y + 6），保证卡片在选区内可见；
 *     若选区高度过小（< 60），改为贴选区上方（top = sel.y - 卡片高 - 6）
 *   - 多个二维码内容：逐行显示；http(s):// 开头渲染为可点击链接（openUrl 打开）
 */
function QrResultCard({ sel, scanning, codes, onClose, scanningText, noResultText, copyAllText, onOpenUrl }: {
  sel: Selection;
  scanning: boolean;
  codes: string[] | null;
  onClose: () => void;
  scanningText: string;
  noResultText: string;
  copyAllText: string;
  onOpenUrl: (url: string) => void;
}) {
  const CARD_MAX_W = 360;
  const CARD_MIN_W = 200;
  const cardW = Math.max(CARD_MIN_W, Math.min(CARD_MAX_W, sel.w));
  const above = sel.h < 80;

  const copyText = (text: string) => {
    navigator.clipboard.writeText(text).catch(() => {});
  };

  return (
    <div style={{
      position: "fixed",
      left: sel.x + sel.w / 2,
      top: above ? Math.max(6, sel.y - 8) : sel.y + 6,
      transform: above ? "translate(-50%, -100%)" : "translate(-50%, 0)",
      width: cardW,
      maxWidth: "90vw",
      padding: "10px 12px",
      background: "#ffffff",
      color: "#1a1a1a",
      borderRadius: 10,
      boxShadow: "0 8px 24px rgba(0,0,0,0.25)",
      zIndex: 210,
      fontSize: 13,
      fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
    }}>
      {/* 关闭按钮 */}
      <button
        onClick={onClose}
        title="✕"
        style={{
          position: "absolute", top: 4, right: 4,
          width: 22, height: 22, borderRadius: 5, border: "none", cursor: "pointer",
          background: "transparent", color: "#71717a", fontSize: 14, lineHeight: 1,
          display: "flex", alignItems: "center", justifyContent: "center",
        }}
        onMouseEnter={(e) => { e.currentTarget.style.background = "#f4f4f5"; }}
        onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; }}
      >✕</button>

      {scanning ? (
        <div style={{ padding: "6px 0", color: "#52525b", textAlign: "center" }}>{scanningText}</div>
      ) : codes && codes.length > 0 ? (
        <div>
          <div style={{ display: "flex", flexDirection: "column", gap: 6, paddingRight: 20 }}>
            {codes.map((c, i) => {
              const isUrl = /^https?:\/\//i.test(c);
              return (
                <div key={i} style={{ display: "flex", alignItems: "flex-start", gap: 4 }}>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    {isUrl ? (
                      <a
                        href={c}
                        onClick={(e) => { e.preventDefault(); onOpenUrl(c); }}
                        style={{ color: "#2563eb", textDecoration: "underline", wordBreak: "break-all", cursor: "pointer", fontSize: 13, lineHeight: 1.4 }}
                        title={c}
                      >{c}</a>
                    ) : (
                      <div style={{ wordBreak: "break-all", whiteSpace: "pre-wrap", fontSize: 13, lineHeight: 1.4, color: "#1a1a1a" }}>{c}</div>
                    )}
                  </div>
                  {/* 单个复制按钮 */}
                  <button
                    onClick={() => copyText(c)}
                    title="复制"
                    style={{
                      flexShrink: 0, width: 24, height: 24, borderRadius: 4, border: "none",
                      cursor: "pointer", background: "transparent", color: "#71717a",
                      display: "flex", alignItems: "center", justifyContent: "center",
                      fontSize: 12, marginTop: -1,
                    }}
                    onMouseEnter={(e) => { e.currentTarget.style.background = "#f4f4f5"; e.currentTarget.style.color = "#3b82f6"; }}
                    onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; e.currentTarget.style.color = "#71717a"; }}
                  ><img src="icons/copy.svg" alt="复制" className="w-[14px] h-[14px]" style={{ filter: "var(--icon-filter)" }} /></button>
                </div>
              );
            })}
          </div>
          {/* 复制所有——仅多码时显示 */}
          {codes.length > 1 && (
            <div style={{ marginTop: 8, paddingTop: 6, borderTop: "1px solid #f0f0f0" }}>
              <button
                onClick={() => copyText(codes.join("\n"))}
                style={{
                  width: "100%", padding: "5px 0", borderRadius: 5, border: "1px solid #e4e4e7",
                  cursor: "pointer", background: "#fafafa", color: "#52525b",
                  fontSize: 12, fontWeight: 500,
                }}
                onMouseEnter={(e) => { e.currentTarget.style.background = "#f4f4f5"; e.currentTarget.style.color = "#3b82f6"; }}
                onMouseLeave={(e) => { e.currentTarget.style.background = "#fafafa"; e.currentTarget.style.color = "#52525b"; }}
              >{copyAllText}</button>
            </div>
          )}
        </div>
      ) : (
        <div style={{ padding: "6px 0", color: "#71717a", textAlign: "center" }}>{noResultText}</div>
      )}
    </div>
  );
}


