import { useState, useEffect, useRef } from "react";
import { cn } from "@/lib/utils";
import { Download, Upload, Star, Pin } from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { Note, NoteType } from "@/types/note";
import {
  getNote,
  updateNote,
  toggleNotePinned,
  toggleNoteFavorite,
  exportNote,
  importNoteFromFile,
} from "@/lib/notepad";
import MarkdownEditor from "./MarkdownEditor";

export default function NoteEditor({ noteId }: { noteId: number | null }) {
  const [note, setNote] = useState<Note | null>(null);
  const [title, setTitle] = useState("");
  const [textBody, setTextBody] = useState(""); // text 原文 / markdown 源码
  const [toast, setToast] = useState<string | null>(null);
  const currentId = useRef<number | null>(null);

  const noteType: NoteType = note?.note_type ?? "text";

  // 加载笔记
  useEffect(() => {
    if (noteId == null) {
      setNote(null);
      return;
    }
    currentId.current = noteId;
    getNote(noteId).then((n) => {
      if (currentId.current !== noteId) return; // 切换防竞态
      setNote(n ?? null);
      setTitle(n?.title ?? "");
      setTextBody(n?.content_text ?? "");
    });
  }, [noteId]);

  // 标题 / 正文变更 debounce 保存（text/markdown 同走 textBody）
  useEffect(() => {
    if (noteId == null) return;
    const t = setTimeout(() => {
      updateNote(noteId, title, textBody, noteType).catch(console.error);
    }, 800);
    return () => clearTimeout(t);
  }, [title, textBody, noteId, noteType]);

  const flash = (msg: string) => {
    setToast(msg);
    setTimeout(() => setToast(null), 2000);
  };

  if (!note) {
    return (
      <div className="flex-1 flex items-center justify-center text-muted-foreground text-sm">
        选择或新建一条笔记
      </div>
    );
  }

  // 导出：markdown→md，text→txt
  const doExport = async () => {
    const stem = (title || textBody.slice(0, 20) || "note").replace(/\s+/g, "_");
    const ext = noteType === "markdown" ? "md" : "txt";
    const path = await exportNote(stem, ext, textBody);
    flash("已导出: " + path);
  };

  // 导入 .md/.txt → 载入当前笔记正文（text 存原文，markdown 存源码）
  const doImport = async () => {
    const selected = await openDialog({
      filters: [{ name: "Markdown/文本", extensions: ["md", "txt"] }],
    });
    if (!selected) return;
    const content = await importNoteFromFile(selected);
    setTextBody(content);
    flash("已导入");
  };

  return (
    <div className="flex-1 flex flex-col bg-background relative">
      {/* 顶部栏：类型标签 + 导入/导出/收藏/置顶 */}
      <div className="flex items-center gap-0.5 px-2 py-1 border-b border-border">
        <span className="px-2 py-0.5 text-[11px] rounded bg-muted text-muted-foreground">
          {noteType === "markdown" ? "Markdown" : "纯文本"}
        </span>
        <div className="ml-auto flex items-center gap-0.5">
          <button
            className="p-1 rounded hover:bg-accent text-muted-foreground"
            title="导入 md/txt"
            onClick={doImport}
          >
            <Upload className="w-4 h-4" />
          </button>
          <button
            className="p-1 rounded hover:bg-accent text-muted-foreground"
            title="导出"
            onClick={doExport}
          >
            <Download className="w-4 h-4" />
          </button>
          <button
            className={cn(
              "p-1 rounded",
              note.is_favorite
                ? "text-amber-400"
                : "text-muted-foreground hover:bg-accent",
            )}
            title="收藏"
            onClick={async () => {
              await toggleNoteFavorite(note.id);
              const n = await getNote(note.id);
              if (n) setNote(n);
            }}
          >
            <Star className={cn("w-4 h-4", note.is_favorite && "fill-amber-400")} />
          </button>
          <button
            className={cn(
              "p-1 rounded",
              note.is_pinned
                ? "text-foreground"
                : "text-muted-foreground hover:bg-accent",
            )}
            title="置顶"
            onClick={async () => {
              await toggleNotePinned(note.id);
              const n = await getNote(note.id);
              if (n) setNote(n);
            }}
          >
            <Pin className={cn("w-4 h-4", note.is_pinned && "fill-foreground")} />
          </button>
        </div>
      </div>
      {/* 标题 */}
      <input
        className="px-4 pt-3 pb-1 text-lg font-semibold bg-transparent focus:outline-none"
        placeholder="无标题"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
      />
      {/* 编辑区：按 type 分发 */}
      {noteType === "markdown" ? (
        <div className="flex-1 mx-4 mb-4 mt-1">
          <MarkdownEditor value={textBody} onChange={setTextBody} />
        </div>
      ) : (
        <textarea
          className="flex-1 mx-4 mb-4 mt-1 resize-none bg-transparent focus:outline-none text-sm leading-relaxed font-mono"
          placeholder="输入纯文本…"
          value={textBody}
          onChange={(e) => setTextBody(e.target.value)}
        />
      )}
      {toast && (
        <div className="absolute bottom-3 right-3 px-3 py-1.5 rounded bg-foreground text-background text-xs">
          {toast}
        </div>
      )}
    </div>
  );
}
