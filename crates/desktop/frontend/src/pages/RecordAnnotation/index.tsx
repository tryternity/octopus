/**
 * RecordAnnotation —— 录屏标注 overlay（完整版）。
 *
 * 录屏开始后显示在选区位置（overlay 尺寸 = 选区尺寸），用户可：
 * - 画 9 种标注（rect/oval/diamond/line/arrow/pen/text/number/blur）
 * - 颜色 / 线宽 / 字号 / 实心 fill 属性（复用截图 ToolPropsPopover）
 * - undo / redo
 * - A 键切换 标注 / 透传（透传时鼠标穿透到下层应用，让用户操作被录的应用）
 * - 顶部工具栏（含 录制边框、停止按钮）
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

  // ── 浮窗 / passthrough ──────────────────────────────────────
  const [showPopover, setShowPopover] = useState(false);
  const [popoverX, setPopoverX] = useState(0);
  const [passthrough, setPassthrough] = useState(false);
  const passthroughRef = useRef(false);

  // 同步 refs
  useEffect(() => { toolRef.current = tool; }, [tool]);
  useEffect(() => { annotationsRef.current = annotations; }, [annotations]);
  useEffect(() => { passthroughRef.current = passthrough; }, [passthrough]);
  useEffect(() => { numberCounterRef.current = numberCounter; }, [numberCounter]);

  // 工具栏实测宽度（浮窗 X clamp）
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [toolbarW, setToolbarW] = useState(0);
  useLayoutEffect(() => {
    if (toolbarRef.current) setToolbarW(toolbarRef.current.offsetWidth);
  }, [tool, passthrough]);

  const dpr = window.devicePixelRatio || 1;

  // ── Canvas 尺寸初始化（仅一次）────────────────────────────────
  const canvasInitedRef = useRef(false);
  useEffect(() => {
    if (canvasInitedRef.current) return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const cssW = window.innerWidth;
    const cssH = window.innerHeight;
    canvas.width = cssW * dpr;
    canvas.height = cssH * dpr;
    canvasInitedRef.current = true;
  }, [dpr]);

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
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const cssW = window.innerWidth;
    const cssH = window.innerHeight;
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

    // 录制区域边框（Canvas 整体边缘 2px 蓝色）
    ctx.strokeStyle = RECORD_BORDER_COLOR;
    ctx.lineWidth = 2;
    ctx.strokeRect(1, 1, cssW - 2, cssH - 2);
  }, [dpr, selectedAnn]);

  useEffect(() => { draw(); }, [draw, annotations, drawingVer]);

  // ── 工具选择：弹浮窗 + 记录按钮中心 x ────────────────────────
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

  // ── 鼠标交互（参考 Screenshot，去除选区逻辑）─────────────────
  function onMouseDown(e: React.MouseEvent) {
    if (passthroughRef.current) return; // 透传模式不画
    if (e.button !== 0) return;
    setShowPopover(false);
    const mx = e.clientX;
    const my = e.clientY;

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
    if (passthroughRef.current) return;
    const mx = e.clientX;
    const my = e.clientY;

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

  // ── 键盘：A 透传 / cmd+z undo / cmd+shift+z redo / Esc 退出工具 ─
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // 文字输入中不拦截
      if (textDraftRef.current) return;

      if (e.key === "a" || e.key === "A") {
        // 不在 textarea 里时 A 切透传
        const tgt = e.target as HTMLElement | null;
        if (tgt && (tgt.tagName === "TEXTAREA" || tgt.tagName === "INPUT")) return;
        e.preventDefault();
        const next = !passthroughRef.current;
        setPassthrough(next);
        invoke("set_annotation_passthrough", { passthrough: next }).catch(() => {});
        return;
      }
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
        if (toolRef.current !== "none") { setTool("none"); setShowPopover(false); }
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

  // ── 浮窗位置（工具栏下方居中，按 X clamp）──────────────────
  const popoverY = 8 + 44; // 工具栏 top 8 + 高 44
  const halfW = toolbarW / 2 || 150;
  const popoverLeft = Math.max(halfW + 8, Math.min(popoverX || window.innerWidth / 2, window.innerWidth - halfW - 8));

  return (
    <>
      <canvas
        ref={canvasRef}
        style={{
          position: "fixed",
          inset: 0,
          width: "100vw",
          height: "100vh",
          cursor: passthrough ? "default" : (tool === "none" ? "default" : "crosshair"),
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

      {/* 顶部工具栏 */}
      <div
        ref={toolbarRef}
        style={{
          position: "fixed",
          top: 8,
          left: "50%",
          transform: "translateX(-50%)",
          display: "flex",
          gap: 4,
          padding: "6px 8px",
          background: passthrough ? "rgba(26, 26, 30, 0.55)" : "var(--color-surface)",
          color: "var(--color-foreground)",
          borderRadius: 8,
          boxShadow: "0 4px 16px rgba(0,0,0,0.3)",
          zIndex: 100,
          alignItems: "center",
          opacity: passthrough ? 0.6 : 1,
          transition: "opacity 0.15s, background 0.15s",
          pointerEvents: passthrough ? "none" : "auto",
        }}
      >
        <ToolButton active={tool === "none"} onClick={() => { setTool("none"); setShowPopover(false); }} label={t("screenshot.tool.select")} icon={
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
        {/* 透传/标注 toggle（A 键）*/}
        <button
          onClick={() => {
            const next = !passthrough;
            setPassthrough(next);
            invoke("set_annotation_passthrough", { passthrough: next }).catch(() => {});
          }}
          title={passthrough ? t("screenshot.tool.select") : "Passthrough"}
          style={{
            width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center",
            borderRadius: 6, border: "none",
            background: passthrough ? "var(--color-voice)" : "transparent",
            cursor: "pointer", padding: 0,
          }}
        >
          <img src="icons/see-eye.svg" alt="" className="w-[18px] h-[18px]" style={{ filter: passthrough ? "brightness(0) invert(1)" : "var(--icon-filter)" }} />
        </button>
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
