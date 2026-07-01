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

/**
 * 剪贴板图片项的预览窗口（轻工具栏形态）。
 *
 * 显示：默认 1:1（自然分辨率）打开；图片超出窗口则滚动容器自动出滚动条（上下+左右），
 * 工具栏放大/缩小按钮调 zoom。标注用「自然像素」坐标（与 zoom 解耦）——绘制时
 * ctx.scale(zoom)，鼠标 /zoom 反算；合成保存/复制在自然尺寸画布 1:1 重绘（与 zoom 无关）。
 */
export default function ImagePreview() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
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

  // 交互 refs（避免重渲染抖动 + 拖拽用最新值）
  const drawingRef = useRef<Annotation | null>(null);
  const dragRef = useRef<{ idx: number; dx: number; dy: number } | null>(null);
  const toolColorRef = useRef("#ef4444");
  const toolWidthRef = useRef(3);
  const toolFontSizeRef = useRef(20);
  const zoomRef = useRef(1);
  // 文字输入框 ref：autoFocus 对动态挂载的 textarea 不可靠，改 setTimeout focus（对齐截图）
  const textInputRef = useRef<HTMLTextAreaElement | null>(null);
  // 文字草稿：state 驱动 textarea 渲染，ref 镜像供 commitText 读最新输入
  const textDraftRef = useRef<{ nx: number; ny: number; val: string } | null>(null);
  const [textDraft, setTextDraft] = useState<{ nx: number; ny: number; val: string } | null>(null);

  const setToolColorSync = (c: string) => { toolColorRef.current = c; setToolColor(c); };
  const setToolWidthSync = (n: number) => { toolWidthRef.current = n; setToolWidth(n); };
  const setToolFontSizeSync = (n: number) => { toolFontSizeRef.current = n; setToolFontSize(n); };
  const setZoomSync = (z: number) => {
    const clamped = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, z));
    zoomRef.current = clamped;
    setZoom(clamped);
  };
  const zoomIn = () => setZoomSync(zoomRef.current * ZOOM_STEP);
  const zoomOut = () => setZoomSync(zoomRef.current / ZOOM_STEP);
  const zoomReset = () => setZoomSync(1);

  // —— mount：取 PENDING + 监听并发再开的 load 事件 ——
  useEffect(() => {
    invoke<{ imageId: number } | null>("get_pending_image").then((p) => {
      if (p) setImageId(p.imageId);
    });
    const unlisten = listen<{ imageId: number }>("image-preview://load", (e) => {
      setImageId(e.payload.imageId);
      setAnnotations([]);
      setZoomSync(1);
    });
    return () => { unlisten.then((f) => f()); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // —— imageId 变 → 拉全图 ——
  useEffect(() => {
    if (imageId == null) return;
    invoke<string>("get_image_full", { id: imageId })
      .then((url) => {
        setDataUrl(url);
        setAnnotations([]);
        setZoomSync(1);
      })
      .catch((e) => console.error(e));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [imageId]);

  // —— draw：1:1 × zoom，图片 + 标注（自然坐标 × zoom）——
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const img = imgRef.current;
    if (!canvas || !img || !natW || !natH) return;
    const dispW = natW * zoom;
    const dispH = natH * zoom;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(dispW * dpr);
    canvas.height = Math.round(dispH * dpr);
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, dispW, dispH);
    ctx.drawImage(img, 0, 0, dispW, dispH);
    // 标注：自然坐标 → ×zoom 缩放到显示空间
    ctx.save();
    ctx.scale(zoom, zoom);
    for (const ann of annotations) drawAnnotation(ctx, ann);
    if (drawingRef.current) drawAnnotation(ctx, drawingRef.current);
    ctx.restore();
  }, [natW, natH, zoom, annotations]);

  useEffect(() => { draw(); }, [draw]);

  // CSS 坐标（相对 canvas，已含滚动偏移）→ 自然坐标（/zoom）
  const toNatural = (cssX: number, cssY: number) => {
    return { nx: cssX / zoomRef.current, ny: cssY / zoomRef.current };
  };

  const canvasCoords = (e: React.MouseEvent) => {
    const rect = canvasRef.current!.getBoundingClientRect();
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

    // rect/oval/line 开始绘制（自然坐标）
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
      drawingRef.current = { ...drawingRef.current, x2: nx, y2: ny };
      draw();
    }
  };

  const onMouseUp = () => {
    if (drawingRef.current) {
      const ann = drawingRef.current;
      drawingRef.current = null;
      // 过滤误触（过小）
      if (Math.abs(ann.x2 - ann.x1) > 3 || Math.abs(ann.y2 - ann.y1) > 3) {
        setAnnotations((prev) => [...prev, ann]);
      } else {
        draw();
      }
    }
    dragRef.current = null;
  };

  const undo = () => setAnnotations((prev) => prev.slice(0, -1));

  // —— compose：图像 + 标注 合成到自然尺寸 PNG → base64（与 zoom 无关，1:1 全分辨率）——
  const composePngBase64 = (): string => {
    const img = imgRef.current!;
    const c = document.createElement("canvas");
    c.width = natW; c.height = natH;
    const ctx = c.getContext("2d")!;
    ctx.drawImage(img, 0, 0, natW, natH);
    for (const ann of annotations) drawAnnotation(ctx, ann);
    const url = c.toDataURL("image/png");
    return url.substring(url.indexOf(",") + 1);
  };

  const handleSave = async () => {
    try {
      await invoke("save_image_dialog", { pngBase64: composePngBase64() });
    } catch (e) { console.error(e); }
  };

  const handleCopy = async () => {
    try {
      await invoke("copy_image_to_clipboard", { pngBase64: composePngBase64() });
    } catch (e) { console.error(e); }
  };

  const handleOcr = async () => {
    if (imageId == null) return;
    try {
      const text = await invoke<string>("ocr_image", { id: imageId });
      if (text) {
        await navigator.clipboard.writeText(text).catch(() => {});
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

  // 文字草稿 textarea 显示位置（相对 canvas wrapper：自然 ×zoom）
  const draftBox = textDraft
    ? { left: textDraft.nx * zoom, top: textDraft.ny * zoom, fs: toolFontSize * zoom }
    : null;

  return (
    <div className="flex h-screen flex-col bg-neutral-900 select-none">
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
      />
      {/* 滚动容器：图片大于视口时自动出上下/左右滚动条；小于则居中 */}
      <div ref={scrollContainerRef} className="relative flex-1 overflow-auto">
        <div className="flex min-h-full min-w-full items-center justify-center p-4">
          {/* canvas wrapper：relative 让 textarea 相对 canvas 定位、随滚动移动 */}
          <div className="relative" style={{ width: dispW || undefined, height: dispH || undefined }}>
            <canvas
              ref={canvasRef}
              className="block"
              style={{ width: dispW, height: dispH, cursor: tool === "none" ? (panning ? "grabbing" : "grab") : "crosshair" }}
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
                  setNatW(e.currentTarget.naturalWidth);
                  setNatH(e.currentTarget.naturalHeight);
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
                className="absolute rounded bg-white/95 px-1 py-0.5 text-black shadow outline-none resize-none"
                style={{ left: draftBox.left, top: draftBox.top, fontSize: draftBox.fs, minWidth: 120, lineHeight: 1.3 }}
              />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
