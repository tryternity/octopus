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
import { useEffect, useRef, useState, useLayoutEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit, listen as rawListen, type UnlistenFn, type Event } from "@tauri-apps/api/event";
import { type Annotation, drawAnnotation, annBounds, hitTestAnnotationPrecise } from "@/lib/annotation";
import { useAnnotationState, AnnotationToolbar, computeToolbarPosition, computeToolbarCenterX, TOOLBAR_H } from "@/components/Annotation";
import { useT } from "@/lib/i18n";

const RECORD_BORDER_COLOR = "#3b82f6";

/** helper event payload（监听 pause/resume/stop 更新录制状态）*/
interface HelperEventLite {
  event: "recording-started" | "recording-paused" | "recording-resumed" | "recording-stopped" | "ready" | "warning" | "error";
}

function formatDuration(secs: number): string {
  const totalSec = Math.max(0, Math.floor(secs));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  if (h > 0) return `${h}:${pad(m)}:${pad(s)}`;
  return `${pad(m)}:${pad(s)}`;
}

export default function RecordAnnotation() {
  const t = useT();
  const canvasRef = useRef<HTMLCanvasElement>(null);

  // ── 标注状态（hook 抽取，与 Screenshot 共用）──────────────────
  const annotation = useAnnotationState();
  const {
    toolRef, annotationsRef, drawingRef, drawingVer, setDrawingVer,
    addAnnotation, undoAnnotation, redoAnnotation,
    numberCounterRef, setSelectedAnn,
    setAnnotations,
  } = annotation;
  const annMoveStartRef = useRef<{ idx: number; mx: number; my: number; anns: Annotation[] } | null>(null);

  // ── 文字输入草稿（业务侧独有，不进 hook）──────────────────────
  const [textDraft, setTextDraft] = useState<{ x: number; y: number; val: string } | null>(null);
  const textDraftRef = useRef<{ x: number; y: number; val: string } | null>(null);
  const textInputRef = useRef<HTMLTextAreaElement | null>(null);

  // ── Canvas/工具栏几何（后端注入：窗口=选区+工具栏空间）────────
  // 后端 record_annotation_window.rs 创建的 overlay 窗口比选区大，
  // URL 注入 canvas_ox/oy/w/h 描述选区在窗口内的位置（逻辑像素）。
  // toolbar_pos 决定 TOOLBAR_ZONE（后端 poller 据此判定鼠标穿透），前端必须与之对齐。
  const [canvasRect, setCanvasRect] = useState({ ox: 0, oy: 0, w: 0, h: 0 });
  // ── 录制时长显示（与 RecordControl 同模式：mount 查 get_record_status + 监听事件）──
  // RecordAnnotation overlay 创建晚于 recording-started 事件，本地直接 setInterval。
  type RecState = "idle" | "recording" | "paused";
  const [recState, setRecState] = useState<RecState>("recording"); // overlay 只在录制中创建
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    let cancelled = false;
    invoke<{ state: string; elapsed_secs: number }>("get_record_status")
      .then((status) => {
        if (cancelled) return;
        setRecState(status.state === "paused" ? "paused" : status.state === "recording" ? "recording" : "idle");
        setElapsed(status.elapsed_secs);
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, []);
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    rawListen<HelperEventLite>("record://event", (e: Event<HelperEventLite>) => {
      const evt = e.payload.event;
      if (evt === "recording-paused") setRecState("paused");
      else if (evt === "recording-resumed") setRecState("recording");
      else if (evt === "recording-stopped") setRecState("idle");
    }).then((fn) => { if (cancelled) fn(); else unlisten = fn; });
    return () => { cancelled = true; unlisten?.(); };
  }, []);
  useEffect(() => {
    if (recState !== "recording") return;
    const timer = setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => clearInterval(timer);
  }, [recState]);
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
  }, []);

  // mount 时默认穿透（tool="none" = 鼠标模式 = 穿透操作下层应用）
  useEffect(() => {
    invoke("set_annotation_passthrough", { passthrough: true }).catch(() => {});
  }, []);

  // 工具栏实测宽度（浮窗 X clamp）
  const toolbarRef = useRef<HTMLDivElement>(null);
  const [toolbarW, setToolbarW] = useState(0);
  useLayoutEffect(() => {
    if (toolbarRef.current) setToolbarW(toolbarRef.current.offsetWidth);
  }, [annotation.tool]);

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

  // add/undo/redo 已抽到 useAnnotationState（与 Screenshot 共用）

  // ── 绘制（透明 Canvas + 录制边框 + 标注）─────────────────────
  // 标注坐标是相对 Canvas 的（与 canvasRect 对应），Canvas 通过 CSS
  // position 在窗口内偏移。绘制时 transform 已让 (0,0) 对齐 Canvas 左上角，
  // 因此标注坐标无需再加 canvasRect 偏移；录制边框画在 Canvas 自身边缘。
  const draw = () => {
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
      if (annotation.selectedAnn === i) {
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
  };

  // 重绘触发：标注变化、绘制中、canvasRect 变化（URL 解析后首次拿到选区几何）。
  // 原依赖列表只有 [annotations, drawingVer]——漏了 canvasRect，导致 URL 解析填充
  // canvasRect 后不重绘，录制边框要等到首次标注操作才出现（bug：录屏开始无边框）。
  useEffect(() => { draw(); }, [annotation.annotations, drawingVer, canvasRect]);


  // ── 鼠标交互（参考 Screenshot，去除选区逻辑）─────────────────
  // e.clientX/Y 是窗口坐标，标注坐标是相对 Canvas（选区）的——
  // 减去 canvasRect.ox/oy 转换到 Canvas 局部坐标系。
  function onMouseDown(e: React.MouseEvent) {
    if (e.button !== 0) return;
    annotation.setShowPopover(false);
    const mx = e.clientX - canvasRectRef.current.ox;
    const my = e.clientY - canvasRectRef.current.oy;

    // 文字输入中：先确认当前文字
    if (textDraftRef.current) {
      const draft = textDraftRef.current;
      if (draft.val.trim()) {
        addAnnotation({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: annotation.toolColorRef.current, fontSize: annotation.toolFontSizeRef.current, textWidth: 200 });
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
    // eraser：mousedown 即开始擦除（划过即删）
    if (tk === "eraser") {
      annotation.eraseAnnotationAt(mx, my);
      return;
    }
    if (tk === "text") {
      setTextDraft({ x: mx, y: my, val: "" });
      textDraftRef.current = { x: mx, y: my, val: "" };
      setTimeout(() => textInputRef.current?.focus(), 10);
      return;
    }
    if (tk === "number") {
      addAnnotation({
        type: "number", x1: mx, y1: my, x2: mx, y2: my,
        number: numberCounterRef.current, color: annotation.toolColorRef.current, circleSize: annotation.toolCircleSize,
      });
      annotation.setNumberCounter(numberCounterRef.current + 1);
      return;
    }
    if (tk === "pen") {
      drawingRef.current = { type: "pen", x1: mx, y1: my, x2: mx, y2: my, points: [[mx, my]], color: annotation.toolColorRef.current, lineWidth: annotation.toolWidth };
    } else {
      drawingRef.current = {
        type: tk, x1: mx, y1: my, x2: mx, y2: my,
        color: annotation.toolColorRef.current, lineWidth: annotation.toolWidth,
        filled: (tk === "rect" || tk === "oval" || tk === "diamond") ? annotation.toolFilledRef.current : undefined,
      };
    }
  }

  function onMouseMove(e: React.MouseEvent) {
    const mx = e.clientX - canvasRectRef.current.ox;
    const my = e.clientY - canvasRectRef.current.oy;

    // eraser：按住左键拖动时擦除（划过即删）
    if (toolRef.current === "eraser" && (e.buttons & 1)) {
      annotation.eraseAnnotationAt(mx, my);
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
      if ((e.key === "Delete" || e.key === "Backspace") && annotation.selectedAnn !== null) {
        const removed = annotationsRef.current[annotation.selectedAnn];
        if (removed?.type === "number" && removed.number === numberCounterRef.current - 1) {
          annotation.setNumberCounter(numberCounterRef.current - 1);
        }
        setAnnotations(annotationsRef.current.filter((_, i) => i !== annotation.selectedAnn));
        setSelectedAnn(null);
        return;
      }
      if (e.key === "Escape") {
        if (toolRef.current !== "none") {
          annotation.setTool("none"); annotation.setShowPopover(false);
          invoke("set_annotation_passthrough", { passthrough: true }).catch(() => {});
        }
        // 录屏停止由全局 ESC 快捷键接管（record_hotkey），这里不重复处理
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [annotation.selectedAnn]);

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

  // ── 工具栏位置（与截图同算法 computeToolbarPosition）──
  // 全屏窗口模式：窗口覆盖整个显示器，Canvas 通过 fixed 定位到选区，
  // 工具栏用 computeToolbarPosition 算位置（与截图完全一致）。
  const tbPos = canvasRect.w > 0
    ? computeToolbarPosition(
        { x: canvasRect.ox, y: canvasRect.oy, w: canvasRect.w, h: canvasRect.h },
        window.innerHeight,
      )
    : null;
  const toolbarTop = tbPos ? tbPos.y : 0;
  // popover Y：below/above → 工具栏下方；inside → 工具栏上方
  const popoverY = tbPos?.belowOrAbove
    ? toolbarTop + TOOLBAR_H
    : Math.max(0, toolbarTop - 200);

  // popover X：跟随被点击的工具按钮中心，fallback 到选区中心
  const popoverLeft = annotation.popoverX || (canvasRect.ox + canvasRect.w / 2);

  // 工具栏 X clamp（与截图同算法 computeToolbarCenterX）
  const toolbarCenterX = canvasRect.w > 0
    ? computeToolbarCenterX(
        { x: canvasRect.ox, y: canvasRect.oy, w: canvasRect.w, h: canvasRect.h },
        window.innerWidth,
        toolbarW,
      )
    : 0;

  // mount 后把工具栏实际位置传回后端（poller 用此区域判定穿透）
  useEffect(() => {
    if (canvasRect.w === 0 || !tbPos) return;
    invoke("set_toolbar_zone", {
      x: toolbarCenterX - (toolbarW || 200) / 2,
      y: toolbarTop,
      w: toolbarW || 200,
      h: TOOLBAR_H,
    }).catch(() => {});
  }, [canvasRect.w, canvasRect.ox, canvasRect.oy, canvasRect.h, toolbarTop, toolbarCenterX, toolbarW, tbPos]);

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
          cursor: annotation.tool === "none" ? "default" : "crosshair",
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
              addAnnotation({ type: "text", x1: draft.x, y1: draft.y, x2: draft.x, y2: draft.y, text: draft.val, color: annotation.toolColorRef.current, fontSize: annotation.toolFontSizeRef.current, textWidth: 200 });
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
            fontSize: annotation.toolFontSize,
            color: annotation.toolColor,
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

      {/* 工具栏（AnnotationToolbar 渲染 9 工具 + undo/redo + children slot） */}
      <AnnotationToolbar
        state={annotation}
        toolbarRef={toolbarRef}
        top={toolbarTop}
        left={toolbarCenterX}
        popoverY={popoverY}
        popoverX={popoverLeft}
        showHighlight={false}
        onToolChange={(target) => {
          // 选标注工具 → 不穿透（画标注）；选 "none"（鼠标）→ 穿透（操作下层应用）
          invoke("set_annotation_passthrough", { passthrough: target === "none" }).catch(() => {});
        }}
      >
        {/* divider + 录制时长 + 停止录制（红色）—— 业务侧独有 */}
        <div style={{ width: 1, height: 20, background: "var(--color-border)", margin: "0 4px" }} />
        {/* 录制时长 mm:ss（等宽数字防跳；红点 pulse 提示录制中）*/}
        <div style={{ display: "flex", alignItems: "center", gap: 4, padding: "0 6px" }}>
          <div style={{
            width: 6, height: 6, borderRadius: "50%",
            background: recState === "paused" ? "rgba(255,255,255,0.4)" : "#dc2626",
            animation: recState === "recording" ? "pulse 1.5s ease-in-out infinite" : "none",
          }} />
          <span style={{
            fontSize: 11, fontWeight: 600, color: "var(--color-foreground)",
            fontFamily: "SF Mono, Menlo, monospace",
            fontVariantNumeric: "tabular-nums",
          }}>
            {formatDuration(elapsed)}
          </span>
          <style>{`@keyframes pulse { 0%, 100% { opacity: 1; transform: scale(1); } 50% { opacity: 0.5; transform: scale(0.85); } }`}</style>
        </div>
        {/* 暂停/继续录制按钮（与 RecordControl 浮窗同范式）*/}
        <button
          onClick={() => {
            // recState 是本地状态（从 record://event 同步），调 record_pause/resume
            if (recState === "recording") invoke("record_pause").catch(() => {});
            else if (recState === "paused") invoke("record_resume").catch(() => {});
          }}
          title={
            recState === "recording"
              ? t("settings.recordings.pauseBtn")
              : t("settings.recordings.resumeBtn")
          }
          style={{
            width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center",
            borderRadius: 6, border: "1px solid var(--color-border)",
            background: "transparent", color: "var(--color-foreground)",
            cursor: "pointer", padding: 0,
          }}
        >
          {recState === "recording" ? (
            // 暂停图标（两竖）
            <svg width="10" height="12" viewBox="0 0 10 12" fill="currentColor">
              <rect x="0" y="0" width="3" height="12" rx="1" />
              <rect x="7" y="0" width="3" height="12" rx="1" />
            </svg>
          ) : (
            // 继续图标（三角）
            <svg width="10" height="12" viewBox="0 0 10 12" fill="currentColor">
              <path d="M0 0 L10 6 L0 12 Z" />
            </svg>
          )}
        </button>
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
      </AnnotationToolbar>
    </>
  );
}
