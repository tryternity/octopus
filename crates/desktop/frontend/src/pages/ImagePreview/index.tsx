import { useState, useRef, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";
import {
  type Annotation,
  type Tool,
  drawAnnotation,
  hitTestAnnotationPrecise,
} from "@/lib/annotation";
import Toolbar from "./Toolbar";

const MIN_ZOOM = 0.1;
const MAX_ZOOM = 8;
const ZOOM_STEP = 1.25;

// fit-to-window：图片完整显示在窗口内，最大不超过 1:1
const FIT_PADDING = 16; // px-2 左右各 8px（画布间隙最小化，图片最大化展示）
// fit-to-window：完整显示在窗口内（宽高都不超出），不放大
const computeFitZoom = (w: number, h: number): number => {
  const containerW = window.innerWidth - FIT_PADDING;
  const containerH = window.innerHeight - FIT_PADDING;
  return Math.min(1, containerW / w, containerH / h);
};
// fit-to-width：图片宽度 = 窗口宽度（高度可超出 → 垂直滚动）
const computeFitToWidthZoom = (w: number): number => {
  const containerW = window.innerWidth - FIT_PADDING;
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, containerW / w));
};

/**
 * 剪贴板图片项的预览窗口（轻工具栏形态）。
 *
 * 显示：默认 fit-to-window 打开（缩略图秒开 → 全图异步替换）；图片超出窗口则滚动容器自动出滚动条（上下+左右），
 * 工具栏放大/缩小按钮调 zoom。标注用「自然像素」坐标（与 zoom 解耦）——绘制时
 * ctx.scale(zoom)，鼠标 /zoom 反算；合成保存/复制在自然尺寸画布 1:1 重绘（与 zoom 无关）。
 */
export default function ImagePreview() {
  const bgCanvasRef = useRef<HTMLCanvasElement>(null);
  const drawCanvasRef = useRef<HTMLCanvasElement>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);

  const [imageId, setImageId] = useState<number | null>(null);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [natW, setNatW] = useState(0);
  const [natH, setNatH] = useState(0);
  // zoom 倍率，1.0 = 1:1 自然分辨率（默认）
  const [zoom, setZoom] = useState(1);
  // 抓手平移中（tool==="none" 未命中标注时按住拖拽平移视口，免拖滚动条）
  const [panning, setPanning] = useState(false);

  const [tool, setTool] = useState<Tool>("none");
  const [toolColor, setToolColor] = useState("#ef4444");
  const [toolWidth, setToolWidth] = useState(3);
  const [toolFontSize, setToolFontSize] = useState(20);
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);
  const [ocrCopied, setOcrCopied] = useState(false);
  // 全图加载中：true 时禁止标注（避免 thumb 坐标系与 full 坐标系不一致）
  const loadingFullRef = useRef(false);
  // 当前 objectURL（图片切换/卸载时 revoke，防内存泄漏）
  const objectUrlRef = useRef<string | null>(null);
  // 全图自然尺寸（thumb 期间为 0，full 加载后赋值；EXIF 条用此而非 natW/natH）
  const [fullNatW, setFullNatW] = useState(0);
  const [fullNatH, setFullNatH] = useState(0);

  // 交互 refs（避免重渲染抖动 + 拖拽用最新值）
  const drawingRef = useRef<Annotation | null>(null);
  const dragRef = useRef<{ idx: number; dx: number; dy: number } | null>(null);
  const toolColorRef = useRef("#ef4444");
  const toolWidthRef = useRef(3);
  const toolFontSizeRef = useRef(20);
  const zoomRef = useRef(1);
  const scaledBitmapRef = useRef<ImageBitmap | null>(null);
  const zoomVersionRef = useRef(0);
  const userZoomedRef = useRef(false);
  // fit 模式：'fitWindow' | 'fitWidth' | 'manual'。ResizeObserver 据此决定是否自动重算
  const fitModeRef = useRef<'fitWindow' | 'fitWidth' | 'manual'>('fitWindow');
  // 文字输入框 ref：autoFocus 对动态挂载的 textarea 不可靠，改 setTimeout focus（对齐截图）
  const textInputRef = useRef<HTMLTextAreaElement | null>(null);
  // 文字草稿：state 驱动 textarea 渲染，ref 镜像供 commitText 读最新输入
  const textDraftRef = useRef<{ nx: number; ny: number; val: string } | null>(null);
  const [textDraft, setTextDraft] = useState<{ nx: number; ny: number; val: string } | null>(null);

  const setToolColorSync = (c: string) => { toolColorRef.current = c; setToolColor(c); };
  const setToolWidthSync = (n: number) => { toolWidthRef.current = n; setToolWidth(n); };
  const setToolFontSizeSync = (n: number) => { toolFontSizeRef.current = n; setToolFontSize(n); };
  const setZoomSync = (z: number, userInitiated = false) => {
    const clamped = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, z));
    zoomRef.current = clamped;
    if (userInitiated) {
      userZoomedRef.current = true;
      fitModeRef.current = 'manual';
    }
    setZoom(clamped);
  };
  const zoomIn = () => setZoomSync(zoomRef.current * ZOOM_STEP, true);
  const zoomOut = () => setZoomSync(zoomRef.current / ZOOM_STEP, true);
  const zoomReset = () => setZoomSync(1, true);
  // 自适应宽度：图片宽度 = 窗口宽度
  const zoomFitWidth = () => {
    if (!natW) return;
    fitModeRef.current = 'fitWidth';
    const z = computeFitToWidthZoom(natW);
    zoomRef.current = z;
    setZoom(z);
  };
  // 自适应窗口：图片完整显示在窗口内（宽高均不超出）
  const zoomFitWindow = () => {
    if (!natW || !natH) return;
    fitModeRef.current = 'fitWindow';
    const z = computeFitZoom(natW, natH);
    zoomRef.current = z;
    setZoom(z);
  };

  // —— ResizeObserver：fit 模式下窗口 resize 自动重算缩放 ——
  useEffect(() => {
    const onResize = () => {
      const mode = fitModeRef.current;
      if (mode === 'manual' || !natW || !natH) return;
      if (mode === 'fitWindow') {
        const z = computeFitZoom(natW, natH);
        zoomRef.current = z;
        setZoom(z);
      } else if (mode === 'fitWidth') {
        const z = computeFitToWidthZoom(natW);
        zoomRef.current = z;
        setZoom(z);
      }
    };
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, [natW, natH]);

  // —— unmount：revoke objectURL + close bitmap，防内存泄漏 ——
  useEffect(() => {
    return () => {
      if (objectUrlRef.current) URL.revokeObjectURL(objectUrlRef.current);
      if (scaledBitmapRef.current) scaledBitmapRef.current.close();
    };
  }, []);

  // —— mount：取 PENDING + 监听并发再开的 load 事件 ——
  useEffect(() => {
    invoke<{ imageId: number } | null>("get_pending_image").then((p) => {
      if (p) setImageId(p.imageId);
    });
    const unlisten = listen<{ imageId: number }>("image-preview://load", (e) => {
      setImageId(e.payload.imageId);
      // setAnnotations 和 setZoomSync 已在 imageId useEffect 中处理
    });
    return () => { unlisten.then((f) => f()); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // —— imageId 变 → 并行拉缩略图（秒开）+ 全图（异步替换） ——
  useEffect(() => {
    if (imageId == null) return;
    let cancelled = false;
    // 清理旧资源
    const old = scaledBitmapRef.current;
    scaledBitmapRef.current = null;
    if (old) old.close();
    if (objectUrlRef.current) {
      URL.revokeObjectURL(objectUrlRef.current);
      objectUrlRef.current = null;
    }
    zoomVersionRef.current++;
    userZoomedRef.current = false;
    fitModeRef.current = 'fitWindow';
    drawingRef.current = null;
    setAnnotations([]);
    setNatW(0);
    setNatH(0);
    setFullNatW(0);
    setFullNatH(0);
    loadingFullRef.current = true;

    const thumbPromise = invoke<string>("get_image_thumb", { id: imageId });
    const fullPromise = invoke<ArrayBuffer>("get_image_full", { id: imageId });

    // 缩略图先到 → 立即显示
    thumbPromise.then((thumbDataUrl) => {
      if (cancelled) return;
      const thumbImg = new Image();
      thumbImg.crossOrigin = "anonymous";
      thumbImg.onload = () => {
        if (cancelled) return;
        imgRef.current = thumbImg;
        setDataUrl(thumbDataUrl);
        const fitZoom = computeFitZoom(thumbImg.naturalWidth, thumbImg.naturalHeight);
        setNatW(thumbImg.naturalWidth);
        setNatH(thumbImg.naturalHeight);
        setZoomSync(fitZoom);
      };
      thumbImg.src = thumbDataUrl;
    }).catch((e) => console.error("thumb failed:", e));

    // 全图后到 → 无缝替换
    fullPromise.then((buf) => {
      if (cancelled) return;
      const blob = new Blob([buf], { type: "image/webp" });
      const url = URL.createObjectURL(blob);
      const fullImg = new Image();
      fullImg.crossOrigin = "anonymous";
      fullImg.onload = () => {
        if (cancelled) {
          URL.revokeObjectURL(url);
          return;
        }
        // revoke 上一张的 objectURL（thumb data URL 不需 revoke）
        if (objectUrlRef.current) URL.revokeObjectURL(objectUrlRef.current);
        objectUrlRef.current = url;
        loadingFullRef.current = false;
        imgRef.current = fullImg;
        setDataUrl(url);
        setFullNatW(fullImg.naturalWidth);
        setFullNatH(fullImg.naturalHeight);
        if (!userZoomedRef.current) {
          const fitZoom = computeFitZoom(fullImg.naturalWidth, fullImg.naturalHeight);
          setNatW(fullImg.naturalWidth);
          setNatH(fullImg.naturalHeight);
          setZoomSync(fitZoom);
        } else {
          setNatW(fullImg.naturalWidth);
          setNatH(fullImg.naturalHeight);
        }
        // 强制重新生成位图（全图替换缩略图后 zoom 可能不变，不触发 zoom effect）
        const oldBitmap = scaledBitmapRef.current;
        scaledBitmapRef.current = null;
        if (oldBitmap) oldBitmap.close();
        zoomVersionRef.current++;
        drawBg();  // 显式重绘（覆盖 thumb/full 同尺寸时 zoom effect 不触发的边界）
      };
      fullImg.src = url;
    }).catch((e) => console.error("full failed:", e));

    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [imageId]);

  // —— drawBg：底层重绘（底图 + 已确认标注），imageId/zoom/annotations 变化时调用 ——
  const drawBg = useCallback(() => {
    const canvas = bgCanvasRef.current;
    const img = imgRef.current;
    if (!canvas || !img || !natW || !natH) return;
    const dw = natW * zoom;
    const dh = natH * zoom;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(dw * dpr);
    canvas.height = Math.round(dh * dpr);
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, dw, dh);
    // 优先用预缩放位图（Task 2 异步生成），fallback 原图
    const bitmap = scaledBitmapRef.current;
    ctx.drawImage(bitmap || img, 0, 0, dw, dh);
    // 标注：自然坐标 → ×zoom 缩放到显示空间
    ctx.save();
    ctx.scale(zoom, zoom);
    for (const ann of annotations) drawAnnotation(ctx, ann);
    ctx.restore();
  }, [natW, natH, zoom, annotations]);

  // —— drawActive：顶层重绘（仅正在绘制的笔迹/形状预览），mousemove 调用 ——
  const drawActive = useCallback(() => {
    const canvas = drawCanvasRef.current;
    if (!canvas || !natW || !natH) return;
    const dw = natW * zoom;
    const dh = natH * zoom;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(dw * dpr);   // 赋值即清空
    canvas.height = Math.round(dh * dpr);
    if (!drawingRef.current) return;
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.save();
    ctx.scale(zoom, zoom);
    drawAnnotation(ctx, drawingRef.current);
    ctx.restore();
  }, [natW, natH, zoom]);

  // bgCanvas 同步触发：imageId/zoom/annotations 任一变化 → 完整重绘底层
  useEffect(() => { drawBg(); }, [drawBg]);

  // zoom 变化 → 异步生成预缩放位图 → drawBg（不阻塞主线程）
  useEffect(() => {
    const img = imgRef.current;
    if (!img || !natW || !natH) return;
    const version = ++zoomVersionRef.current;
    const dw = natW * zoom;
    const dh = natH * zoom;
    const dpr = window.devicePixelRatio || 1;
    const pw = Math.round(dw * dpr);
    const ph = Math.round(dh * dpr);
    // 极小尺寸（zoom 接近 0）跳过
    if (pw < 1 || ph < 1) return;

    createImageBitmap(img, {
      resizeWidth: pw,
      resizeHeight: ph,
      resizeQuality: "high",
    }).then((bitmap) => {
      // 版本不匹配 → 用户已切换到另一个 zoom，丢弃
      if (version !== zoomVersionRef.current) {
        bitmap.close();
        return;
      }
      const old = scaledBitmapRef.current;
      scaledBitmapRef.current = bitmap;
      if (old) old.close();
      drawBg();
    }).catch(() => {});
  }, [zoom, natW, natH]); // eslint-disable-line react-hooks/exhaustive-deps

  // CSS 坐标（相对 canvas，已含滚动偏移）→ 自然坐标（/zoom）
  const toNatural = (cssX: number, cssY: number) => {
    return { nx: cssX / zoomRef.current, ny: cssY / zoomRef.current };
  };

  const canvasCoords = (e: React.MouseEvent) => {
    const rect = drawCanvasRef.current!.getBoundingClientRect();
    return { cssX: e.clientX - rect.left, cssY: e.clientY - rect.top };
  };

  const commitText = () => {
    const d = textDraftRef.current;
    if (d && d.val.trim()) {
      setAnnotations((prev) => [...prev, {
        type: "text", x1: d.nx, y1: d.ny, x2: d.nx, y2: d.ny,
        text: d.val, color: toolColorRef.current, fontSize: toolFontSizeRef.current,
      }]);
    }
    textDraftRef.current = null;
    setTextDraft(null);
  };

  // 抓手平移：tool==="none" 未命中标注时，按住拖动平移滚动视口（免拖滚动条）。
  // 用 window 监听 mousemove/up，鼠标移出 canvas 仍跟随。
  const startPan = (e: React.MouseEvent) => {
    const sc = scrollContainerRef.current;
    if (!sc) return;
    const startX = e.clientX;
    const startY = e.clientY;
    const startLeft = sc.scrollLeft;
    const startTop = sc.scrollTop;
    setPanning(true);
    const onMove = (ev: MouseEvent) => {
      sc.scrollLeft = startLeft - (ev.clientX - startX);
      sc.scrollTop = startTop - (ev.clientY - startY);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      setPanning(false);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const { cssX, cssY } = canvasCoords(e);
    const { nx, ny } = toNatural(cssX, cssY);

    // 文字草稿进行中：点击别处 = 提交当前文字
    if (textDraftRef.current) {
      commitText();
    }

    // 全图加载中：仅允许选择/平移，禁止标注（thumb 坐标系 ≠ full 坐标系）
    if (loadingFullRef.current && tool !== "none") return;

    if (tool === "none") {
      const idx = hitTestAnnotationPrecise(nx, ny, annotations);
      if (idx != null) {
        dragRef.current = { idx, dx: nx - annotations[idx].x1, dy: ny - annotations[idx].y1 };
      } else {
        // 未命中标注 → 抓手拖拽平移视口
        startPan(e);
      }
      return;
    }

    if (tool === "text") {
      const d = { nx, ny, val: "" };
      textDraftRef.current = d;
      setTextDraft(d);
      // autoFocus 不可靠：等 textarea 挂载后手动聚焦
      setTimeout(() => textInputRef.current?.focus(), 10);
      return;
    }

    // 画笔（自由曲线）：起 points 点序列
    if (tool === "pen") {
      drawingRef.current = {
        type: "pen", x1: nx, y1: ny, x2: nx, y2: ny,
        points: [[nx, ny]],
        color: toolColorRef.current, lineWidth: toolWidthRef.current,
      };
      return;
    }
    // rect/oval/line/arrow 开始绘制（自然坐标）
    drawingRef.current = {
      type: tool as Annotation["type"],
      x1: nx, y1: ny, x2: nx, y2: ny,
      color: toolColorRef.current, lineWidth: toolWidthRef.current,
    };
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const { cssX, cssY } = canvasCoords(e);
    const { nx, ny } = toNatural(cssX, cssY);
    if (dragRef.current) {
      const { idx, dx, dy } = dragRef.current;
      setAnnotations((prev) => prev.map((a, i) => {
        if (i !== idx) return a;
        const mx = nx - dx, my = ny - dy;
        const w = a.x2 - a.x1, h = a.y2 - a.y1;
        return { ...a, x1: mx, y1: my, x2: mx + w, y2: my + h };
      }));
      return;
    }
    if (drawingRef.current) {
      // 画笔：push 新点到 points（自然坐标）
      if (drawingRef.current.type === "pen" && drawingRef.current.points) {
        drawingRef.current.points.push([nx, ny]);
      } else {
        drawingRef.current = { ...drawingRef.current, x2: nx, y2: ny };
      }
      drawActive();
    }
  };

  const onMouseUp = () => {
    if (drawingRef.current) {
      const ann = drawingRef.current;
      drawingRef.current = null;
      // 过滤误触：画笔按点数（≥2 才算画了一笔），其余按尺寸
      const ok = ann.type === "pen"
        ? (ann.points?.length ?? 0) >= 2
        : (Math.abs(ann.x2 - ann.x1) > 3 || Math.abs(ann.y2 - ann.y1) > 3);
      if (ok) {
        setAnnotations((prev) => [...prev, ann]);
        drawActive();  // drawingRef 已 null → 清空 drawCanvas
      } else {
        drawActive();
      }
    }
    dragRef.current = null;
  };

  const undo = () => setAnnotations((prev) => prev.slice(0, -1));

  // —— compose：图像 + 标注 合成到自然尺寸 PNG → Uint8Array（Raw body 二进制传输）——
  const composePngBytes = async (): Promise<ArrayBuffer> => {
    const img = imgRef.current!;
    const c = document.createElement("canvas");
    c.width = natW; c.height = natH;
    const ctx = c.getContext("2d")!;
    ctx.drawImage(img, 0, 0, natW, natH);
    for (const ann of annotations) drawAnnotation(ctx, ann);
    const blob: Blob = await new Promise((resolve, reject) => c.toBlob((b) => b ? resolve(b) : reject("toBlob failed"), "image/png"));
    return await blob.arrayBuffer();
  };

  const handleSave = async () => {
    try {
      if (annotations.length > 0) {
        // 有标注：前端 Canvas 合成 → Raw body 传后端
        const pngBytes = await composePngBytes();
        await invoke("save_image_dialog", pngBytes as unknown as Record<string, unknown>);
      } else if (imageId != null) {
        // 无标注：后端直接从 DB 保存原始数据
        await invoke("save_image_item", { id: imageId, format: "png" });
      }
    } catch (e) { console.error(e); }
  };

  const handleCopy = async () => {
    if (annotations.length > 0) {
      // 有标注：前端 Canvas 合成 → Raw body 传后端写剪贴板
      try {
        const pngBytes = await composePngBytes();
        await invoke("copy_image_to_clipboard", pngBytes as unknown as Record<string, unknown>);
      } catch (e) { console.error(e); }
    }
    // 无标注：剪贴板已有数据（截图/滚动截图停止时已写入），无需操作
  };

  const handleOcr = async () => {
    if (imageId == null) return;
    try {
      const text = await invoke<string>("ocr_image", { id: imageId });
      if (text) {
        // 识别结果存为 source=ocr 的笔记 → 打开记事本并选中（用户可在笔记里编辑）
        const noteId = await invoke<number>("save_ocr_to_note", { text });
        await invoke("open_notepad_with_note", { noteId });
        setOcrCopied(true);
        setTimeout(() => setOcrCopied(false), 1500);
      }
    } catch (e) { console.error(e); }
  };

  const toggleAlwaysOnTop = async () => {
    const next = !alwaysOnTop;
    setAlwaysOnTop(next);
    try {
      await getCurrentWindow().setAlwaysOnTop(next);
    } catch (e) { console.error(e); }
  };

  const close = async () => {
    try { await invoke("close_image_preview"); } catch (e) { console.error(e); }
  };

  // Esc 关闭 / Cmd/Ctrl+Z 撤销
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
      if ((e.metaKey || e.ctrlKey) && e.key === "z") { e.preventDefault(); undo(); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [annotations]);

  const dispW = natW * zoom;
  const dispH = natH * zoom;
  // 图像格式：从 dataUrl 前缀解析（data:image/png;base64,… → PNG），底部 EXIF 条显示
  const fmt = dataUrl ? (dataUrl.match(/^data:image\/([a-zA-Z0-9.+-]+)/)?.[1] ?? "").toUpperCase() : "";

  // 文字草稿 textarea 显示位置（相对 canvas wrapper：自然 ×zoom）
  const draftBox = textDraft
    ? { left: textDraft.nx * zoom, top: textDraft.ny * zoom, fs: toolFontSize * zoom }
    : null;

  return (
    // 灯箱暗场：深 stone 让图片本身发光；工具卡与底部 EXIF 条均 fixed 浮于其上
    <div className="relative h-screen overflow-hidden select-none" style={{ background: "#18181b" }}>
      <Toolbar
        tool={tool} setTool={setTool}
        toolColor={toolColor} setToolColor={setToolColorSync}
        toolWidth={toolWidth} setToolWidth={setToolWidthSync}
        toolFontSize={toolFontSize} setToolFontSize={setToolFontSizeSync}
        alwaysOnTop={alwaysOnTop} onToggleTop={toggleAlwaysOnTop}
        onSave={handleSave} onCopy={handleCopy} onOcr={handleOcr}
        onUndo={undo} canUndo={annotations.length > 0}
        ocrCopied={ocrCopied}
        zoom={zoom} onZoomIn={zoomIn} onZoomOut={zoomOut} onZoomReset={zoomReset}
        onZoomFitWidth={zoomFitWidth} onZoomFitWindow={zoomFitWindow}
      />
      {/* 滚动容器：全屏画布，图片大于视口自动出滚动条；小于则居中 */}
      <div ref={scrollContainerRef} className="absolute inset-0 overflow-auto thin-scrollbar">
        <div className="flex min-h-full min-w-full items-center justify-center px-2 pt-14 pb-2">
          {/* canvas wrapper：棋盘格底显透明 PNG（zinc 冷灰系，不干扰色彩判断）*/}
          <div className="relative" style={{
            width: dispW || undefined, height: dispH || undefined,
            backgroundColor: "#27272a",
            backgroundImage:
              "linear-gradient(45deg, #1e1e22 25%, transparent 25%)," +
              "linear-gradient(-45deg, #1e1e22 25%, transparent 25%)," +
              "linear-gradient(45deg, transparent 75%, #1e1e22 75%)," +
              "linear-gradient(-45deg, transparent 75%, #1e1e22 75%)",
            backgroundSize: "14px 14px",
            backgroundPosition: "0 0, 0 7px, 7px -7px, -7px 0px",
          }}>
            {/* 底层：底图 + 已确认标注 */}
            <canvas
              ref={bgCanvasRef}
              className="absolute inset-0 block"
              style={{ width: dispW, height: dispH }}
            />
            {/* 顶层：正在绘制的笔迹/形状预览；pointer 事件绑此层 */}
            <canvas
              ref={drawCanvasRef}
              className="absolute inset-0 block"
              style={{
                width: dispW, height: dispH,
                cursor: tool === "none" ? (panning ? "grabbing" : "grab") : "crosshair",
              }}
              onMouseDown={onMouseDown}
              onMouseMove={onMouseMove}
              onMouseUp={onMouseUp}
            />
            {dataUrl && (
              <img
                ref={imgRef}
                src={dataUrl}
                alt=""
                crossOrigin="anonymous"
                style={{ display: "none" }}
                onLoad={(e) => {
                  // natW/natH 已在 useEffect 加载流程中设置
                  // 此处仅兜底：确保 imgRef.current 指向 React 渲染的最新 img
                  if (!imgRef.current || imgRef.current !== e.currentTarget) {
                    imgRef.current = e.currentTarget;
                  }
                }}
              />
            )}
            {draftBox && (
              <textarea
                ref={textInputRef}
                value={textDraft!.val}
                onChange={(e) => {
                  const val = e.target.value;
                  const next = { ...textDraft!, val };
                  textDraftRef.current = next;
                  setTextDraft(next);
                }}
                onBlur={commitText}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); commitText(); }
                  if (e.key === "Escape") { textDraftRef.current = null; setTextDraft(null); }
                }}
                placeholder="输入文字…"
                className="absolute rounded bg-white/95 px-1 py-0.5 shadow outline-none resize-none"
                style={{
                  left: draftBox.left, top: draftBox.top, fontSize: draftBox.fs, minWidth: 120, lineHeight: 1.3,
                  // 按选定色显示（写时即所见）；白色字加细深描边防在白底上丢失
                  color: toolColor,
                  WebkitTextStroke: toolColor.toLowerCase() === "#ffffff" ? "0.4px #999" : "none",
                }}
              />
            )}
          </div>
        </div>
      </div>

      {/* 底部 EXIF 状态条：等宽 tabular-nums，半透 blur 融于暗场 */}
      {natW > 0 && (
        <div style={{
          position: "fixed", bottom: 6, left: "50%", transform: "translateX(-50%)", zIndex: 100,
          padding: "3px 10px", borderRadius: 6,
          background: "rgba(24,24,27,0.72)",
          backdropFilter: "blur(12px)", WebkitBackdropFilter: "blur(12px)",
          color: "rgba(255,255,255,0.5)", fontSize: 10, fontWeight: 500,
          fontFamily: "SF Mono, Menlo, monospace", fontVariantNumeric: "tabular-nums",
          boxShadow: "0 1px 8px rgba(0,0,0,0.25)",
          display: "flex", gap: 8, alignItems: "center", pointerEvents: "none",
        }}>
          <span>{fullNatW || natW} × {fullNatH || natH}</span>
          {fmt && <>
            <span style={{ opacity: 0.3 }}>·</span>
            <span>{fmt}</span>
          </>}
        </div>
      )}
    </div>
  );
}
