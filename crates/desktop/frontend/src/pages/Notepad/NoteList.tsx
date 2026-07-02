import { useState } from "react";
import { cn } from "@/lib/utils";
import {
  Search,
  Pin,
  Star,
  Plus,
  Mic,
  ScanText,
  Clipboard as ClipIcon,
} from "lucide-react";
import type { Note, NoteSource, NoteType } from "@/types/note";
import { useNotes } from "@/hooks/useNotes";
import {
  createNote,
  toggleNotePinned,
  toggleNoteFavorite,
} from "@/lib/notepad";

const SOURCE_TABS: { key: NoteSource | null; label: string }[] = [
  { key: null, label: "全部" },
  { key: "asr", label: "语音" },
  { key: "ocr", label: "OCR" },
  { key: "clipboard", label: "剪贴板" },
];

export default function NoteList({
  selectedId,
  onSelect,
}: {
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  const [tab, setTab] = useState<NoteSource | null>(null);
  const [search, setSearch] = useState("");
  const [favOnly, setFavOnly] = useState(false);
  const [showNewMenu, setShowNewMenu] = useState(false);
  const { items, total, loadMore, hasMore } = useNotes(tab, search, favOnly);

  // 新建笔记：选 type（已建笔记 type 锁定，不可改）
  const handleNew = async (type: NoteType) => {
    setShowNewMenu(false);
    const id = await createNote("manual", null, "", type);
    onSelect(id);
  };

  const handlePin = async (e: React.MouseEvent, id: number) => {
    e.stopPropagation();
    await toggleNotePinned(id);
  };
  const handleFav = async (e: React.MouseEvent, id: number) => {
    e.stopPropagation();
    await toggleNoteFavorite(id);
  };

  return (
    <div className="flex flex-col h-full border-r border-border bg-card">
      {/* 搜索 + 新建 */}
      <div className="p-2 flex items-center gap-1.5 border-b border-border">
        <div className="relative flex-1">
          <Search className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground" />
          <input
            className="w-full pl-7 pr-2 py-1 text-sm rounded bg-background border border-border focus:outline-none focus:ring-1 focus:ring-ring"
            placeholder="搜索笔记"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
        <div className="relative">
          <button
            className="p-1 rounded hover:bg-accent text-foreground"
            onClick={() => setShowNewMenu((v) => !v)}
            title="新建笔记"
          >
            <Plus className="w-4 h-4" />
          </button>
          {showNewMenu && (
            <div className="absolute right-0 top-full mt-1 z-10 w-28 rounded-md border border-border bg-background shadow-md py-0.5">
              {(
                [
                  { type: "html", label: "富文本" },
                  { type: "text", label: "纯文本" },
                  { type: "markdown", label: "Markdown" },
                ] as { type: NoteType; label: string }[]
              ).map((opt) => (
                <button
                  key={opt.type}
                  className="block w-full text-left px-3 py-1 text-xs hover:bg-accent"
                  onClick={() => handleNew(opt.type)}
                >
                  {opt.label}
                </button>
              ))}
            </div>
          )}
        </div>
      </div>
      {/* 来源 tab + 收藏 */}
      <div className="px-2 py-1.5 flex items-center gap-1 border-b border-border overflow-x-auto">
        {SOURCE_TABS.map((t) => (
          <button
            key={t.label}
            className={cn(
              "px-2 py-0.5 text-xs rounded whitespace-nowrap",
              tab === t.key
                ? "bg-primary text-primary-foreground"
                : "hover:bg-accent text-muted-foreground",
            )}
            onClick={() => setTab(t.key)}
          >
            {t.label}
          </button>
        ))}
        <button
          className={cn(
            "ml-auto p-1 rounded",
            favOnly ? "text-amber-400" : "text-muted-foreground hover:bg-accent",
          )}
          onClick={() => setFavOnly((v) => !v)}
          title="仅看收藏"
        >
          <Star className={cn("w-3.5 h-3.5", favOnly && "fill-amber-400")} />
        </button>
      </div>
      {/* 列表 */}
      <div className="flex-1 overflow-y-auto">
        {items.map((n) => (
          <NoteRow
            key={n.id}
            note={n}
            active={n.id === selectedId}
            onSelect={onSelect}
            onPin={handlePin}
            onFav={handleFav}
          />
        ))}
        {items.length === 0 && (
          <div className="p-4 text-center text-xs text-muted-foreground">
            暂无笔记
          </div>
        )}
        {hasMore && (
          <button
            className="w-full py-2 text-xs text-muted-foreground hover:bg-accent"
            onClick={loadMore}
          >
            加载更多（共 {total} 条）
          </button>
        )}
      </div>
    </div>
  );
}

function NoteRow({
  note,
  active,
  onSelect,
  onPin,
  onFav,
}: {
  note: Note;
  active: boolean;
  onSelect: (id: number) => void;
  onPin: (e: React.MouseEvent, id: number) => void;
  onFav: (e: React.MouseEvent, id: number) => void;
}) {
  const preview =
    note.title || note.content_text.slice(0, 60) || "（空笔记）";
  const SourceIcon =
    note.source === "asr"
      ? Mic
      : note.source === "ocr"
        ? ScanText
        : note.source === "clipboard"
          ? ClipIcon
          : null;
  return (
    <div
      className={cn(
        "group px-3 py-2 cursor-pointer border-b border-border/50",
        active ? "bg-accent" : "hover:bg-accent/50",
      )}
      onClick={() => onSelect(note.id)}
    >
      <div className="flex items-center gap-1.5">
        {SourceIcon && (
          <SourceIcon className="w-3 h-3 flex-shrink-0 text-muted-foreground" />
        )}
        {note.note_type !== "html" && (
          <span className="flex-shrink-0 px-1 text-[9px] leading-tight rounded bg-muted text-muted-foreground">
            {note.note_type === "markdown" ? "MD" : "T"}
          </span>
        )}
        <span className="flex-1 truncate text-sm font-medium">{preview}</span>
        <button
          className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100"
          onClick={(e) => onPin(e, note.id)}
          title={note.is_pinned ? "取消置顶" : "置顶"}
        >
          <Pin
            className={cn(
              "w-3 h-3",
              note.is_pinned
                ? "fill-foreground text-foreground"
                : "text-muted-foreground",
            )}
          />
        </button>
        <button
          className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100"
          onClick={(e) => onFav(e, note.id)}
          title="收藏"
        >
          <Star
            className={cn(
              "w-3 h-3",
              note.is_favorite
                ? "fill-amber-400 text-amber-400"
                : "text-muted-foreground",
            )}
          />
        </button>
      </div>
      <div className="mt-0.5 text-[10px] text-muted-foreground">
        {note.updated_at}
      </div>
    </div>
  );
}
