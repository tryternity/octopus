// 标注状态 hook —— Screenshot 与 RecordAnnotation 共用。
//
// 抽取自：
//   - RecordAnnotation/index.tsx L48-69（已是 ref 模式，作为基线）
//   - RecordAnnotation/index.tsx L134-159（add/undo/redo）
//   - Screenshot/index.tsx L40-79（state 定义）
//   - Screenshot/index.tsx L161-185（add/undo/redo，numberCounter 用 useState）
//
// **修复 numberCounter 不一致**：
//   - 原截图 numberCounter 用 useState，undo 闭包每次 render 重建读到最新 state（能工作但脆弱）
//   - 原录屏 numberCounter 用 useRef + state 镜像，更稳定
//   - 抽取后统一用 ref 模式（ref 为主，state 触发 render）
//
// 业务侧职责（不进 hook）：
//   - 选区 hitTest / move / resize（Screenshot）
//   - canvasRect 偏移（RecordAnnotation）
//   - passthrough 切换（RecordAnnotation）
//   - 文字输入 textarea 浮层（两边差异较大）

import { useState, useRef, useEffect, useCallback } from "react";
import type { Annotation, Tool } from "@/lib/annotation";
import { hitTestAnnotationPrecise } from "@/lib/annotation";

export interface AnnotationState {
  // ── 工具 ──────────────────────────────────
  tool: Tool;
  setTool: (t: Tool) => void;
  toolRef: React.MutableRefObject<Tool>;

  // ── 工具属性 ──────────────────────────────
  toolColor: string;
  setToolColor: (c: string) => void;
  toolColorRef: React.MutableRefObject<string>;
  toolWidth: number;
  setToolWidth: (n: number) => void;
  toolFontSize: number;
  setToolFontSize: (n: number) => void;
  toolFontSizeRef: React.MutableRefObject<number>;
  blurMode: "pixelate" | "gaussian" | "redact";
  setBlurMode: (m: "pixelate" | "gaussian" | "redact") => void;
  blurModeRef: React.MutableRefObject<"pixelate" | "gaussian" | "redact">;
  // 形状/线条子模式（合并按钮后记忆当前子模式）
  shapeMode: "rect" | "oval" | "diamond";
  setShapeMode: (m: "rect" | "oval" | "diamond") => void;
  shapeModeRef: React.MutableRefObject<"rect" | "oval" | "diamond">;
  lineMode: "line" | "arrow" | "pen" | "highlight" | "number";
  setLineMode: (m: "line" | "arrow" | "pen" | "highlight" | "number") => void;
  lineModeRef: React.MutableRefObject<"line" | "arrow" | "pen" | "highlight" | "number">;
  toolFilled: boolean;
  setToolFilled: (f: boolean) => void;
  toolFilledRef: React.MutableRefObject<boolean>;
  toolCircleSize: number;
  setToolCircleSize: (n: number) => void;

  // ── 标注数据 ──────────────────────────────
  annotations: Annotation[];
  annotationsRef: React.MutableRefObject<Annotation[]>;
  setAnnotations: React.Dispatch<React.SetStateAction<Annotation[]>>;
  drawingRef: React.MutableRefObject<Annotation | null>;
  /** 触发 pen 实时重绘的版本号（业务侧 useEffect 依赖它） */
  drawingVer: number;
  setDrawingVer: React.Dispatch<React.SetStateAction<number>>;
  addAnnotation: (ann: Annotation) => void;
  undoAnnotation: () => void;
  redoAnnotation: () => void;
  redoAvailable: boolean;
  eraseAnnotationAt: (x: number, y: number) => void;
  clearAllAnnotations: () => void;
  numberCounter: number;
  numberCounterRef: React.MutableRefObject<number>;
  setNumberCounter: React.Dispatch<React.SetStateAction<number>>;
  selectedAnn: number | null;
  setSelectedAnn: React.Dispatch<React.SetStateAction<number | null>>;

  // ── 浮窗（工具属性 popover） ──────────────
  showPopover: boolean;
  setShowPopover: (b: boolean) => void;
  popoverX: number;
  setPopoverX: (n: number) => void;
}

export function useAnnotationState(): AnnotationState {
  // ── 工具 ──────────────────────────────────
  const [tool, setTool] = useState<Tool>("none");
  const toolRef = useRef<Tool>("none");

  // ── 工具属性（state + ref 镜像）────────────
  const [toolColor, setToolColorState] = useState("#ef4444");
  const toolColorRef = useRef("#ef4444");
  const setToolColor = (c: string) => {
    toolColorRef.current = c;
    setToolColorState(c);
  };

  const [toolWidth, setToolWidth] = useState(3);

  const [toolFontSize, setToolFontSizeState] = useState(16);
  const toolFontSizeRef = useRef(16);
  const setToolFontSize = (s: number) => {
    toolFontSizeRef.current = s;
    setToolFontSizeState(s);
  };

  // blur 渲染模式：pixelate（默认）/ gaussian / redact
  // —— AnnotationToolbar popover 切换，addAnnotation 时写入 blur 标注。
  const [blurMode, setBlurModeState] = useState<"pixelate" | "gaussian" | "redact">("pixelate");
  const blurModeRef = useRef<"pixelate" | "gaussian" | "redact">("pixelate");
  const setBlurMode = useCallback((m: "pixelate" | "gaussian" | "redact") => {
    blurModeRef.current = m;
    setBlurModeState(m);
  }, []);

  // 形状/线条子模式（合并按钮后记忆当前子模式，切回时恢复）
  const [shapeMode, setShapeModeState] = useState<"rect" | "oval" | "diamond">("rect");
  const shapeModeRef = useRef<"rect" | "oval" | "diamond">("rect");
  const setShapeMode = useCallback((m: "rect" | "oval" | "diamond") => {
    shapeModeRef.current = m;
    setShapeModeState(m);
  }, []);
  const [lineMode, setLineModeState] = useState<"line" | "arrow" | "pen" | "highlight" | "number">("line");
  const lineModeRef = useRef<"line" | "arrow" | "pen" | "highlight" | "number">("line");
  const setLineMode = useCallback((m: "line" | "arrow" | "pen" | "highlight" | "number") => {
    lineModeRef.current = m;
    setLineModeState(m);
  }, []);

  const [toolFilled, setToolFilledState] = useState(false);
  const toolFilledRef = useRef(false);
  const setToolFilled = (f: boolean) => {
    toolFilledRef.current = f;
    setToolFilledState(f);
  };

  const [toolCircleSize, setToolCircleSize] = useState(24);

  // ── 标注数据 ──────────────────────────────
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const annotationsRef = useRef<Annotation[]>([]);
  const drawingRef = useRef<Annotation | null>(null);
  const [drawingVer, setDrawingVer] = useState(0);

  const redoStackRef = useRef<Annotation[]>([]);
  const [redoAvailable, setRedoAvailable] = useState(false);

  const [numberCounter, setNumberCounter] = useState(1);
  const numberCounterRef = useRef(1);

  const [selectedAnn, setSelectedAnn] = useState<number | null>(null);

  // ── 浮窗 ──────────────────────────────────
  const [showPopover, setShowPopover] = useState(false);
  const [popoverX, setPopoverX] = useState(0);

  // ── 同步 refs ─────────────────────────────
  useEffect(() => {
    toolRef.current = tool;
  }, [tool]);
  useEffect(() => {
    annotationsRef.current = annotations;
  }, [annotations]);
  useEffect(() => {
    numberCounterRef.current = numberCounter;
  }, [numberCounter]);

  // ── add / undo / redo ─────────────────────
  const addAnnotation = (ann: Annotation) => {
    // blur 标注补 blurMode（调用方未必传，用当前 tool state 兜底）
    if (ann.type === "blur" && !ann.blurMode) {
      ann.blurMode = blurModeRef.current;
    }
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
      // number 标注 undo 时回退计数（用 ref 读最新值，避免闭包陈旧）
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

  // ── eraser / clear / delete ─────────────────
  // eraser：从顶层（数组末尾）往下逐个 hitTest，命中第一个 → 推入 redo 并移除。
  // hitTestAnnotationPrecise 实际签名为 (mx,my,anns[])=>number|null（返回索引），
  // 这里逐个传入单元素数组拿到 i=0 命中结果，等价于 task 描述里的「单 ann 布尔判定」。
  const eraseAnnotationAt = (x: number, y: number) => {
    setAnnotations((prev) => {
      for (let i = prev.length - 1; i >= 0; i--) {
        const hitIdx = hitTestAnnotationPrecise(x, y, [prev[i]]);
        if (hitIdx !== null) {
          redoStackRef.current.push(prev[i]);
          setRedoAvailable(true);
          return prev.filter((_, j) => j !== i);
        }
      }
      return prev;
    });
    setSelectedAnn(null);
  };

  // clearAll：清空全部标注，全部推入 redo，重置 selectedAnn 与 numberCounter。
  const clearAllAnnotations = () => {
    setAnnotations((prev) => {
      if (prev.length === 0) return prev;
      // 倒序入栈，保持 redo 时的相对顺序与撤销一致
      for (let i = prev.length - 1; i >= 0; i--) {
        redoStackRef.current.push(prev[i]);
      }
      setRedoAvailable(true);
      return [];
    });
    setSelectedAnn(null);
    setNumberCounter(1);
  };

  return {
    tool,
    setTool,
    toolRef,
    toolColor,
    setToolColor,
    toolColorRef,
    toolWidth,
    setToolWidth,
    toolFontSize,
    setToolFontSize,
    toolFontSizeRef,
    blurMode,
    setBlurMode,
    blurModeRef,
    shapeMode,
    setShapeMode,
    shapeModeRef,
    lineMode,
    setLineMode,
    lineModeRef,
    toolFilled,
    setToolFilled,
    toolFilledRef,
    toolCircleSize,
    setToolCircleSize,
    annotations,
    annotationsRef,
    setAnnotations,
    drawingRef,
    drawingVer,
    setDrawingVer,
    addAnnotation,
    undoAnnotation,
    redoAnnotation,
    redoAvailable,
    eraseAnnotationAt,
    clearAllAnnotations,
    numberCounter,
    numberCounterRef,
    setNumberCounter,
    selectedAnn,
    setSelectedAnn,
    showPopover,
    setShowPopover,
    popoverX,
    setPopoverX,
  };
}
