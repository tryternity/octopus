import { useState, useRef, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";
import {
  type Annotation,
  type Tool,
  drawAnnotation,
  drawMosaic,
  hitTestAnnotationPrecise,
} from "@/lib/annotation";
import Toolbar from "./Toolbar";
import { AnnotationSvg } from "./AnnotationSvg";
import { computeVisibleRect, visibleToViewport, computeSrcSlice } from "./viewportMath";
import { openCompactEditorTab } from "@/lib/compactEditor";
import { MIN_ZOOM, MAX_ZOOM, ZOOM_STEP, TOOLBAR_H, FIT_PADDING, computeFitZoom, computeFitToWidthZoom } from "./zoom";

/**
 * 剪贴板图片项的预览窗口（轻工具栏形态）。
 *
 * 显示：默认 fit-to-window 打开（缩略图秒开 → 全图异步替换）；图片超出窗口则滚动容器自动出滚动条（上下+左右），
 * 工具栏放大/缩小按钮调 zoom。标注用「自然像素」坐标（与 zoom 解耦）——绘制时
 * ctx.scale(zoom)，鼠标 /zoom 反算；合成保存/复制在自然尺寸画布 1:1 重绘（与 zoom 无关）。
 */

export default function ImagePreview({ imageId: propImageId, initialWidth, initialHeight }: { imageId: number; initialWidth?: number; initialHeight?: number }) {
  const bgCanvasRef = useRef<HTMLCanvasElement>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const wrapperRef = useRef<HTMLDivElement>(null);
  // 视口尺寸（ResizeObserver 跟踪，用于手算居中 + drawBg 裁剪）
  const [viewport, setViewport] = useState({ w: 0, h: 0 });

  const [imageId, setImageId] = useState<number | null>(propImageId);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  // URL 注入的初始尺寸——图片 tab 打开时首帧即有正确宽高，消除布局突变。
  // 缩略图 onload 后会被真实值覆盖（但值相同，无视觉跳变）。
  const [natW, setNatW] = useState(initialWidth || 0);
  const [natH, setNatH] = useState(initialHeight || 0);
  // zoom 倍率，1.0 = 1:1 自然分辨率（默认）
  const [zoom, setZoom] = useState(1);
  // 抓手平移中（tool==="none" 未命中标注时按住拖拽平移视口，免拖滚动条）
  const [panning, setPanning] = useState(false);

  const [tool, setTool] = useState<Tool>("none");
  const [toolColor, setToolColor] = useState("#ef4444");
  const [toolWidth, setToolWidth] = useState(3);
  const [toolFontSize, setToolFontSize] = useState(20);
  const [filled, setFilled] = useState(false);
  const [popoverDismissKey, setPopoverDismissKey] = useState(0);
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const redoStackRef = useRef<Annotation[]>([]);
  const [redoAvailable, setRedoAvailable] = useState(false);
  // 正在绘制的标注预览（SVG overlay 渲染，不触发 canvas 重绘）
  const [draftAnn, setDraftAnn] = useState<Annotation | null>(null);
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);
  const [ocrCopied, setOcrCopied] = useState(false);
  const [ocrWarn, setOcrWarn] = useState(false);
  const [ocrCopiedText, setOcrCopiedText] = useState<string | null>(null);
  interface OcrBlock { text: string; x: number; y: number; w: number; h: number; score: number; }
  const [ocrBlocks, setOcrBlocks] = useState<OcrBlock[]>([]);
  const [ocrOverlay, setOcrOverlay] = useState<'off' | 'overlay' | 'mask'>('off');
  const ocrDoneRef = useRef(false);  // 防重复 OCR（截图 OCR 已推送 blocks 后不再重跑）
  // 全图加载中：true 时禁止标注（避免 thumb 坐标系与 full 坐标系不一致）
  const loadingFullRef = useRef(false);
  // 全图已加载：true 时缩略图后到直接丢弃（防竞态降级）
  const fullLoadedRef = useRef(false);
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

  // —— unmount：revoke objectURL + close bitmap + cancel RAF，防内存泄漏 ——
  useEffect(() => {
    return () => {
      if (objectUrlRef.current) URL.revokeObjectURL(objectUrlRef.current);
      if (scaledBitmapRef.current) scaledBitmapRef.current.close();
    };
  }, []);

  // —— mount：取 PENDING + 监听并发再开的 load 事件 ——
  // imageId 由 props 驱动
  useEffect(() => { setImageId(propImageId); }, [propImageId]);

  // 截图 OCR → 推送 OCR blocks
  useEffect(() => {
    const unlistenOcr = listen<{ text: string; blocks: OcrBlock[] }>("ocr-screenshot://result", (e) => {
      if (e.payload.blocks.length > 0) {
        ocrDoneRef.current = true;
        setOcrBlocks(e.payload.blocks);
        setOcrOverlay('overlay');
      }
    });
    return () => { unlistenOcr.then((f) => f()); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // —— imageId 变 → 并行拉缩略图（秒开）+ 全图（异步替换） ——
  useEffect(() => {
    if (imageId == null) return;
    let cancelled = false;
    // 清理旧资源
    setOcrBlocks([]);
    setOcrOverlay('off');
    ocrDoneRef.current = false;
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
    fullLoadedRef.current = false;

    const thumbPromise = invoke<string>("get_image_thumb", { id: imageId });
    const fullPromise = invoke<ArrayBuffer>("get_image_full", { id: imageId });

    // 缩略图先到 → 立即显示
    thumbPromise.then((thumbDataUrl) => {
      if (cancelled) return;
      const thumbImg = new Image();
      thumbImg.crossOrigin = "anonymous";
      thumbImg.onload = () => {
        if (cancelled || fullLoadedRef.current) return; // 全图已加载→丢弃滞后的缩略图
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
        fullLoadedRef.current = true;
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
          // 用户已手动缩放：等比例修正 zoom 保持视觉大小不变
          const prevNatW = natW;
          const ratio = prevNatW / fullImg.naturalWidth;
          setNatW(fullImg.naturalWidth);
          setNatH(fullImg.naturalHeight);
          if (ratio > 0 && ratio !== 1) {
            setZoomSync(zoomRef.current * ratio, true);
          }
        }
        // 强制重新生成位图（全图替换缩略图后 zoom 可能不变，不触发 zoom effect）
        const oldBitmap = scaledBitmapRef.current;
        scaledBitmapRef.current = null;
        if (oldBitmap) oldBitmap.close();
        zoomVersionRef.current++;
        drawBg();  // 显式重绘（覆盖 thumb/full 同尺寸时 zoom effect 不触发的边界）
      };
      fullImg.src = url;
    }).catch((e) => {
      console.error("full failed:", e);
      // 全图加载失败也须释放防误触锁，否则 loadingFullRef 永久 true、标注被
      // L444 永久拦截（用户在该图上再也画不了）。退回缩略图模式仍可做基础标注。
      if (!cancelled) loadingFullRef.current = false;
    });

    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [imageId]);

  // wrapper div ref（鼠标坐标用）
  // 视口渲染：canvas sticky 钉 scrollContainer 视口，物理尺寸 = 视口×dpr（永不超 32767），
  // 只画图片露出视口的切片。wrapper（SVG overlay/鼠标）随 content 滚，canvas 不随滚。全部坐标手算。

  // 图片在 content 空间中的 left/top（直接读 DOM，与 drawBg 同源同步）
  const dispW = natW * zoom;
  const dispH = natH * zoom;
  const scForLayout = scrollContainerRef.current;
  const currentVW = scForLayout?.clientWidth || viewport.w;
  const imgLeft = Math.max(FIT_PADDING / 2, (currentVW - dispW) / 2);
  const imgTop = TOOLBAR_H;

  // —— drawBg：canvas 视口固定（sticky 钉 scrollContainer 视口），物理尺寸 = 视口×dpr（永不超
  // Chromium 32767 单边硬限，长图不再崩）。只画图片露出视口的切片到视口坐标 (dstL,dstT)。
  // 几何换算见 viewportMath.ts（纯函数，已单测）；DOM/sticky 对齐靠 GUI 验证。
  const drawBg = useCallback(() => {
    const canvas = bgCanvasRef.current;
    const img = imgRef.current;
    const sc = scrollContainerRef.current;
    if (!canvas || !img || !sc) return;
    if (!natW || !natH) return;
    if (dispW <= 0 || dispH <= 0) return; // 防零除（MIN_ZOOM=0.1 兜底，双保险）
    const dpr = window.devicePixelRatio || 1;
    // 视口 CSS 像素（不含 scrollbar）= canvas 物理尺寸基准
    const vw = sc.clientWidth;
    const vh = sc.clientHeight;
    if (vw <= 0 || vh <= 0) return;
    const cw = Math.min(Math.round(vw * dpr), 32767);
    const ch = Math.min(Math.round(vh * dpr), 32767);
    if (canvas.width !== cw) canvas.width = cw;
    if (canvas.height !== ch) canvas.height = ch;
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    // 清整个视口画布（canvas sticky 不随滚，但内容随滚变化，须全清防残留旧帧）
    ctx.clearRect(0, 0, vw, vh);
    // 图片露出视口的区域（content 空间交集）
    const vis = computeVisibleRect(
      imgLeft, imgTop, dispW, dispH,
      sc.scrollLeft, sc.scrollTop, vw, vh,
    );
    if (!vis) return; // 图片不在视口 → 已清空，返回
    // dst（视口坐标）+ src 切片（bitmap 物理空间 或 img 自然空间，公式一致）
    const { dstL, dstT, dstW, dstH } = visibleToViewport(vis, sc.scrollLeft, sc.scrollTop);
    const bitmap = scaledBitmapRef.current;
    const srcW = bitmap ? bitmap.width : img.naturalWidth;
    const srcH = bitmap ? bitmap.height : img.naturalHeight;
    const { sx, sy, sw, sh } = computeSrcSlice(vis, imgLeft, imgTop, dispW, dispH, srcW, srcH);
    ctx.drawImage(bitmap || img, sx, sy, sw, sh, dstL, dstT, dstW, dstH);
  }, [natW, natH, zoom, viewport, imgLeft, imgTop, dispW, dispH]);

  useEffect(() => { drawBg(); }, [drawBg]);

  // viewport 尺寸跟踪（ResizeObserver）
  useEffect(() => {
    const sc = scrollContainerRef.current;
    if (!sc) return;
    const update = () => setViewport({ w: sc.clientWidth, h: sc.clientHeight });
    update();
    const ro = new ResizeObserver(update);
    ro.observe(sc);
    return () => ro.disconnect();
  }, []);

  // scroll RAF：canvas 在 wrapper 内随滚动原生移动，RAF 只画新暴露的区域
  useEffect(() => {
    const sc = scrollContainerRef.current;
    if (!sc) return;
    let raf = 0;
    const onScroll = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => { drawBg(); });
    };
    sc.addEventListener('scroll', onScroll, { passive: true });
    return () => { sc.removeEventListener('scroll', onScroll); cancelAnimationFrame(raf); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [drawBg]);

  // zoom 变化 → 异步生成预缩放位图（debounce 150ms，期间 drawBg 用原图拉伸占位）→ drawBg
  useEffect(() => {
    const img = imgRef.current;
    if (!img || !natW || !natH) return;
    const dpr = window.devicePixelRatio || 1;
    const pw = Math.round(dispW * dpr);
    const ph = Math.round(dispH * dpr);
    if (pw < 1 || ph < 1) return;
    // 防抖：快速缩放时等用户停下来再生成高质量位图
    const timer = setTimeout(() => {
      const version = ++zoomVersionRef.current;
      createImageBitmap(img, {
        resizeWidth: pw,
        resizeHeight: ph,
        resizeQuality: "high",
      }).then((bitmap) => {
        if (version !== zoomVersionRef.current) {
          bitmap.close();
          return;
        }
        const old = scaledBitmapRef.current;
        scaledBitmapRef.current = bitmap;
        if (old) old.close();
        drawBg();
      }).catch(() => {});
    }, 150);
    return () => { clearTimeout(timer); zoomVersionRef.current++; };
  }, [zoom, natW, natH]); // eslint-disable-line react-hooks/exhaustive-deps

  // CSS 坐标（相对图片左上角，含滚动偏移）→ 自然坐标（/zoom）
  const toNatural = (cssX: number, cssY: number) => {
    return { nx: cssX / zoomRef.current, ny: cssY / zoomRef.current };
  };

  const canvasCoords = (e: React.MouseEvent) => {
    // 手算图片在屏幕上的位置（不查 DOM 布局）
    const sc = scrollContainerRef.current!;
    const scRect = sc.getBoundingClientRect();
    const imgScreenX = scRect.left + imgLeft - sc.scrollLeft;
    const imgScreenY = scRect.top + imgTop - sc.scrollTop;
    return { cssX: e.clientX - imgScreenX, cssY: e.clientY - imgScreenY };
  };

  const commitText = () => {
    const d = textDraftRef.current;
    if (d && d.val.trim()) {
      // 记录 textarea 实际宽度（自然像素），供导出时折行参考
      const textWidth = textInputRef.current
        ? textInputRef.current.clientWidth / zoomRef.current
        : undefined;
      addAnnotation({
        type: "text", x1: d.nx, y1: d.ny, x2: d.nx, y2: d.ny,
        text: d.val, color: toolColorRef.current, fontSize: toolFontSizeRef.current,
        textWidth,
      });
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
    // 用户开始操作画布 → 收起工具栏浮窗
    setPopoverDismissKey((k) => k + 1);
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

    // 序号：点击放置，自动递增编号
    if (tool === "number") {
      const maxNum = annotations.reduce((max, a) => {
        if (a.type === "number" && a.number && a.number > max) return a.number;
        return max;
      }, 0);
      const ann: Annotation = {
        type: "number", x1: nx, y1: ny, x2: nx, y2: ny,
        number: maxNum + 1,
        color: toolColorRef.current, circleSize: 28,
      };
      drawingRef.current = ann;
      addAnnotation(ann);
      drawingRef.current = null;
      setDraftAnn(null);
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
      filled: (tool === "rect" || tool === "oval" || tool === "diamond") ? filled : undefined,
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
        setDraftAnn({ ...drawingRef.current, points: [...drawingRef.current.points] });
      } else {
        drawingRef.current = { ...drawingRef.current, x2: nx, y2: ny };
        setDraftAnn({ ...drawingRef.current });
      }
      return;
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
        addAnnotation(ann);
      }
      setDraftAnn(null);
    }
    dragRef.current = null;
  };

  const undo = () => {
    setAnnotations((prev) => {
      if (prev.length === 0) return prev;
      redoStackRef.current.push(prev[prev.length - 1]);
      setRedoAvailable(true);
      return prev.slice(0, -1);
    });
  };
  const redo = () => {
    const ann = redoStackRef.current.pop();
    if (ann) {
      addAnnotation(ann);
      setRedoAvailable(redoStackRef.current.length > 0);
    }
  };
  // 新增标注时清空 redo stack
  const addAnnotation = (ann: Annotation) => {
    redoStackRef.current = [];
    setRedoAvailable(false);
    setAnnotations((prev) => [...prev, ann]);
  };

  // —— compose：图像 + 标注 合成到自然尺寸 PNG → Uint8Array（Raw body 二进制传输）——
  const composePngBytes = async (): Promise<ArrayBuffer> => {
    const img = imgRef.current;
    if (!img || !natW || !natH) throw new Error("图片尚未加载完成");
    const c = document.createElement("canvas");
    c.width = natW; c.height = natH;
    const ctx = c.getContext("2d")!;
    ctx.drawImage(img, 0, 0, natW, natH);
    // 先处理 blur（像素马赛克降采样），再画其他标注
    for (const ann of annotations) {
      if (ann.type === "blur") drawMosaic(ctx, ann);
    }
    for (const ann of annotations) {
      if (ann.type === "blur") continue; // blur 已由 drawMosaic 处理，跳过避免色块叠加两次
      drawAnnotation(ctx, ann);
    }
    const blob: Blob = await new Promise((resolve, reject) => c.toBlob((b) => b ? resolve(b) : reject("toBlob failed"), "image/png"));
    return await blob.arrayBuffer();
  };

  const handleSave = async () => {
    try {
      // 统一走前端合成 → save_image_dialog 弹窗（有标注画标注，无标注只画原图）
      const pngBytes = await composePngBytes();
      await invoke("save_image_dialog", pngBytes as unknown as Record<string, unknown>);
    } catch (e) { console.error(e); }
  };

  const handleCopy = async () => {
    if (annotations.length > 0) {
      // 有标注：前端 Canvas 合成 → Raw body 传后端写剪贴板
      try {
        const pngBytes = await composePngBytes();
        await invoke("copy_image_to_clipboard", pngBytes as unknown as Record<string, unknown>);
      } catch (e) { console.error(e); }
    } else if (imageId != null) {
      // 无标注：从 DB 重新写原图到系统剪贴板（剪贴板内容可能已被覆盖）
      try {
        await invoke("copy_clipboard_item", { id: imageId });
      } catch (e) { console.error(e); }
    }
  };

  const handleOcr = async () => {
    if (imageId == null) return;
    // 已识别过 → 三态循环：off → overlay → mask → off（不重新识别）
    if (ocrDoneRef.current) {
      setOcrOverlay(ocrOverlay === 'off' ? 'overlay' : ocrOverlay === 'overlay' ? 'mask' : 'off');
      return;
    }
    // 首次识别
    try {
      const result = await invoke<{text: string; blocks: OcrBlock[]}>("ocr_image", { id: imageId });
      if (result.text) {
        ocrDoneRef.current = true;
        setOcrBlocks(result.blocks);
        setOcrOverlay('overlay');
        // 保持现有行为：入库 + 打开编辑器
        const ocrId = await invoke<number>("insert_ocr_clipboard_item", { text: result.text });
        await openCompactEditorTab(ocrId);
        setOcrCopied(true);
        setTimeout(() => setOcrCopied(false), 1500);
      }
    } catch (e) {
      const msg = String(e);
      if (msg.includes("还未完成")) {
        setOcrWarn(true);
        setTimeout(() => setOcrWarn(false), 1800);
      } else {
        console.error(e);
      }
    }
  };

  const toggleAlwaysOnTop = async () => {
    const next = !alwaysOnTop;
    setAlwaysOnTop(next);
    try {
      await getCurrentWindow().setAlwaysOnTop(next);
    } catch (e) { console.error(e); }
  };

  // Esc 撤销 / Cmd/Ctrl+Z 撤销（不再关窗——ImagePreview 是 CompactEditor 的 tab）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "z") { e.preventDefault(); redo(); return; }
      if ((e.metaKey || e.ctrlKey) && e.key === "z") { e.preventDefault(); undo(); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [annotations]);

  // 图像格式：从 dataUrl 前缀解析（data:image/png;base64,… → PNG），底部 EXIF 条显示
  const fmt = dataUrl ? (dataUrl.match(/^data:image\/([a-zA-Z0-9.+-]+)/)?.[1] ?? "").toUpperCase() : "";

  // 文字草稿 textarea 显示位置（相对 canvas wrapper：自然 ×zoom）
  const draftBox = textDraft
    ? { left: textDraft.nx * zoom, top: textDraft.ny * zoom, fs: toolFontSize * zoom }
    : null;

  return (
    // 灯箱暗场（填满 CompactEditor tab 内容区，不再用 h-screen/fixed）
    <div className="relative h-full w-full overflow-hidden select-none bg-background">
      <Toolbar
        tool={tool} setTool={setTool}
        toolColor={toolColor} setToolColor={setToolColorSync}
        toolWidth={toolWidth} setToolWidth={setToolWidthSync}
        toolFontSize={toolFontSize} setToolFontSize={setToolFontSizeSync}
        alwaysOnTop={alwaysOnTop} onToggleTop={toggleAlwaysOnTop}
        onSave={handleSave} onCopy={handleCopy} onOcr={handleOcr}
        onUndo={undo} canUndo={annotations.length > 0}
        onRedo={redo} canRedo={redoAvailable}
        ocrCopied={ocrCopied} ocrWarn={ocrWarn}
        ocrMode={ocrOverlay}
        zoom={zoom} onZoomIn={zoomIn} onZoomOut={zoomOut} onZoomReset={zoomReset}
        onZoomFitWidth={zoomFitWidth} onZoomFitWindow={zoomFitWindow}
        filled={filled} setFilled={setFilled}
        popoverDismissKey={popoverDismissKey}
      />
      {/* 滚动容器：canvas + wrapper 撑滚动条 + SVG overlay + 鼠标事件，全部在同一 scroll context */}
      <div ref={scrollContainerRef} className="absolute inset-0 overflow-auto thin-scrollbar" style={{ zIndex: 2 }}>
        {/* content：撑起滚动区域，至少 = viewport 尺寸（保证居中正确） */}
        <div style={{
          position: "relative",
          width: Math.max(dispW + FIT_PADDING, viewport.w),
          height: Math.max(dispH + TOOLBAR_H + 8, viewport.h),
        }}
        onMouseDown={() => {
          // 暗区点击不再关窗（CompactEditor tab 模式）
        }}>
          {/* canvas：底图，视口固定——sticky 钉 scrollContainer 视口左上，物理尺寸 = 视口×dpr
              （永不超 Chromium 32767 单边硬限，长图不再崩）。DOM 先于 wrapper → 默认 stack 在下层；
              pointer-events:none 让鼠标穿透到 wrapper。drawBg 只画图片露出视口的切片。 */}
          <canvas
            ref={bgCanvasRef}
            style={{
              position: "sticky",
              top: 0,
              left: 0,
              width: viewport.w || undefined,
              height: viewport.h || undefined,
              display: "block",
              pointerEvents: "none",
            }}
          />
          {/* wrapper：absolute 定位（手算居中），透明背景（canvas 在下层画底图）+ SVG + 鼠标 */}
          <div ref={wrapperRef}
            style={{
              position: "absolute",
              left: imgLeft, top: imgTop,
              width: dispW || undefined, height: dispH || undefined,
              cursor: tool === "none" ? (panning ? "grabbing" : "grab") : "crosshair",
            }}
            onMouseDown={onMouseDown}
            onMouseMove={onMouseMove}
            onMouseUp={onMouseUp}
          >
            {/* canvas 已移出 wrapper（视口固定 sticky）；此处为 SVG overlay / OCR / img / textarea */}
            {/* OCR 文本块叠加层（三态：off / overlay / mask） */}
            {ocrOverlay !== 'off' && ocrBlocks.length > 0 && (
              <svg className="absolute inset-0 block"
                viewBox={`0 0 ${natW} ${natH}`}
                preserveAspectRatio="none"
                style={{ width: dispW, height: dispH, pointerEvents: "none" }}
              >
                {/* 第一遍：所有遮罩底（避免后面的 rect 盖住前面的 text） */}
                {ocrBlocks.map((b, i) => (
                  <rect key={`bg-${i}`} x={b.x} y={b.y} width={b.w} height={b.h}
                    fill={ocrOverlay === 'mask' ? "rgba(255,255,255,0.92)" : "rgba(59,130,246,0.08)"}
                    stroke={ocrOverlay === 'mask' ? "rgba(0,0,0,0.1)" : "rgba(59,130,246,0.4)"}
                    strokeWidth={1} rx={2}
                    style={{ cursor: 'pointer', pointerEvents: 'all' }}
                    onDoubleClick={(e) => {
                      e.stopPropagation();
                      navigator.clipboard?.writeText(b.text).then(() => {
                        setOcrCopiedText(`已复制：${b.text.length > 20 ? b.text.slice(0, 20) + '…' : b.text}`);
                        setTimeout(() => setOcrCopiedText(null), 2000);
                      }).catch(() => {});
                    }}
                  />
                ))}
                {/* 第二遍：所有文字（保证在前面的 rect 之上） */}
                {ocrBlocks.map((b, i) => (
                  <text key={`tx-${i}`} x={b.x + 2} y={b.y + b.h - 2}
                    fontSize={Math.min(b.h * 0.8, 14)}
                    fill={ocrOverlay === 'mask' ? "rgba(0,0,0,0.85)" : "rgba(59,130,246,0.7)"}
                    dominantBaseline="alphabetic"
                    style={{ pointerEvents: 'none', userSelect: 'none' }}>
                    {b.text}
                  </text>
                ))}
              </svg>
            )}
            {/* SVG overlay：标注。viewBox 自然坐标，随 wrapper 滚动 */}
            {natW > 0 && (
              <svg
                className="absolute inset-0 block"
                viewBox={`0 0 ${natW} ${natH}`}
                preserveAspectRatio="none"
                style={{ width: dispW, height: dispH, pointerEvents: "none" }}
              >
                {annotations.map((ann, i) => (
                  <AnnotationSvg key={i} ann={ann} />
                ))}
                {draftAnn && <AnnotationSvg ann={draftAnn} />}
              </svg>
            )}
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
                  if (e.key === "Escape") { e.stopPropagation(); textDraftRef.current = null; setTextDraft(null); }
                }}
                placeholder="输入文字…"
                className="absolute rounded bg-background px-1 py-0.5 shadow outline-none resize-none border border-border"
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
          position: "absolute", bottom: 6, left: "50%", transform: "translateX(-50%)", zIndex: 100,
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

      {/* OCR 双击复制提示浮泡 */}
      {ocrCopiedText && (
        <div style={{
          position: "absolute", top: 50, left: "50%", transform: "translateX(-50%)", zIndex: 200,
          padding: "6px 14px", borderRadius: 8,
          background: "rgba(34,197,94,0.95)", color: "#fff",
          fontSize: 12, fontWeight: 600, fontFamily: "-apple-system, sans-serif",
          boxShadow: "0 4px 12px rgba(0,0,0,0.2)", pointerEvents: "none",
          animation: "fadeIn 0.2s ease",
        }}>
          {ocrCopiedText}
        </div>
      )}
    </div>
  );
}
