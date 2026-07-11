import { useState, useRef, useCallback, useEffect } from "react";
import type { EditorView } from "@codemirror/view";
import { undo, redo } from "@codemirror/commands";
import { Undo2, Redo2, ZoomIn, ZoomOut, Eraser, Check, Save, Eye, Columns2, FileText } from "lucide-react";
import { CodeMirrorEditor } from "./CodeMirrorEditor";
import { MarkdownPreview } from "./MarkdownPreview";
import { Splitter } from "./Splitter";
import { useSyncScroll } from "@/hooks/useSyncScroll";
import { useT } from "@/lib/i18n";

type ViewMode = "split" | "editor" | "preview";

const FONT_MIN = 12;
const FONT_MAX = 24;
const SPLIT_KEY = "compact-editor-split-ratio";

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
    return saved >= 0.2 && saved <= 0.8 ? saved : 0.5;
  });
  const viewRef = useRef<EditorView | null>(null);

  useEffect(() => {
    localStorage.setItem(SPLIT_KEY, String(splitRatio));
  }, [splitRatio]);

  useSyncScroll({ rebindKey: viewMode });

  const handleUndo = useCallback(() => {
    if (viewRef.current) undo(viewRef.current);
  }, []);
  const handleRedo = useCallback(() => {
    if (viewRef.current) redo(viewRef.current);
  }, []);

  const handleClear = () => {
    if (!clearPending) {
      setClearPending(true);
      window.setTimeout(() => setClearPending(false), 2000);
      return;
    }
    onClear();
    setClearPending(false);
  };

  const charCount = [...text].length;
  const showRight = viewMode !== "editor";
  const showLeft = viewMode !== "preview";

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
        <ToolBtn onClick={handleClear} title={clearPending ? t("editor.clearConfirm") : t("editor.clear")}>
          {clearPending ? <Check className="w-4 h-4 text-red-500" /> : <Eraser className="w-4 h-4" />}
        </ToolBtn>
        <div className="flex-1" />
        <span className="text-[11px] text-muted-foreground mr-2 tabular-nums">
          {t("editor.charCount", { n: charCount })}
        </span>
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

      {/* 内容区 */}
      <div className="flex-1 flex min-h-0">
        {showLeft && showRight ? (
          <Splitter
            left={<CodeMirrorEditor value={text} readOnly={readOnly} fontSize={fontSize} onChange={onChange} viewRef={viewRef} />}
            right={<MarkdownPreview source={text} />}
            ratio={splitRatio}
            onRatioChange={setSplitRatio}
            showRight={true}
          />
        ) : showLeft ? (
          <Splitter
            left={<CodeMirrorEditor value={text} readOnly={readOnly} fontSize={fontSize} onChange={onChange} viewRef={viewRef} />}
            right={null}
            ratio={1}
            onRatioChange={() => {}}
            showRight={false}
          />
        ) : (
          <MarkdownPreview source={text} />
        )}
      </div>
    </div>
  );
}
