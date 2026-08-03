// 标注鼠标交互 hook —— 统一 ImagePreview/Screenshot/RecordAnnotation 的标注鼠标逻辑。
// 在 useAnnotationState 基础上叠加：mousedown/move/up 交互 + 坐标换算 + 文字 draft。
//
// 各场景只需提供 clientToNatural 坐标函数 + useAnnotationState 返回值。
// 平移（pan）/选区（crop）/窗口管理/全图加载拦截不纳入 hook（各场景自己处理）。
//
// ⚠️ 迁移进度（2026-08-03）：仅 ImagePreview 已接入。Screenshot 和 RecordAnnotation
// 仍用各自内联的鼠标交互逻辑，未完成「统一三场景」目标。遗留迁移记录在
// docs/superpowers/plans/2026-07-30-annotation-interaction-unification.md Task 3/4。

import { useState, useRef, useCallback } from "react";
import type { Annotation, Tool } from "@/lib/annotation";
import { hitTestAnnotationPrecise } from "@/lib/annotation";
import type { AnnotationState } from "./useAnnotationState";

/** 坐标换算：屏幕 clientX/Y → 标注自然坐标。各场景提供。 */
export type ClientToNatural = (clientX: number, clientY: number) => { x: number; y: number };

/** mousedown 时传入的工具上下文 */
export interface ToolContext {
  tool: Tool;
  color: string;
  width: number;
  fontSize: number;
  filled: boolean;
}

/** 文字标注草稿 */
export interface TextDraft {
  x: number;
  y: number;
  val: string;
  fs: number;
}

export interface UseAnnotationInteractionOptions {
  clientToNatural: ClientToNatural;
  natW: number;
  natH: number;
  state: AnnotationState;
}

export interface AnnotationInteraction {
  /** 绘制中的临时标注（SVG overlay 渲染用） */
  draftAnn: Annotation | null;
  /** 文字标注草稿 */
  textDraft: TextDraft | null;
  textDraftRef: React.MutableRefObject<TextDraft | null>;
  /** mousedown：标注创建/拖拽/擦除/文字。返回是否命中标注或进入了标注操作。 */
  handleMouseDown: (e: React.MouseEvent, ctx: ToolContext) => void;
  /** mousemove：绘制中/拖拽中/擦除中 */
  handleMouseMove: (e: React.MouseEvent) => void;
  /** mouseup：结束当前操作 */
  handleMouseUp: () => void;
  /** 提交文字草稿 */
  commitText: (color: string, fontSize: number) => void;
  /** 取消文字草稿 */
  cancelText: () => void;
  /** 设置文字草稿值（textarea onChange 用） */
  setTextDraftVal: (val: string) => void;
  /** 擦除中 ref（各场景判断 cursor 用） */
  erasingRef: React.MutableRefObject<boolean>;
  /** 拖拽中 ref（各场景判断 cursor 用） */
  dragRef: React.MutableRefObject<{ idx: number; dx: number; dy: number } | null>;
}

export function useAnnotationInteraction(opts: UseAnnotationInteractionOptions): AnnotationInteraction {
  const { clientToNatural, state } = opts;
  const {
    annotationsRef, drawingRef, addAnnotation,
    eraseAnnotationAt, numberCounter, setNumberCounter, setAnnotations,
  } = state;

  const [draftAnn, setDraftAnn] = useState<Annotation | null>(null);
  const [textDraft, setTextDraft] = useState<TextDraft | null>(null);
  const textDraftRef = useRef<TextDraft | null>(null);
  const erasingRef = useRef(false);
  const dragRef = useRef<{ idx: number; dx: number; dy: number } | null>(null);

  // clientToNatural 用 ref 镜像（闭包安全）
  const ctRef = useRef(clientToNatural);
  ctRef.current = clientToNatural;

  const commitText = useCallback((color: string, fontSize: number) => {
    const d = textDraftRef.current;
    textDraftRef.current = null;
    setTextDraft(null);
    if (!d || !d.val.trim()) return;
    addAnnotation({
      type: "text",
      x1: d.x, y1: d.y, x2: d.x, y2: d.y,
      text: d.val,
      color, fontSize,
    });
  }, [addAnnotation]);

  const handleMouseDown = useCallback((e: React.MouseEvent, ctx: ToolContext) => {
    if (e.button !== 0) return;
    const { x: nx, y: ny } = ctRef.current(e.clientX, e.clientY);

    // 文字草稿进行中：点击别处 = 提交当前文字
    if (textDraftRef.current) {
      commitText(ctx.color, ctx.fontSize);
    }

    // 橡皮擦
    if (ctx.tool === "eraser") {
      eraseAnnotationAt(nx, ny);
      erasingRef.current = true;
      return;
    }

    // 选择工具：hitTest → 拖拽
    if (ctx.tool === "none") {
      const idx = hitTestAnnotationPrecise(nx, ny, annotationsRef.current);
      if (idx != null) {
        dragRef.current = {
          idx,
          dx: nx - annotationsRef.current[idx].x1,
          dy: ny - annotationsRef.current[idx].y1,
        };
      }
      return;
    }

    // 文字标注
    if (ctx.tool === "text") {
      const d = { x: nx, y: ny, val: "", fs: ctx.fontSize };
      textDraftRef.current = d;
      setTextDraft(d);
      return;
    }

    // 序号标注
    if (ctx.tool === "number") {
      const ann: Annotation = {
        type: "number", x1: nx, y1: ny, x2: nx, y2: ny,
        number: numberCounter, color: ctx.color, circleSize: 28,
      };
      addAnnotation(ann);
      setNumberCounter(numberCounter + 1);
      return;
    }

    // 画笔 / 荧光笔
    if (ctx.tool === "pen" || ctx.tool === "highlight") {
      drawingRef.current = {
        type: ctx.tool, x1: nx, y1: ny, x2: nx, y2: ny,
        points: [[nx, ny]],
        color: ctx.color, lineWidth: ctx.tool === "highlight" ? 15 : ctx.width,
      };
      return;
    }

    // rect/oval/line/arrow/diamond
    drawingRef.current = {
      type: ctx.tool as Annotation["type"],
      x1: nx, y1: ny, x2: nx, y2: ny,
      color: ctx.color, lineWidth: ctx.width,
      filled: (ctx.tool === "rect" || ctx.tool === "oval" || ctx.tool === "diamond") ? ctx.filled : undefined,
    };
  }, [annotationsRef, drawingRef, addAnnotation, eraseAnnotationAt, numberCounter, setNumberCounter, commitText]);

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    const { x: nx, y: ny } = ctRef.current(e.clientX, e.clientY);

    // 擦除中
    if (erasingRef.current) {
      eraseAnnotationAt(nx, ny);
      return;
    }

    // 拖拽中
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

    // 绘制中
    if (drawingRef.current) {
      if ((drawingRef.current.type === "pen" || drawingRef.current.type === "highlight") && drawingRef.current.points) {
        drawingRef.current.points.push([nx, ny]);
        setDraftAnn({ ...drawingRef.current, points: [...drawingRef.current.points] });
      } else {
        drawingRef.current = { ...drawingRef.current, x2: nx, y2: ny };
        setDraftAnn({ ...drawingRef.current });
      }
      return;
    }
  }, [eraseAnnotationAt, setAnnotations]);

  const handleMouseUp = useCallback(() => {
    if (drawingRef.current) {
      const ann = drawingRef.current;
      drawingRef.current = null;
      // 过滤误触
      const ok = (ann.type === "pen" || ann.type === "highlight")
        ? (ann.points?.length ?? 0) >= 2
        : (Math.abs(ann.x2 - ann.x1) > 3 || Math.abs(ann.y2 - ann.y1) > 3);
      if (ok) addAnnotation(ann);
      setDraftAnn(null);
    }
    erasingRef.current = false;
    dragRef.current = null;
  }, [drawingRef, addAnnotation]);

  const cancelText = useCallback(() => {
    textDraftRef.current = null;
    setTextDraft(null);
  }, []);

  const setTextDraftVal = useCallback((val: string) => {
    const d = textDraftRef.current;
    if (!d) return;
    const next = { ...d, val };
    textDraftRef.current = next;
    setTextDraft(next);
  }, []);

  return {
    draftAnn, textDraft, textDraftRef,
    handleMouseDown, handleMouseMove, handleMouseUp,
    commitText, cancelText, setTextDraftVal,
    erasingRef, dragRef,
  };
}
