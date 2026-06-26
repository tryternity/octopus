import { useState, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";
import { useClipboardHistory } from "@/hooks/useClipboardHistory";
import FilterTabs from "./FilterTabs";
import SearchBar from "./SearchBar";
import ClipboardItemRow from "./ClipboardItem";
import { Pin, X } from "lucide-react";
import { cn } from "@/lib/utils";

export default function Clipboard() {
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [pinned, setPinned] = useState(false);

  const { items, total, refresh } = useClipboardHistory(filter, search);

  const handleClear = useCallback(async () => {
    if (!confirm("清空所有非收藏的历史记录？")) return;
    try {
      await invoke("clear_clipboard_history", { keepFavorite: true });
      refresh();
    } catch (e) {
      console.error(e);
    }
  }, [refresh]);

  const togglePin = useCallback(async () => {
    const next = !pinned;
    setPinned(next);
    try {
      const win = getCurrentWindow();
      await win.setAlwaysOnTop(next);
    } catch (e) {
      console.error(e);
    }
  }, [pinned]);

  return (
    <div className="flex flex-col h-screen bg-background text-foreground select-none overflow-hidden rounded-xl border border-border shadow-2xl shadow-black/8" data-tauri-drag-region>
      {/* Title bar — 极简，去掉"历史" */}
      <div className="flex items-center justify-between px-2 py-1.5" data-tauri-drag-region>
        <button
          className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground transition-colors"
          onClick={() => getCurrentWindow().hide()}
          title="关闭"
        >
          <X className="w-3.5 h-3.5" />
        </button>
        <span className="text-[11px] font-medium tracking-wide text-muted-foreground">剪贴板</span>
        <button
          className={cn(
            "p-1 rounded transition-colors",
            pinned ? "text-voice bg-voice/10" : "text-muted-foreground hover:bg-accent hover:text-foreground",
          )}
          onClick={togglePin}
          title="置顶"
        >
          <Pin className="w-3.5 h-3.5" />
        </button>
      </div>

      {/* Search + Filter */}
      <div className="px-2 pb-1.5 flex flex-col gap-1.5">
        <SearchBar value={search} onChange={setSearch} />
        <FilterTabs value={filter} onChange={setFilter} />
      </div>

      {/* List */}
      <div className="clipboard-list flex-1 overflow-y-auto pb-1">
        {items.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full gap-1 text-muted-foreground/50">
            <span className="text-xs">暂无记录</span>
          </div>
        ) : (
          items.map((item, index) => (
            <ClipboardItemRow key={item.id} item={item} isLast={index === items.length - 1} onChanged={refresh} />
          ))
        )}
      </div>

      {/* Footer */}
      <div className="flex items-center justify-between px-3 py-1 border-t border-border text-[10px] text-muted-foreground/80">
        <span>{total} 条</span>
        <button
          className="hover:text-red-500 transition-colors"
          onClick={handleClear}
        >
          清空
        </button>
      </div>
    </div>
  );
}
