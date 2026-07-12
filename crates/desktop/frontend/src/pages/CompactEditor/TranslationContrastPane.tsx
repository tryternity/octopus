import { useState, useRef, useCallback, useEffect, useMemo } from "react";
import type { EditorView } from "@codemirror/view";
import { undo, redo } from "@codemirror/commands";
import {
  Undo2, Redo2, ZoomIn, ZoomOut, Check, Save, FileText, Eye,
  PanelLeft, Columns2, PanelRight, Languages, Loader2,
} from "lucide-react";
import { CodeMirrorEditor } from "./CodeMirrorEditor";
import { MarkdownPreview } from "./MarkdownPreview";
import { useT } from "@/lib/i18n";

type PaneMode = "editor" | "preview";
type ViewLayout = "left" | "contrast" | "right";

const FONT_MIN = 12;
const FONT_MAX = 24;
const SPLIT_KEY = "contrast-split-ratio";
const MIN_RATIO = 0.2;
const MAX_RATIO = 0.8;

interface TranslationContrastPaneProps {
  originalText: string;
  translatedText: string;
  readOnly: boolean;
  fontSize: number;
  onFontSizeChange: (n: number) => void;
  onOriginalChange: (s: string) => void;
  onTranslatedChange: (s: string) => void;
  onTranslate: () => void;
  onSave: () => void;
  disableSave?: boolean;
  savedFlash: boolean;
  translating: boolean;
}

const ToolBtn = ({ onClick, title, disabled, active, children }: {
  onClick: () => void; title: string; disabled?: boolean; active?: boolean; children: React.ReactNode;
}) => (
  <button
    type="button"
    disabled={disabled}
    title={title}
    onClick={onClick}
    className={`p-1.5 rounded-md transition-colors disabled:opacity-30 disabled:hover:bg-transparent ${
      active ? "bg-accent text-foreground" : "text-muted-foreground hover:bg-accent hover:text-foreground"
    }`}
  >{children}</button>
);

export function TranslationContrastPane({
  originalText, translatedText, readOnly, fontSize, onFontSizeChange,
  onOriginalChange, onTranslatedChange, onTranslate, onSave, disableSave, savedFlash, translating,
}: TranslationContrastPaneProps) {
  const t = useT();
  const [leftMode, setLeftMode] = useState<PaneMode>("editor");
  const [rightMode, setRightMode] = useState<PaneMode>("editor");
  const [viewLayout, setViewLayout] = useState<ViewLayout>("contrast");
  const [translateConfirm, setTranslateConfirm] = useState(false);
  const [splitRatio, setSplitRatio] = useState(() => {
    const saved = Number(localStorage.getItem(SPLIT_KEY));
    return saved >= MIN_RATIO && saved <= MAX_RATIO ? saved : 0.5;
  });
  const dirtyTranslatedRef = useRef(false);
  const leftViewRef = useRef<EditorView | null>(null);
  const rightViewRef = useRef<EditorView | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  const splitRatioRef = useRef(splitRatio);
  useEffect(() => { splitRatioRef.current = splitRatio; }, [splitRatio]);
  const persistSplitRatio = useCallback(() => {
    localStorage.setItem(SPLIT_KEY, String(splitRatioRef.current));
  }, []);

  useEffect(() => { dirtyTranslatedRef.current = false; }, [translatedText]);
  const handleTranslatedChange = useCallback((next: string) => {
    dirtyTranslatedRef.current = true;
    onTranslatedChange(next);
  }, [onTranslatedChange]);

  const confirmTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (confirmTimerRef.current) clearTimeout(confirmTimerRef.current); }, []);

  const handleTranslateClick = useCallback(() => {
    if (dirtyTranslatedRef.current && !translateConfirm) {
      setTranslateConfirm(true);
      confirmTimerRef.current = setTimeout(() => { setTranslateConfirm(false); confirmTimerRef.current = null; }, 2000);
      return;
    }
    if (confirmTimerRef.current) { clearTimeout(confirmTimerRef.current); confirmTimerRef.current = null; }
    setTranslateConfirm(false);
    dirtyTranslatedRef.current = false;
    onTranslate();
  }, [dirtyTranslatedRef, translateConfirm, onTranslate]);

  const handleUndoLeft = useCallback(() => { if (leftViewRef.current) undo(leftViewRef.current); }, []);
  const handleRedoLeft = useCallback(() => { if (leftViewRef.current) redo(leftViewRef.current); }, []);
  const handleUndoRight = useCallback(() => { if (rightViewRef.current) undo(rightViewRef.current); }, []);
  const handleRedoRight = useCallback(() => { if (rightViewRef.current) redo(rightViewRef.current); }, []);

  // ── Splitter 拖拽（复用 MarkdownPane 模式）──
  const onDividerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    draggingRef.current = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    document.documentElement.classList.add("md-splitter-dragging");
  }, []);
  const onDividerMove = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current || !gridRef.current) return;
    const rect = gridRef.current.getBoundingClientRect();
    const next = (e.clientX - rect.left) / rect.width;
    setSplitRatio(Math.min(MAX_RATIO, Math.max(MIN_RATIO, next)));
  }, []);
  const onDividerUp = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    try { (e.target as HTMLElement).releasePointerCapture(e.pointerId); } catch { /* released */ }
    document.documentElement.classList.remove("md-splitter-dragging");
    persistSplitRatio();
  }, [persistSplitRatio]);
  useEffect(() => {
    return () => { document.documentElement.classList.remove("md-splitter-dragging"); };
  }, []);

  const gridCols = viewLayout === "contrast"
    ? `${splitRatio * 100}% 1px ${(1 - splitRatio) * 100}%`
    : "1fr";

  const origCharCount = useMemo(() => [...originalText].length, [originalText]);
  const transCharCount = useMemo(() => [...translatedText].length, [translatedText]);

  const renderPane = (
    label: string,
    charCount: number,
    paneMode: PaneMode,
    setPaneMode: (m: PaneMode) => void,
    text: string,
    onChange: (s: string) => void,
    viewRef: React.RefObject<EditorView | null>,
    onUndo: () => void,
    onRedo: () => void,
    visible: boolean,
  ) => {
    // toggle 按钮：显示将切换到的目标模式图标
    const toggleMode = paneMode === "editor" ? "preview" : "editor";
    return (
      <div
        className="flex flex-col min-h-0 min-w-0 overflow-hidden"
        style={{ display: visible ? "flex" : "none" }}
      >
        <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1 border-b border-border bg-muted/50">
          <button type="button" onClick={onUndo} title={t("editor.undo")} className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground">
            <Undo2 className="w-3.5 h-3.5" />
          </button>
          <button type="button" onClick={onRedo} title={t("editor.redo")} className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground">
            <Redo2 className="w-3.5 h-3.5" />
          </button>
          <span className="text-[11px] text-muted-foreground font-medium ml-1">{label}</span>
          <span className="text-[10px] text-muted-foreground tabular-nums">{t("editor.charCount", { n: charCount })}</span>
          <div className="flex-1" />
          <ToolBtn onClick={() => setPaneMode(toggleMode)} title={toggleMode === "editor" ? t("editor.view.editor") : t("editor.view.preview")} disabled={readOnly && toggleMode === "editor"}>
            {paneMode === "editor" ? <Eye className="w-3.5 h-3.5" /> : <FileText className="w-3.5 h-3.5" />}
          </ToolBtn>
        </div>
        <div className="flex-1 flex min-h-0">
          <div className="flex-1 min-h-0 min-w-0 flex flex-col overflow-hidden" style={{ display: paneMode === "preview" ? "none" : "flex" }}>
            <CodeMirrorEditor value={text} readOnly={readOnly} fontSize={fontSize} onChange={onChange} viewRef={viewRef} />
          </div>
          <div className="flex-1 min-h-0 min-w-0 flex flex-col overflow-hidden" style={{ display: paneMode === "editor" ? "none" : "flex" }}>
            <MarkdownPreview source={text} fontSize={fontSize} />
          </div>
        </div>
      </div>
    );
  };

  return (
    <div className="flex flex-col flex-1 min-h-0">
      <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1.5 border-b border-border bg-muted">
        <ToolBtn onClick={() => onFontSizeChange(Math.max(FONT_MIN, fontSize - 1))} title={t("editor.fontSize")} disabled={fontSize <= FONT_MIN}>
          <ZoomOut className="w-4 h-4" />
        </ToolBtn>
        <span className="text-[11px] text-muted-foreground w-7 text-center tabular-nums">{fontSize}</span>
        <ToolBtn onClick={() => onFontSizeChange(Math.min(FONT_MAX, fontSize + 1))} title={t("editor.fontSize")} disabled={fontSize >= FONT_MAX}>
          <ZoomIn className="w-4 h-4" />
        </ToolBtn>
        <span className="w-px h-4 bg-border mx-1" />
        <ToolBtn onClick={() => setViewLayout("left")} title={t("editor.contrast.layoutOriginal")} active={viewLayout === "left"}>
          <PanelLeft className="w-4 h-4" />
        </ToolBtn>
        <ToolBtn onClick={() => setViewLayout("contrast")} title={t("editor.contrast.layoutContrast")} active={viewLayout === "contrast"}>
          <Columns2 className="w-4 h-4" />
        </ToolBtn>
        <ToolBtn onClick={() => setViewLayout("right")} title={t("editor.contrast.layoutTranslated")} active={viewLayout === "right"}>
          <PanelRight className="w-4 h-4" />
        </ToolBtn>
        <div className="flex-1" />
        <button
          type="button"
          onClick={handleTranslateClick}
          disabled={translating}
          title={translateConfirm ? t("editor.translateConfirm") : t("editor.translate")}
          className={`flex items-center gap-1 px-2.5 py-1 rounded-md text-xs transition-colors ${
            translateConfirm
              ? "bg-red-500 text-white"
              : "bg-[#007aff] hover:bg-[#0066d6] text-white"
          } disabled:opacity-50 disabled:cursor-not-allowed`}
        >
          {translating ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Languages className="w-3.5 h-3.5" />}
          {translateConfirm ? t("editor.translateConfirm") : t("editor.translate")}
        </button>
        <span className="w-px h-4 bg-border mx-1" />
        <button
          type="button"
          disabled={disableSave}
          onClick={onSave}
          className={`flex items-center gap-1 px-2.5 py-1 rounded-md text-xs transition-colors ${
            disableSave
              ? "bg-muted text-muted-foreground cursor-not-allowed"
              : savedFlash ? "bg-emerald-600 text-white" : "bg-[#007aff] hover:bg-[#0066d6] text-white"
          }`}
        >
          {savedFlash ? <Check className="w-3.5 h-3.5" /> : <Save className="w-3.5 h-3.5" />}
          {savedFlash ? t("editor.saved") : t("editor.save")}
          <span className="text-[10px] opacity-70">⌘↵</span>
        </button>
      </div>

      {/* 内容区：grid 布局，contrast 模式有 splitter 可拖拽 */}
      <div
        ref={gridRef}
        className="flex-1 grid min-h-0"
        style={{ gridTemplateColumns: gridCols }}
      >
        {renderPane(
          t("editor.contrast.original"), origCharCount, leftMode, setLeftMode,
          originalText, onOriginalChange, leftViewRef, handleUndoLeft, handleRedoLeft,
          viewLayout !== "right",
        )}
        {/* Splitter（仅 contrast 模式可见） */}
        <div
          role="separator"
          aria-orientation="vertical"
          aria-valuenow={Math.round(splitRatio * 100)}
          onPointerDown={onDividerDown}
          onPointerMove={onDividerMove}
          onPointerUp={onDividerUp}
          onPointerCancel={onDividerUp}
          className="relative bg-border cursor-col-resize select-none hover:bg-voice transition-colors"
          style={{ display: viewLayout === "contrast" ? "block" : "none" }}
        >
          <div className="absolute inset-y-0 -inset-x-[5px]" />
        </div>
        {renderPane(
          t("editor.contrast.translated"), transCharCount, rightMode, setRightMode,
          translatedText, handleTranslatedChange, rightViewRef, handleUndoRight, handleRedoRight,
          viewLayout !== "left",
        )}
      </div>
    </div>
  );
}
