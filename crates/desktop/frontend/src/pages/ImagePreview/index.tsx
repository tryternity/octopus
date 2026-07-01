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

/**
 * 剪贴板图片项的预览窗口（轻工具栏形态）。
 *
 * 坐标空间：标注用「自然像素」（图像本征分辨率），与显示尺寸解耦——窗口可缩放，
 * resize 不会让标注错位。屏幕绘制时 ctx 先 scale(disp/nat) 再 drawAnnotation；
 * 鼠标 CSS 坐标 → 自然坐标用 /disp*nat 反算；合成保存/复制在自然尺寸画布 1:1 重绘。
 */
export default function ImagePreview() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);

  const [imageId, setImageId] = useState<number | null>(null);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [natW, setNatW] = useState(0);
  const [natH, setNatH] = useState(0);
  // contain-fit 后的显示矩形（CSS px），draw 时算，供鼠标坐标换算与文字草稿定位读取
  const dispRef = useRef({ w: 0, h: 0, ox: 0, oy: 0, scale: 1 });

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
  // 文字草稿：state 驱动 textarea 渲染，ref 镜像供 commitText 读最新输入
  const textDraftRef = useRef<{ nx: number; ny: number; val: string } | null>(null);
  const [textDraft, setTextDraft] = useState<{ nx: number; ny: number; val: string } | null>(null);

  const setToolColorSync = (c: string) => { toolColorRef.current = c; setToolColor(c); };
  const setToolWidthSync = (n: number) => { toolWidthRef.current = n; setToolWidth(n); };
  const setToolFontSizeSync = (n: number) => { toolFontSizeRef.current = n; setToolFontSize(n); };

  // —— mount：取 PENDING + 监听并发再开的 load 事件 ——
  useEffect(() => {
    invoke<{ imageId: number } | null>("get_pending_image").then((p) => {
      if (p) setImageId(p.imageId);
    });
    const unlisten = listen<{ imageId: number }>("image-preview://load", (e) => {
      setImageId(e.payload.imageId);
      setAnnotations([]);
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  // —— imageId 变 → 拉全图 ——
  useEffect(() => {
    if (imageId == null) return;
    invoke<string>("get_image_full", { id: imageId })
      .then((url) => {
        setDataUrl(url);
        setAnnotations([]);
      })
      .catch((e) => console.error(e));
  }, [imageId]);

  // —— draw：contain-fit + 图片 + 标注 + 进行中的草稿 ——
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const img = imgRef.current;
    if (!canvas || !img || !natW || !natH) return;
    const cssW = canvas.clientWidth;
    const cssH = canvas.clientHeight;
    if (!cssW || !cssH) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(cssW * dpr);
    canvas.height = Math.round(cssH * dpr);
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);

    // contain-fit
    const scale = Math.min(cssW / natW, cssH / natH);
    const dispW = natW * scale;
    const dispH = natH * scale;
    const ox = (cssW - dispW) / 2;
    const oy = (cssH - dispH) / 2;
    dispRef.current = { w: dispW, h: dispH, ox, oy, scale };
    ctx.drawImage(img, ox, oy, dispW, dispH);

    // 标注：自然坐标 → 平移到显示原点 + 缩放到 disp
    ctx.save();
    ctx.translate(ox, oy);
    ctx.scale(scale, scale);
    for (const ann of annotations) drawAnnotation(ctx, ann);
    if (drawingRef.current) drawAnnotation(ctx, drawingRef.current);
    ctx.restore();
  }, [natW, natH, annotations]);

  useEffect(() => { draw(); }, [draw]);

  useEffect(() => {
    const onResize = () => draw();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [draw]);

  // CSS 坐标（相对 canvas）→ 自然坐标
  const toNatural = (cssX: number, cssY: number) => {
    const { ox, oy, scale } = dispRef.current;
    return { nx: (cssX - ox) / scale, ny: (cssY - oy) / scale };
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

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const { cssX, cssY } = canvasCoords(e);
    const { nx, ny } = toNatural(cssX, cssY);

    // 文字草稿进行中：点击别处 = 提交当前文字
    if (textDraftRef.current) {
      commitText();
      // 若仍是绘制工具且点在同处，下方会继续建新草稿
    }

    if (tool === "none") {
      const idx = hitTestAnnotationPrecise(nx, ny, annotations);
      if (idx != null) {
        dragRef.current = { idx, dx: nx - annotations[idx].x1, dy: ny - annotations[idx].y1 };
      }
      return;
    }

    if (tool === "text") {
      const d = { nx, ny, val: "" };
      textDraftRef.current = d;
      setTextDraft(d);
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

  // —— compose：图像 + 标注 合成到自然尺寸 PNG → base64（不含 data: 前缀）——
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

  // 文字草稿 textarea 的显示位置（自然 → CSS）
  const draftBox = (() => {
    if (!textDraft) return null;
    const { ox, oy, scale } = dispRef.current;
    return { left: ox + textDraft.nx * scale, top: oy + textDraft.ny * scale, fs: toolFontSize * scale };
  })();

  return (
    <div className="flex flex-col h-screen bg-neutral-900 select-none">
      <Toolbar
        tool={tool} setTool={setTool}
        toolColor={toolColor} setToolColor={setToolColorSync}
        toolWidth={toolWidth} setToolWidth={setToolWidthSync}
        toolFontSize={toolFontSize} setToolFontSize={setToolFontSizeSync}
        alwaysOnTop={alwaysOnTop} onToggleTop={toggleAlwaysOnTop}
        onSave={handleSave} onCopy={handleCopy} onOcr={handleOcr}
        onUndo={undo} canUndo={annotations.length > 0}
        ocrCopied={ocrCopied}
      />
      <div className="relative flex-1 overflow-hidden">
        <canvas
          ref={canvasRef}
          className="absolute inset-0 w-full h-full"
          style={{ cursor: tool === "none" ? "default" : "crosshair" }}
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
            autoFocus
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
            className="absolute bg-white/95 text-black outline-none resize-none px-1 py-0.5 rounded shadow"
            style={{ left: draftBox.left, top: draftBox.top, fontSize: draftBox.fs, minWidth: 120, lineHeight: 1.3 }}
          />
        )}
      </div>
    </div>
  );
}
