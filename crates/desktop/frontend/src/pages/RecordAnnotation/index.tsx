/**
 * RecordAnnotation —— 录屏标注 overlay（完整版）。
 *
 * 录屏开始后显示在选区位置（overlay 尺寸 = 选区尺寸），用户可：
 * - 画 9 种标注（rect/oval/diamond/line/arrow/pen/text/number/blur）
 * - 颜色 / 线宽 / 字号 / 实心 fill 属性（复用截图 ToolPropsPopover）
 * - undo / redo
 * - 顶部工具栏（含 录制边框、停止按钮）
 *
 * 鼠标穿透（passthrough）现由后端 poller 管理（按鼠标位置实时切换 setIgnoresMouseEvents），
 * 前端不再持有 passthrough state。
 *
 * 与截图 Screenshot 的关键差异：
 * - **Canvas 透明背景**（不画暗遮罩）—— 录制时能看到下层应用正常操作。
 *   暗遮罩只在 AreaPicker 阶段有，overlay 阶段无。
 * - **无选区调整**（选区固定，不能 move/resize）。
 * - **无 OCR / 滚动 / 保存 / pin / confirm**（截图特有）。
 * - **无标注 resize 手柄**（标注画完固定）。
 *
 * 复用：
 * - `@/lib/annotation`：Annotation / Tool / drawAnnotation / hitTestAnnotationPrecise
 * - 截图 ToolButton / ToolPropsPopover
 * - 截图 index.tsx 的 mousedown/move/up 标注交互逻辑（参考不 copy）
 *
 * 停止录制：ESC 已被 record_hotkey 全局注册（调 stop_and_store 入库 + 关闭 overlay）。
 * 工具栏停止按钮：通过模拟 ESC keydown 不能触发全局快捷键（系统级），所以
 * 直接 emit `record://stop-requested` 事件——主进程在 setup 监听后调 stop_and_store。
 * 但目前主进程未监听此事件，所以停止按钮的 fallback 是 hide overlay + 提示用户按 ESC。
 * 这里采用最简方案：hide overlay 让用户从托盘 / 快捷键停止（与原 RecordAnnotation 行为一致，
 * 因为视频继续录，用户停止后 ESC 路径会正常关闭 overlay）。
 */
import { useEffect, useRef, useState, useCallback, useLayoutEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import { type Annotation, type Tool, drawAnnotation, annBounds, hitTestAnnotationPrecise } from "@/lib/annotation";
import { ToolButton } from "@/pages/Screenshot/ToolButton";
import { ToolPropsPopover } from "@/pages/Screenshot/ToolPropsPopover";
import { useT } from "@/lib/i18n";

const RECORD_BORDER_COLOR = "#3b82f6";

export default function RecordAnnotation() {
  const t = useT();
  const canvasRef = useRef<HTMLCanvasElement>(null);

  // ── 工具状态 ──────────────────────────────────────────────────
  const [tool, setTool] = useState<Tool>("none");
  const toolRef = useRef<Tool>("none");
  const [toolColor, setToolColorState] = useState("#ef4444");
  const toolColorRef = useRef("#ef4444");
  const [toolWidth, setToolWidth] = useState(3);
  const [toolFontSize, setToolFontSizeState] = useState(16);
  const toolFontSizeRef = useRef(16);
  const [toolFilled, setToolFilled] = useState(false);
  const toolFilledRef = useRef(false);
  const [toolCircleSize, setToolCircleSize] = useState(24);
  const setToolColor = (c: string) => { toolColorRef.current = c; setToolColorState(c); };
  const setToolFontSize = (s: number) => { toolFontSizeRef.current = s; setToolFontSizeState(s); };

  // ── 标注数据 ──────────────────────────────────────────────────
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const annotationsRef = useRef<Annotation[]>([]);
  const drawingRef = useRef<Annotation | null>(null);
  const [drawingVer, setDrawingVer] = useState(0); // 触发 pen 实时重绘
  const redoStackRef = useRef<Annotation[]>([]);
  const [redoAvailable, setRedoAvailable] = useState(false);
  const [numberCounter, setNumberCounter] = useState(1);
  const numberCounterRef = useRef(1);
  const [selectedAnn, setSelectedAnn] = useState<number | null>(null);
  const annMoveStartRef = useRef<{ idx: number; mx: number; my: number; anns: Annotation[] } | null>(null);

  // ── 文字输入草稿 ──────────────────────────────────────────────
  const [textDraft, setTextDraft] = useState<{ x: number; y: number; val: string } | null>(null);
  const textDraftRef = useRef<{ x: number; y: number; val: string } | null>(null);
  const textInputRef = useRef<HTMLTextAreaElement | null>(null);

  // ── 浮窗 ──────────────────────────────────────────────────────
  const [showPopover, setShowPopover] = useState(false);
  const [popoverX, setPopoverX] = useState(0);

  // ── Canvas/工具栏几何（后端注入：窗口=选区+工具栏空间）────────
  // 后端 record_annotation_window.rs 创建的 overlay 窗口比选区大，
  // URL 注入 canvas_ox/oy/w/h 描述选区在窗口内的位置（逻辑像素）。
  const [canvasRect, setCanvasRect] = useState({ ox: 0, oy: 0, w: 0, h: 0 });
  const [toolbarPos, setToolbarPos] = useState<"below" | "above" | "inside">("inside");
  const canvasRectRef = useRef(canvasRect);
  useEffect(() => { canvasRectRef.current = canvasRect; }, [canvasRect]);

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    setCanvasRect({
      ox: parseFloat(params.get("canvas_ox") || "0"),
      oy: parseFloat(params.get("canvas_oy") || "0"),
      w: parseFloat(params.get("canvas_w") || String(window.innerWidth)),
      h: parseFloat(params.get("canvas_h") || String(window.innerHeight)),
    });
    setToolbarPos((params.get("toolbar") || "inside") as "below" | "above" | "inside");
  }, []);

  // 同步 refs
  useEffect(() => { toolRef.current = tool; }, [tool]);
  useEffect(() => { annotationsRef.current = annotations; }, [annotations]);
  useEffect(() => { numberCounterRef.current = numberCounter; }, [numberCounter]);

  // mount 时默认穿透（tool="none" = 鼠标模式 = 穿透操作下层应用）
  useEffect(() => {
    invoke("set_annotation_passthrough", { passthrough: true }).catch(() => {});
  }, []);

  // 工具栏实测宽度（浮窗 X clamp）
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [toolbarW, setToolbarW] = useState(0);
  useLayoutEffect(() => {
    if (toolbarRef.current) setToolbarW(toolbarRef.current.offsetWidth);
  }, [tool]);

  const dpr = window.devicePixelRatio || 1;

  // ── Canvas 尺寸初始化（绑定 canvasRect）────────────────────────
  // 后端窗口 = 选区 + 工具栏空间；Canvas 只占选区那部分（canvasRect）。
  const canvasInitedRef = useRef(false);
  useEffect(() => {
    const { w, h } = canvasRect;
    if (w === 0 || h === 0) return;  // URL 还没解析
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    canvasInitedRef.current = true;
  }, [dpr, canvasRect]);

  // ── undo / redo / add ────────────────────────────────────────
  const addAnnotation = (ann: Annotation) => {
    redoStackRef.current = [];
    setRedoAvailable(false);
    setAnnotations((prev) => [...prev, ann]);
  };
  const undoAnnotation = () => {
    setAnnotations((prev) => {
      if (prev.length === 0) return prev;
      const removed = prev[prev.length - 1];
      redoStackRef.current.push(removed);
      setRedoAvailable(true);
      if (removed.type === "number" && removed.number === numberCounterRef.current - 1) {
        setNumberCounter(numberCounterRef.current - 1);
      }
      return prev.slice(0, -1);
    });
    setSelectedAnn(null);
  };
  const redoAnnotation = () => {
    const ann = redoStackRef.current.pop();
    if (ann) {
      if (ann.type === "number") setNumberCounter(numberCounterRef.current + 1);
      setAnnotations((prev) => [...prev, ann]);
      setRedoAvailable(redoStackRef.current.length > 0);
    }
  };

  // ── 绘制（透明 Canvas + 录制边框 + 标注）─────────────────────
  // 标注坐标是相对 Canvas 的（与 canvasRect 对应），Canvas 通过 CSS
  // position 在窗口内偏移。绘制时 transform 已让 (0,0) 对齐 Canvas 左上角，
  // 因此标注坐标无需再加 canvasRect 偏移；录制边框画在 Canvas 自身边缘。
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const cssW = canvasRect.w;
    const cssH = canvasRect.h;
    if (cssW === 0 || cssH === 0) return;
    if (canvas.width !== cssW * dpr || canvas.height !== cssH * dpr) {
      canvas.width = cssW * dpr;
      canvas.height = cssH * dpr;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);

    // 标注
    const anns = annotationsRef.current;
    for (let i = 0; i < anns.length; i++) {
      // 编辑中的文字标注（text=""）跳过——textarea 已显示
      if (anns[i].type === "text" && (anns[i].text ?? "") === "") continue;
      drawAnnotation(ctx, anns[i]);
      if (selectedAnn === i) {
        const b = annBounds(anns[i]);
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

    // 录制区域边框（Canvas 自身边缘 2px 蓝色）
    ctx.strokeStyle = RECORD_BORDER_COLOR;
    ctx.lineWidth = 2;
    ctx.strokeRect(1, 1, cssW - 2, cssH - 2);
  }, [dpr, selectedAnn, canvasRect]);

  useEffect(() => { draw(); }, [draw, annotations, drawingVer]);

  // ── 工具选择：弹浮窗 + 记录按钮中心 x ────────────────────────
  const onToolSelect = (e: React.MouseEvent, t: Tool, extra?: () => void) => {
    const btn = e.currentTarget as HTMLElement;
    const rect = btn.getBoundingClientRect();
    setPopoverX(rect.left + rect.width / 2);
    if (tool === t) {
      if (showPopover) { setShowPopover(false); setTool("none"); invoke("set_annotation_passthrough", { passthrough: true }).catch(() => {}); }
      else { setShowPopover(true); }
    } else {
      setTool(t); setShowPopover(true);
      // 选标注工具 → 不穿透（画标注）；选 "none"（鼠标）→ 穿透（操作下层应用）
      invoke("set_annotation_passthrough", { passthrough: t === "none" }).catch(() => {});
      extra?.();
    }
  };

  // ── 鼠标交互（参考 Screenshot，去除选区逻辑）─────────────────
  // e.clientX/Y 是窗口坐标，标注坐标是相对 Canvas（选区）的——
  // 减去 canvasRect.ox/oy 转换到 Canvas 局部坐标系。
  function onMouseDown(e: React.MouseEvent) {
    if (e.button !== 0) return;
    setShowPopover(false);
    const mx = e.clientX - canvasRectRef.current.ox;
    const my = e.clientY - canvasRectRef.current.oy;

    // 文字输入中：先确认当前文字
    if (textDraftRef.current) {
      const draft = textDraftRef.current;
      if (draft.val.trim()) {
        addAnnotation({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: toolColorRef.current, fontSize: toolFontSizeRef.current, textWidth: 200 });
      }
      textDraftRef.current = null;
      setTextDraft(null);
      // 若点击的是文字工具，开新输入
      if (toolRef.current === "text") {
        setTextDraft({ x: mx, y: my, val: "" });
        textDraftRef.current = { x: mx, y: my, val: "" };
        setTimeout(() => textInputRef.current?.focus(), 10);
      }
      return;
    }

    // 选择工具 + 命中已有标注：选中并准备拖动
    if (toolRef.current === "none") {
      const idx = hitTestAnnotationPrecise(mx, my, annotationsRef.current);
      if (idx !== null) {
        setSelectedAnn(idx);
        annMoveStartRef.current = { idx, mx, my, anns: [...annotationsRef.current] };
        return;
      }
      setSelectedAnn(null);
      return;
    }

    // 标注工具激活：开始绘制
    const tk = toolRef.current;
    if (tk === "text") {
      setTextDraft({ x: mx, y: my, val: "" });
      textDraftRef.current = { x: mx, y: my, val: "" };
      setTimeout(() => textInputRef.current?.focus(), 10);
      return;
    }
    if (tk === "number") {
      addAnnotation({
        type: "number", x1: mx, y1: my, x2: mx, y2: my,
        number: numberCounterRef.current, color: toolColorRef.current, circleSize: toolCircleSize,
      });
      setNumberCounter(numberCounterRef.current + 1);
      return;
    }
    if (tk === "pen") {
      drawingRef.current = { type: "pen", x1: mx, y1: my, x2: mx, y2: my, points: [[mx, my]], color: toolColorRef.current, lineWidth: toolWidth };
    } else {
      drawingRef.current = {
        type: tk, x1: mx, y1: my, x2: mx, y2: my,
        color: toolColorRef.current, lineWidth: toolWidth,
        filled: (tk === "rect" || tk === "oval" || tk === "diamond") ? toolFilledRef.current : undefined,
      };
    }
  }

  function onMouseMove(e: React.MouseEvent) {
    const mx = e.clientX - canvasRectRef.current.ox;
    const my = e.clientY - canvasRectRef.current.oy;

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
      const next = [...anns];
      next[idx] = moved;
      setAnnotations(next);
      return;
    }

    // 标注绘制中
    if (drawingRef.current && toolRef.current !== "none") {
      if (drawingRef.current.type === "pen" && drawingRef.current.points) {
        drawingRef.current.points.push([mx, my]);
      } else {
        drawingRef.current = { ...drawingRef.current, x2: mx, y2: my };
      }
      setDrawingVer((v) => v + 1);
      return;
    }

    // 悬停光标
    const c = e.currentTarget as HTMLCanvasElement;
    if (toolRef.current === "none") {
      const hit = hitTestAnnotationPrecise(mx, my, annotationsRef.current);
      c.style.cursor = hit !== null ? "move" : "default";
    } else {
      c.style.cursor = "crosshair";
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
      if (ann.type === "rect" || ann.type === "oval" || ann.type === "diamond") {
        if (Math.abs(ann.x2 - ann.x1) > 5 && Math.abs(ann.y2 - ann.y1) > 5) { addAnnotation(ann); added = true; }
      } else if (ann.type === "line" || ann.type === "arrow") {
        const dx = ann.x2 - ann.x1, dy = ann.y2 - ann.y1;
        if (Math.sqrt(dx * dx + dy * dy) > 10) { addAnnotation(ann); added = true; }
      } else if (ann.type === "pen" && ann.points) {
        if (ann.points.length > 2) { addAnnotation(ann); added = true; }
      } else if (ann.type === "blur") {
        if (Math.abs(ann.x2 - ann.x1) > 5 && Math.abs(ann.y2 - ann.y1) > 5) { addAnnotation(ann); added = true; }
      }
      if (!added) setDrawingVer((v) => v + 1);
    }
  }

  // ── 键盘：cmd+z undo / cmd+shift+z redo / Esc 退出工具 ───────
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // 文字输入中不拦截
      if (textDraftRef.current) return;

      if ((e.metaKey || e.ctrlKey) && e.shiftKey && (e.key === "z" || e.key === "Z")) {
        e.preventDefault();
        redoAnnotation();
        return;
      }
      if ((e.metaKey || e.ctrlKey) && (e.key === "z" || e.key === "Z")) {
        e.preventDefault();
        undoAnnotation();
        return;
      }
      if ((e.key === "Delete" || e.key === "Backspace") && selectedAnn !== null) {
        const removed = annotationsRef.current[selectedAnn];
        if (removed?.type === "number" && removed.number === numberCounterRef.current - 1) {
          setNumberCounter(numberCounterRef.current - 1);
        }
        setAnnotations(annotationsRef.current.filter((_, i) => i !== selectedAnn));
        setSelectedAnn(null);
        return;
      }
      if (e.key === "Escape") {
        if (toolRef.current !== "none") {
          setTool("none"); setShowPopover(false);
          invoke("set_annotation_passthrough", { passthrough: true }).catch(() => {});
        }
        // 录屏停止由全局 ESC 快捷键接管（record_hotkey），这里不重复处理
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selectedAnn]);

  // ── 停止录制按钮：emit 事件让主进程停止（主进程未监听时 fallback hide overlay）──
  const onStopClick = async () => {
    // 主进程 stop_and_store 已由 ESC 全局快捷键 / 托盘菜单路径统一处理。
    // 这里 emit 一个事件，未来主进程可监听；目前先 hide overlay（让出屏幕给用户操作）。
    try {
      await emit("record://stop-requested", { from: "annotation" });
    } catch {
      // ignore
    }
    try {
      await getCurrentWindow().hide();
    } catch {
      // ignore
    }
  };

  // ── 工具栏位置（按 toolbarPos + canvasRect 计算）──────────────
  // 后端窗口 = 选区 + 工具栏空间，工具栏放在 Canvas（选区）外的预留空间内：
  //   toolbar=below：Canvas 下方（canvas_oy=0，工具栏在 Canvas 底部下方）
  //   toolbar=above：Canvas 上方（canvas_oy=工具栏空间，工具栏在 Canvas 顶部上方）
  //   toolbar=inside：兜底，工具栏浮在 Canvas 内部底部
  const TOOLBAR_H = 44;
  const toolbarTop = toolbarPos === "below"
    ? canvasRect.oy + canvasRect.h + 8        // Canvas 下方
    : toolbarPos === "above"
      ? 8                                      // 窗口顶部（Canvas 上方）
      : canvasRect.oy + canvasRect.h - TOOLBAR_H - 8;  // Canvas 内部底部

  // popover 位置：below → 工具栏下方；above / inside → 工具栏上方
  const popoverY = toolbarPos === "below"
    ? toolbarTop + TOOLBAR_H
    : Math.max(0, toolbarTop - 200);
  // popover X：跟随被点击的工具按钮中心（与截图 onToolSelect setPopoverX 一致）
  const popoverLeft = popoverX || (canvasRect.ox + canvasRect.w / 2);

  // 工具栏 X clamp（基于 canvasRect 水平区间，DOCK_MARGIN 留边）
  const DOCK_MARGIN = 80;
  const halfW = toolbarW / 2 || 150;
  const toolbarCenterX = Math.max(
    canvasRect.ox + DOCK_MARGIN + halfW,
    Math.min(
      canvasRect.ox + canvasRect.w / 2,
      canvasRect.ox + canvasRect.w - DOCK_MARGIN - halfW,
    ),
  );

  return (
    <>
      <canvas
        ref={canvasRef}
        style={{
          position: "fixed",
          left: canvasRect.ox,
          top: canvasRect.oy,
          width: canvasRect.w,
          height: canvasRect.h,
          cursor: tool === "none" ? "default" : "crosshair",
          display: "block",
        }}
        onMouseDown={onMouseDown}
        onMouseMove={onMouseMove}
        onMouseUp={onMouseUp}
      />

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
            if (draft && draft.val.trim()) {
              addAnnotation({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: toolColorRef.current, fontSize: toolFontSizeRef.current, textWidth: 200 });
            }
            textDraftRef.current = null;
            setTextDraft(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              textDraftRef.current = null;
              setTextDraft(null);
            }
            e.stopPropagation();
          }}
          style={{
            position: "fixed",
            left: textDraft.x + canvasRect.ox,
            top: textDraft.y + canvasRect.oy,
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

      {/* 顶部工具栏 */}
      <div
        ref={toolbarRef}
        style={{
          position: "fixed",
          top: toolbarTop,
          left: toolbarCenterX,
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
        <ToolButton active={tool === "none"} onClick={() => { setTool("none"); setShowPopover(false); invoke("set_annotation_passthrough", { passthrough: true }).catch(() => {}); }} label={t("screenshot.tool.select")} icon={
          <img src="icons/arrow-pointer.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "none" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <ToolButton active={tool === "rect"} onClick={(e) => onToolSelect(e, "rect")} label={t("screenshot.tool.rect")} icon={
          <img src="icons/square.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "rect" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <ToolButton active={tool === "oval"} onClick={(e) => onToolSelect(e, "oval")} label={t("screenshot.tool.ellipse")} icon={
          <img src="icons/oval-vertical.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "oval" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <ToolButton active={tool === "diamond"} onClick={(e) => onToolSelect(e, "diamond")} label={t("screenshot.tool.diamond")} icon={
          <img src="icons/diamond.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "diamond" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <ToolButton active={tool === "line"} onClick={(e) => onToolSelect(e, "line")} label={t("screenshot.tool.line")} icon={
          <img src="icons/straight-line.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "line" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <ToolButton active={tool === "arrow"} onClick={(e) => onToolSelect(e, "arrow")} label={t("screenshot.tool.arrow")} icon={
          <img src="icons/arrow-line.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "arrow" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <ToolButton active={tool === "pen"} onClick={(e) => onToolSelect(e, "pen")} label={t("screenshot.tool.pen")} icon={
          <img src="icons/sketching.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "pen" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <ToolButton active={tool === "text"} onClick={(e) => onToolSelect(e, "text")} label={t("screenshot.tool.text")} icon={
          <img src="icons/text.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "text" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <ToolButton active={tool === "number"} onClick={(e) => onToolSelect(e, "number", () => setNumberCounter(1))} label={t("screenshot.tool.number")} icon={
          <img src="icons/sequence-note.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "number" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <ToolButton active={tool === "blur"} onClick={(e) => onToolSelect(e, "blur")} label={t("screenshot.tool.mosaic")} icon={
          <img src="icons/mosaic.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: tool === "blur" ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        } />
        <div style={{ width: 1, height: 20, background: "var(--color-border)", margin: "0 4px" }} />
        <ToolButton onClick={undoAnnotation} label={t("screenshot.tool.undo")} icon={
          <img src="icons/restore.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)", opacity: annotations.length > 0 ? 1 : 0.3 }} />
        } />
        <ToolButton onClick={redoAnnotation} label={t("screenshot.tool.redo")} icon={
          <img src="icons/redo.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: "var(--icon-filter)", opacity: redoAvailable ? 1 : 0.3 }} />
        } />
        <div style={{ width: 1, height: 20, background: "var(--color-border)", margin: "0 4px" }} />
        {/* 停止录制（红色）*/}
        <button
          onClick={onStopClick}
          title={t("tray.recordStop")}
          style={{
            width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center",
            borderRadius: 6, border: "none",
            background: "#dc2626",
            cursor: "pointer", padding: 0, marginLeft: 2,
          }}
        >
          {/* 红色实心圆点（停止录制标准图标）*/}
          <span style={{ width: 10, height: 10, borderRadius: "50%", background: "#fff", display: "block" }} />
        </button>
      </div>

      {/* 工具属性浮窗 */}
      {tool !== "none" && showPopover && (
        <ToolPropsPopover
          x={popoverLeft}
          y={popoverY}
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
    </>
  );
}
