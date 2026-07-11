import { useState, useRef, useCallback, useEffect, useMemo } from "react";
import type { EditorView } from "@codemirror/view";
import { undo, redo } from "@codemirror/commands";
import { Undo2, Redo2, ZoomIn, ZoomOut, Eraser, Check, Save, Eye, Columns2, FileText } from "lucide-react";
import { CodeMirrorEditor } from "./CodeMirrorEditor";
import { MarkdownPreview } from "./MarkdownPreview";
import { useSyncScroll } from "@/hooks/useSyncScroll";
import { useT } from "@/lib/i18n";

type ViewMode = "split" | "editor" | "preview";

const FONT_MIN = 12;
const FONT_MAX = 24;
const SPLIT_KEY = "compact-editor-split-ratio";
const MIN_RATIO = 0.2;
const MAX_RATIO = 0.8;

interface MarkdownPaneProps {
  text: string;
  readOnly: boolean;
  fontSize: number;
  onFontSizeChange: (n: number) => void;
  onChange: (next: string) => void;
  onClear: () => void;
  onSave: () => void;
  disableSave?: boolean;
  savedFlash: boolean;
}

const ToolBtn = ({ onClick, title, disabled, children }: {
  onClick: () => void; title: string; disabled?: boolean; children: React.ReactNode;
}) => (
  <button
    type="button"
    disabled={disabled}
    title={title}
    onClick={onClick}
    className="p-1.5 rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-30 disabled:hover:bg-transparent transition-colors"
  >{children}</button>
);

export function MarkdownPane({
  text, readOnly, fontSize, onFontSizeChange, onChange, onClear, onSave, disableSave, savedFlash,
}: MarkdownPaneProps) {
  const t = useT();
  const [viewMode, setViewMode] = useState<ViewMode>(readOnly ? "preview" : "split");
  const [clearPending, setClearPending] = useState(false);
  const [splitRatio, setSplitRatio] = useState(() => {
    const saved = Number(localStorage.getItem(SPLIT_KEY));
    return saved >= MIN_RATIO && saved <= MAX_RATIO ? saved : 0.5;
  });
  const viewRef = useRef<EditorView | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  // splitRatio 拖拽中只更新 state，释放时落盘（避免逐帧 localStorage IO）
  const splitRatioRef = useRef(splitRatio);
  useEffect(() => { splitRatioRef.current = splitRatio; }, [splitRatio]);
  const persistSplitRatio = useCallback(() => {
    localStorage.setItem(SPLIT_KEY, String(splitRatioRef.current));
  }, []);

  // 仅 split 模式启用滚动同步
  useSyncScroll({ rebindKey: viewMode });

  const handleUndo = useCallback(() => {
    if (viewRef.current) undo(viewRef.current);
  }, []);
  const handleRedo = useCallback(() => {
    if (viewRef.current) redo(viewRef.current);
  }, []);

  const clearTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const handleClear = () => {
    if (!clearPending) {
      setClearPending(true);
      clearTimerRef.current = setTimeout(() => { setClearPending(false); clearTimerRef.current = null; }, 2000);
      return;
    }
    if (clearTimerRef.current) { clearTimeout(clearTimerRef.current); clearTimerRef.current = null; }
    onClear();
    setClearPending(false);
  };

  // 组件卸载时清理 timer
  useEffect(() => {
    return () => { if (clearTimerRef.current) clearTimeout(clearTimerRef.current); };
  }, []);

  // charCount 仅在 text 变化时重算（避免大文档逐键 O(n) spread）
  const charCount = useMemo(() => [...text].length, [text]);

  // ── Splitter 拖拽逻辑（内联，CM6 不卸载）──
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

  // grid 模板列：split 模式按比例分三列；非 split 单轨道占满（display:none 子项不占 grid cell）
  const gridCols = viewMode === "split"
    ? `${splitRatio * 100}% 1px ${(1 - splitRatio) * 100}%`
    : "1fr";

  return (
    <div className="flex flex-col flex-1 min-h-0">
      {/* 工具栏 */}
      <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1.5 border-b border-border bg-muted">
        <ToolBtn onClick={handleUndo} title={t("editor.undo")}><Undo2 className="w-4 h-4" /></ToolBtn>
        <ToolBtn onClick={handleRedo} title={t("editor.redo")}><Redo2 className="w-4 h-4" /></ToolBtn>
        <span className="w-px h-4 bg-border mx-1" />
        <ToolBtn onClick={() => onFontSizeChange(Math.max(FONT_MIN, fontSize - 1))} title={t("editor.fontSize")} disabled={fontSize <= FONT_MIN}>
          <ZoomOut className="w-4 h-4" />
        </ToolBtn>
        <span className="text-[11px] text-muted-foreground w-7 text-center tabular-nums">{fontSize}</span>
        <ToolBtn onClick={() => onFontSizeChange(Math.min(FONT_MAX, fontSize + 1))} title={t("editor.fontSize")} disabled={fontSize >= FONT_MAX}>
          <ZoomIn className="w-4 h-4" />
        </ToolBtn>
        {readOnly ? <span className="w-px h-4 bg-border mx-1" /> : (
          <>
            <span className="w-px h-4 bg-border mx-1" />
            <ToolBtn onClick={handleClear} title={clearPending ? t("editor.clearConfirm") : t("editor.clear")}>
              {clearPending ? <Check className="w-4 h-4 text-red-500" /> : <Eraser className="w-4 h-4" />}
            </ToolBtn>
          </>
        )}
        <div className="flex-1" />
        {/* 视图模式组（右侧，与编辑操作用 flex-1 隔开） */}
        <span className="text-[11px] text-muted-foreground mr-2 tabular-nums">
          {t("editor.charCount", { n: charCount })}
        </span>
        <span className="w-px h-4 bg-border mx-1" />
        <ToolBtn onClick={() => setViewMode("editor")} title={t("editor.view.editor")} disabled={viewMode === "editor"}>
          <FileText className="w-4 h-4" />
        </ToolBtn>
        <ToolBtn onClick={() => setViewMode("split")} title={t("editor.view.split")} disabled={viewMode === "split"}>
          <Columns2 className="w-4 h-4" />
        </ToolBtn>
        <ToolBtn onClick={() => setViewMode("preview")} title={t("editor.view.preview")} disabled={viewMode === "preview"}>
          <Eye className="w-4 h-4" />
        </ToolBtn>
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

      {/* 内容区：CM6 + Preview 始终挂载，display:none 切换可见性（无 mount/unmount） */}
      <div
        ref={gridRef}
        className="flex-1 grid min-h-0"
        style={{ gridTemplateColumns: gridCols }}
      >
        {/* CM6 编辑器 */}
        <div
          className="min-h-0 min-w-0 flex flex-col overflow-hidden"
          style={{ display: viewMode === "preview" ? "none" : "flex" }}
        >
          <CodeMirrorEditor value={text} readOnly={readOnly} fontSize={fontSize} onChange={onChange} viewRef={viewRef} />
        </div>
        {/* 分割线（仅 split 模式可见） */}
        <div
          role="separator"
          aria-orientation="vertical"
          aria-valuenow={Math.round(splitRatio * 100)}
          onPointerDown={onDividerDown}
          onPointerMove={onDividerMove}
          onPointerUp={onDividerUp}
          onPointerCancel={onDividerUp}
          className="relative bg-border cursor-col-resize select-none hover:bg-voice transition-colors"
          style={{ display: viewMode === "split" ? "block" : "none" }}
        >
          <div className="absolute inset-y-0 -inset-x-[5px]" />
        </div>
        {/* Markdown 预览 */}
        <div
          className="min-h-0 min-w-0 flex flex-col overflow-hidden"
          style={{ display: viewMode === "editor" ? "none" : "flex" }}
        >
          <MarkdownPreview source={text} />
        </div>
      </div>
    </div>
  );
}
